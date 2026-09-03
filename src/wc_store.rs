//! State for one open worktree's Working Copy view. Operations spawn on the
//! background executor with a generation counter; stale snapshot completions
//! are dropped, while mutation completions always apply (the disk effect is
//! real whenever it lands).

use crate::engine::{self, commit, diff, mutate, working_copy as eng};
use gpui::{App, AppContext, Context, Entity};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Files,
    Diff,
}

#[derive(Clone, Debug)]
pub enum FileDetail {
    Diff(diff::UnifiedDiff),
    Preview(diff::Preview),
    Failed(String),
}

pub struct WorkingCopyStore {
    pub worktree: PathBuf,
    pub wc: Option<eng::WorkingCopy>,
    pub detail: Option<FileDetail>,
    /// Index into `rows()`.
    pub selected: Option<usize>,
    pub pane: Pane,
    pub author: Option<(String, String)>,
    /// A mutation (stage/unstage/discard/commit) is in flight. Blocks all
    /// other mutating entry points, the discard dialog, and closing the
    /// detail view — never raised by snapshot refreshes, so a refresh can
    /// neither re-arm keys under a pending commit nor trap the user.
    pub(crate) mutating: bool,
    pub message: Option<String>,
    /// Consumed by the app shell: one successful mutation → one home-list
    /// refresh.
    mutated: bool,
    /// The current `message` is the transient "Busy" hint (set by a
    /// mutating entry point that was swallowed while busy). Completions
    /// clear it so the hint never outlives the operation.
    busy_hint: bool,
    /// Guards status/numstat snapshot loads. Bumped by refresh and by
    /// mutations (a mutation invalidates any in-flight snapshot), but NOT
    /// by detail loads — changing the selected file must never cancel a
    /// status refresh, or post-mutation groups go stale.
    generation: u64,
    /// Guards detail (diff/preview) loads, independent of `generation` so
    /// the two load kinds can't cancel each other.
    detail_generation: u64,
}

impl WorkingCopyStore {
    pub fn new(worktree: PathBuf, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_cx| Self {
            worktree: worktree.clone(),
            wc: None,
            detail: None,
            selected: None,
            pane: Pane::Files,
            author: None,
            mutating: false,
            message: None,
            mutated: false,
            busy_hint: false,
            generation: 0,
            detail_generation: 0,
        });
        entity.update(cx, |store, cx| {
            store.refresh(cx);
            store.fetch_author(cx);
        });
        entity
    }

    pub fn rows(&self) -> Vec<(eng::Group, usize)> {
        self.wc.as_ref().map(eng::group_rows).unwrap_or_default()
    }

    pub fn selected_row(&self) -> Option<(eng::Group, &eng::FileEntry)> {
        let idx = self.selected?;
        let (group, entry_idx) = *self.rows().get(idx)?;
        Some((
            group,
            &self.wc.as_ref().expect("rows implies wc").entries[entry_idx],
        ))
    }

    pub fn staged_count(&self) -> usize {
        self.rows()
            .iter()
            .filter(|(g, _)| matches!(g, eng::Group::Staged))
            .count()
    }

    pub fn take_mutated(&mut self) -> bool {
        std::mem::take(&mut self.mutated)
    }

    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected = idx.filter(|&i| i < self.rows().len());
        if self.pane == Pane::Diff {
            self.pane = Pane::Files; // selection change returns focus target to files
        }
        self.load_detail(cx);
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.rows().len();
        if len == 0 {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(s) if s + 1 >= len => s, // list-bounded: stop at the last row
            Some(s) => s + 1,
        };
        self.select(Some(next), cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        let next = match self.selected {
            Some(0) | None => 0,
            Some(s) => s - 1,
        };
        self.select(Some(next.min(self.rows().len().saturating_sub(1))), cx);
    }

    /// Re-runs status and reloads the selected row's detail. Keeps the
    /// selection on the same path when it still exists.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.mutating {
            // A refresh completing mid-mutation would clear the busy hint
            // while mutating keys stay swallowed — refuse instead; the
            // mutation's own completion triggers the authoritative refresh.
            self.busy_message(cx);
            return;
        }
        self.generation += 1;
        let gen = self.generation;
        let worktree = self.worktree.clone();
        let keep_path = self.selected_row().map(|(_, e)| e.path.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { eng::status(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.generation {
                    return;
                }
                match result {
                    Ok(wc) => {
                        if store.busy_hint {
                            store.busy_hint = false;
                            store.message = None;
                        }
                        // Arrow keys aren't gated by `mutating`, so the user
                        // can navigate while a refresh is in flight: the
                        // CURRENT selection wins over the path this refresh
                        // started with. `keep_path` is only a fallback for
                        // when the currently selected row vanished. Both
                        // resolve against the NEW snapshot (path → entry
                        // index → row index) so stale rows are never
                        // indexed into fresh entries. A file present in
                        // BOTH Staged and Unstaged groups resolves against
                        // its (group, path) pair first — a refresh must not
                        // snap an Unstaged selection onto the Staged row,
                        // or the next `s` unstage/stages the wrong surface.
                        let rows = eng::group_rows(&wc);
                        let resolve = |group: Option<eng::Group>, path: &str| -> Option<usize> {
                            let entry = wc.entries.iter().position(|e| e.path == path)?;
                            match group {
                                Some(g) => rows
                                    .iter()
                                    .position(|(rg, i)| *i == entry && *rg == g)
                                    .or_else(|| rows.iter().position(|(_, i)| *i == entry)),
                                None => rows.iter().position(|(_, i)| *i == entry),
                            }
                        };
                        let selected = store
                            .selected_row()
                            .map(|(g, e)| resolve(Some(g), e.path.as_str()))
                            .unwrap_or(None)
                            .or_else(|| keep_path.as_deref().and_then(|p| resolve(None, p)));
                        store.wc = Some(wc);
                        store.selected = selected.or(if rows.is_empty() { None } else { Some(0) });
                        store.load_detail(cx);
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// One-shot author lookup. Deliberately does NOT touch the shared
    /// generation: it starts alongside the initial `refresh`, and bumping
    /// the counter here would cancel that refresh before its result lands.
    fn fetch_author(&mut self, cx: &mut Context<Self>) {
        let worktree = self.worktree.clone();
        cx.spawn(async move |this, cx| {
            let author = cx
                .background_executor()
                .spawn(async move { commit::author(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                store.author = Some(author);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the right detail view for the selected row: unified diff for
    /// staged/unstaged rows, working-tree preview for untracked and
    /// conflicted rows.
    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            self.detail = None;
            return;
        };
        // Detail loads use their own counter: a selection change must cancel
        // an in-flight diff load, but must NOT cancel a status refresh that
        // shares the other counter (and vice versa).
        self.detail_generation += 1;
        let gen = self.detail_generation;
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        let kind = match group {
            eng::Group::Staged => DetailKind::Staged,
            eng::Group::Unstaged => DetailKind::Unstaged,
            eng::Group::Conflicts | eng::Group::Untracked => DetailKind::Preview,
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match kind {
                        DetailKind::Staged => {
                            diff::diff_staged(&worktree, &path).map(FileDetail::Diff)
                        }
                        DetailKind::Unstaged => {
                            diff::diff_unstaged(&worktree, &path).map(FileDetail::Diff)
                        }
                        DetailKind::Preview => {
                            Ok(FileDetail::Preview(diff::read_preview(&worktree, &path)))
                        }
                    }
                })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.detail_generation {
                    return;
                }
                store.detail = Some(match result {
                    Ok(d) => d,
                    Err(e) => FileDetail::Failed(e.message),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Transient hint for keys pressed while the first status snapshot is
    /// still loading — an accurate alternative to "Busy", since nothing is
    /// actually running yet.
    pub fn loading_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Loading working copy…".into());
        self.busy_hint = true;
        cx.notify();
    }

    /// Transient "busy" hint, shared by the mutating entry points and the
    /// shell's guards (e.g. refusing to close the detail view mid-commit).
    pub fn busy_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Busy — wait for the current operation".into());
        self.busy_hint = true;
        cx.notify();
    }

    /// `s` on a row: stage unstaged/untracked/conflict rows, unstage staged
    /// rows. Conflicts: staging marks them resolved.
    pub fn toggle_stage(&mut self, cx: &mut Context<Self>) {
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        if self.wc.is_none() {
            self.loading_message(cx);
            return;
        }
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        if entry.unsupported {
            self.message = Some(
                "filename contains characters git's output lost — stage this one in a terminal"
                    .into(),
            );
            cx.notify();
            return;
        }
        let worktree = self.worktree.clone();
        // Path lists are built per DIRECTION inside the match:
        // - Unstaging a staged RENAME must reset BOTH paths — resetting only
        //   the new path leaves the old path's deletion staged.
        // - STAGING must NOT include `orig_path`: on a `2 RM` record (rename
        //   staged, new path edited again) the old path no longer exists, so
        //   `git add -- new :(literal)old` would abort the whole stage.
        let unstage = matches!(group, eng::Group::Staged);
        let mut paths = vec![entry.path.clone()];
        if unstage {
            if let Some(orig) = &entry.orig_path {
                paths.push(orig.clone());
            }
        }
        // Bump to cancel in-flight snapshot loads; the mutation completion
        // below applies regardless of generation (see `after_mutation`).
        self.generation += 1;
        self.mutating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if unstage {
                        mutate::unstage(&worktree, &paths)
                    } else {
                        mutate::stage(&worktree, &paths)
                    }
                })
                .await;
            this.update(cx, |store, cx| {
                store.after_mutation(result, cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn stage_all(&mut self, cx: &mut Context<Self>) {
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        if self.wc.is_none() {
            self.loading_message(cx);
            return;
        }
        let worktree = self.worktree.clone();
        // Lossy-decoded names can't be matched by a pathspec; including one
        // would abort the whole `git add` (a chunk may already have
        // applied), so they're skipped — the rows are visibly marked
        // "(non-UTF-8 name — unsupported)".
        let paths: Vec<String> = self
            .rows()
            .into_iter()
            .filter(|(g, _)| !matches!(g, eng::Group::Staged | eng::Group::Conflicts))
            .filter_map(|(_, i)| {
                self.wc.as_ref().and_then(|wc| {
                    let e = &wc.entries[i];
                    (!e.unsupported).then(|| e.path.clone())
                })
            })
            .collect();
        if paths.is_empty() {
            return;
        }
        self.generation += 1;
        self.mutating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { mutate::stage(&worktree, &paths) })
                .await;
            this.update(cx, |store, cx| store.after_mutation(result, cx))
                .ok();
        })
        .detach();
    }

    /// Discards a specific path. `untracked_at_confirm` is what the dialog
    /// showed the user; the executed action is only allowed when the file's
    /// LIVE state still agrees with it. If the state flipped mid-dialog
    /// (an external `git rm --cached` / re-add can do that), the action is
    /// refused: a flip to untracked followed by a blind delete would
    /// permanently destroy the only copy of the content.
    pub fn discard_path(
        &mut self,
        untracked_at_confirm: bool,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        let Some(entry) = self
            .wc
            .as_ref()
            .and_then(|wc| wc.entries.iter().find(|e| e.path == path))
            .cloned()
        else {
            self.message =
                Some("That file is no longer in the working copy — reopen the dialog".into());
            cx.notify();
            return;
        };
        if entry.unsupported {
            self.message = Some(
                "filename contains characters git's output lost — discard this one in a terminal"
                    .into(),
            );
            cx.notify();
            return;
        }
        if entry.is_dir() {
            return; // no recursive delete in Phase 1
        }
        if entry.untracked != untracked_at_confirm {
            self.message =
                Some("That file's state changed — reopen the dialog to try again".into());
            cx.notify();
            return;
        }
        let worktree = self.worktree.clone();
        let untracked = entry.untracked;
        self.generation += 1;
        self.mutating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if untracked {
                        mutate::discard_untracked(&worktree, &path)
                    } else {
                        mutate::discard_unstaged(&worktree, &path)
                    }
                })
                .await;
            this.update(cx, |store, cx| store.after_mutation(result, cx))
                .ok();
        })
        .detach();
    }

    fn after_mutation(&mut self, result: engine::Result<()>, cx: &mut Context<Self>) {
        // Mutation completions deliberately skip the generation guard: the
        // disk effect is real whenever it lands; only snapshots are guarded.
        self.mutating = false;
        self.busy_hint = false;
        match result {
            Ok(()) => {
                self.message = None;
                self.mutated = true;
            }
            Err(e) => {
                self.message = Some(if e.is_lock_error() {
                    "another git process may be using this worktree — retry".into()
                } else {
                    e.message
                });
            }
        }
        self.refresh(cx);
    }

    /// While an operation is in flight (`mutating`), every mutating entry point
    /// below early-returns: a second commit editor, or an index mutation
    /// under the pending commit, would corrupt what the user is committing.
    pub fn commit_with_editor(&mut self, cx: &mut Context<Self>) {
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        let Some(wc) = self.wc.clone() else {
            // First snapshot still loading: `c` would be a silent no-op.
            self.loading_message(cx);
            return;
        };
        if self.staged_count() == 0 {
            self.message = Some("Nothing staged — press s on files to stage them first".into());
            cx.notify();
            return;
        }
        let summary = staged_summary(&wc);
        let worktree = self.worktree.clone();
        self.mutating = true;
        self.message = Some("Waiting for commit editor…".into());
        // Bump to cancel in-flight snapshot loads; the completion below
        // applies regardless of generation (see `after_mutation`).
        self.generation += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { commit::commit_with_editor(&worktree, &summary) })
                .await;
            this.update(cx, |store, cx| {
                store.mutating = false;
                // A hint raised mid-editor session (e.g. a swallowed `s`)
                // must not let the follow-up refresh completion erase the
                // commit outcome below.
                store.busy_hint = false;
                match result {
                    Ok(commit::CommitOutcome::Committed) => {
                        store.message = Some("Committed".into());
                        store.mutated = true;
                    }
                    Ok(commit::CommitOutcome::AbortedEmpty) => {
                        store.message = Some("Commit aborted — empty message".into());
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        })
                    }
                }
                store.refresh(cx);
            })
            .ok();
        })
        .detach();
    }
}

enum DetailKind {
    Staged,
    Unstaged,
    Preview,
}

/// "2 staged files: a.txt, b.txt" (names capped at 8, then "…").
pub fn staged_summary(wc: &eng::WorkingCopy) -> String {
    let mut names: Vec<String> = eng::group_rows(wc)
        .into_iter()
        .filter(|(g, _)| matches!(g, eng::Group::Staged))
        .map(|(_, i)| wc.entries[i].path.clone())
        .collect();
    names.dedup();
    let count = names.len();
    if names.len() > 8 {
        names.truncate(8);
        names.push("…".to_string());
    }
    let plural = if count == 1 { "file" } else { "files" };
    format!("{count} staged {plural}: {}", names.join(", "))
}
