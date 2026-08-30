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
    pub busy: bool,
    pub message: Option<String>,
    /// Consumed by the app shell: one successful mutation → one home-list
    /// refresh.
    mutated: bool,
    generation: u64,
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
            busy: false,
            message: None,
            mutated: false,
            generation: 0,
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
            Some(s) if s + 1 >= len => s, // group-bounded: stop at the last row
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
        self.busy = true;
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
                store.busy = false;
                match result {
                    Ok(wc) => {
                        // Resolve the keep-path against the NEW snapshot
                        // (path → entry index → row index) so a vanished
                        // file simply falls back to the first row instead
                        // of indexing stale rows into fresh entries.
                        let rows = eng::group_rows(&wc);
                        let selected = keep_path.as_ref().and_then(|p| {
                            wc.entries
                                .iter()
                                .position(|e| &e.path == p)
                                .and_then(|entry| rows.iter().position(|(_, i)| *i == entry))
                        });
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
        self.generation += 1;
        let gen = self.generation;
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
                if gen != store.generation {
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

    /// `s` on a row: stage unstaged/untracked/conflict rows, unstage staged
    /// rows. Conflicts: staging marks them resolved.
    pub fn toggle_stage(&mut self, cx: &mut Context<Self>) {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        // Bump to cancel in-flight snapshot loads; the mutation completion
        // below applies regardless of generation (see `after_mutation`).
        self.generation += 1;
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match group {
                        eng::Group::Staged => {
                            mutate::unstage(&worktree, std::slice::from_ref(&path))
                        }
                        _ => mutate::stage(&worktree, std::slice::from_ref(&path)),
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
        let worktree = self.worktree.clone();
        let paths: Vec<String> = self
            .rows()
            .into_iter()
            .filter(|(g, _)| !matches!(g, eng::Group::Staged))
            .filter_map(|(_, i)| self.wc.as_ref().map(|wc| wc.entries[i].path.clone()))
            .collect();
        if paths.is_empty() {
            return;
        }
        self.generation += 1;
        self.busy = true;
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

    /// Caller shows the confirmation dialog first; this executes.
    pub fn discard_selected(&mut self, cx: &mut Context<Self>) {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        if entry.is_dir() {
            return; // no recursive delete in Phase 1
        }
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        self.generation += 1;
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match group {
                        eng::Group::Untracked => mutate::discard_untracked(&worktree, &path),
                        eng::Group::Unstaged => mutate::discard_unstaged(&worktree, &path),
                        _ => Ok(()), // staged/conflicts: dialog is not offered
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
        self.busy = false;
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

    pub fn commit_with_editor(&mut self, cx: &mut Context<Self>) {
        let Some(wc) = self.wc.clone() else { return };
        if self.staged_count() == 0 {
            self.message = Some("Nothing staged — press s on files to stage them first".into());
            cx.notify();
            return;
        }
        let summary = staged_summary(&wc);
        let worktree = self.worktree.clone();
        self.busy = true;
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
                store.busy = false;
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
