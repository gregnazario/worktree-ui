//! State for one open worktree's Working Copy view. Operations spawn on the
//! background executor with a generation counter; stale snapshot completions
//! are dropped, while mutation completions always apply (the disk effect is
//! real whenever it lands).

use crate::engine::{self, commit, diff, mutate, sequence, working_copy as eng};
use gpui::{App, AppContext, Context, Entity};
use std::path::PathBuf;

/// "merge" → "Merge": completion messages start a sentence.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

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

/// Selection and rendering are clamped to this many rows: beyond it the
/// view draws a trailer instead, and an undrawn row must never be
/// selectable (actions on it would look like a frozen list).
pub(crate) const MAX_VISIBLE_ROWS: usize = 1000;
/// Upper bound on diff lines rendered in the detail pane's diff pane.
/// Also bounds the hunk cursor: a hunk whose header the pane never
/// renders (5000+ line diffs truncate) must not be stageable — acting on
/// content the user cannot see is the invisible-action bug
/// MAX_VISIBLE_ROWS fixed for rows.
pub(crate) const DIFF_RENDER_CAP: usize = 5000;

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
    /// The mutation was a COMMIT — the only one that changes reachable
    /// history (stage/unstage/discard only move things between the index
    /// and the worktree).
    history_changed: bool,
    /// Mirrored from the sibling HistoryStore while the detail view is
    /// open: a history action (checkout / worktree add) is in flight, and
    /// this store's mutating entry points must not race it in the same
    /// worktree. Set by the shell observer regardless of entry point.
    pub history_busy: bool,
    /// A paused merge / rebase / cherry-pick / revert in this worktree,
    /// probed from git on every refresh (never app memory). The banner
    /// names it; `g`/`K`/`A` drive it.
    pub in_progress: Option<sequence::InProgress>,
    /// (current step, total) of an in-progress rebase.
    pub rebase_step: Option<(u64, u64)>,
    /// The current `message` is the transient "Busy" hint (set by a
    /// mutating entry point that was swallowed while busy). Completions
    /// clear it so the hint never outlives the operation.
    busy_hint: bool,
    /// A notice to surface after the next mutation completes (e.g.
    /// "Conflicts were skipped…") — survives after_mutation's message
    /// reset, cleared once shown or on the next mutation.
    pending_notice: Option<String>,
    /// Guards status/numstat snapshot loads. Bumped by refresh and by
    /// mutations (a mutation invalidates any in-flight snapshot), but NOT
    /// by detail loads — changing the selected file must never cancel a
    /// status refresh, or post-mutation groups go stale.
    generation: u64,
    /// True when the FIRST status snapshot failed: the view must show the
    /// error instead of an eternal "Loading working copy…".
    pub load_failed: bool,
    /// Guards detail (diff/preview) loads, independent of `generation` so
    /// the two load kinds can't cancel each other.
    detail_generation: u64,
    /// Shared handle to the in-flight commit editor's process; Some only
    /// while a commit-editor session runs. Powers the escape hatch:
    /// `abandon_commit` kills a wedged or forgotten editor instead of
    /// keyboard-locking the detail view until the app quits.
    editor_handle: Option<commit::EditorHandle>,
    /// Hovered hunk in the diff pane (Phase 1b). A bare index, clamped at
    /// every use and re-clamped when a detail load lands — after staging a
    /// hunk the diff shrinks and the cursor naturally points at the next
    /// one. Reset to 0 on selection change.
    hunk_cursor: usize,
    /// What `detail` currently describes: (path, kind). Selection changes
    /// kick off an async detail load, and until it lands `detail` still
    /// holds the PREVIOUS selection's diff — `stage_hunk` must refuse when
    /// the two disagree, or it would build its patch from stale/wrong
    /// content: another FILE (git's preimage check cannot catch
    /// pure-insertion hunks, which would be silently duplicated) or the
    /// same file's OTHER surface (its staged diff applied against the
    /// unstaged row's expectations). A successful mutation clears this
    /// (see `after_mutation`), so post-mutation `s` also waits for the
    /// fresh diff instead of re-reading a pre-mutation one.
    detail_of: Option<(String, DetailKind)>,
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
            history_changed: false,
            history_busy: false,
            in_progress: None,
            rebase_step: None,
            load_failed: false,
            busy_hint: false,
            pending_notice: None,
            generation: 0,
            detail_generation: 0,
            editor_handle: None,
            hunk_cursor: 0,
            detail_of: None,
        });
        entity.update(cx, |store, cx| {
            store.refresh(cx);
            store.fetch_author(cx);
        });
        entity
    }

    /// Selection is clamped to the rows the view actually renders: a
    /// selection on an undrawn row would act invisibly (the list looks
    /// frozen while s/d target something the user cannot see).
    pub(crate) fn selectable_len(&self) -> usize {
        self.rows().len().min(MAX_VISIBLE_ROWS)
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

    /// True when the last mutation was a COMMIT (the only wc mutation
    /// that changes reachable history — the History section's log).
    pub fn take_history_changed(&mut self) -> bool {
        std::mem::take(&mut self.history_changed)
    }

    pub fn take_mutated(&mut self) -> bool {
        std::mem::take(&mut self.mutated)
    }

    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected = idx.filter(|&i| i < self.selectable_len());
        if self.pane == Pane::Diff {
            self.pane = Pane::Files; // selection change returns focus target to files
        }
        self.hunk_cursor = 0; // a new file's diff starts at its first hunk
        self.load_detail(cx);
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.rows().len();
        if len == 0 {
            return;
        }
        let len = self.selectable_len();
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
        self.select(Some(next.min(self.selectable_len().saturating_sub(1))), cx);
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
                .spawn(async move {
                    let status = eng::status(&worktree);
                    // The paused-operation probe rides the same git
                    // batch: the banner must track reality, not a stale
                    // app-side flag.
                    let op = sequence::operation_state(&worktree);
                    let step = if op == Some(sequence::InProgress::Rebase) {
                        sequence::rebase_progress(&worktree)
                    } else {
                        None
                    };
                    (status, op, step)
                })
                .await;
            let (result, op, step) = result;
            this.update(cx, |store, cx| {
                if gen != store.generation {
                    return;
                }
                store.in_progress = op;
                store.rebase_step = step;
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
                        // Match (group, path) at ROW level: a path can have
                        // TWO entries (separate staged + unstaged `1 M`
                        // records), and resolving the entry first always
                        // picks the staged record — snapping an unstaged
                        // selection onto the Staged row on every refresh.
                        let resolve = |group: Option<eng::Group>, path: &str| -> Option<usize> {
                            match group {
                                Some(g) => rows
                                    .iter()
                                    .position(|(rg, i)| *rg == g && wc.entries[*i].path == path)
                                    .or_else(|| {
                                        rows.iter().position(|(_, i)| wc.entries[*i].path == path)
                                    }),
                                None => rows.iter().position(|(_, i)| wc.entries[*i].path == path),
                            }
                        };
                        let selected = store
                            .selected_row()
                            .map(|(g, e)| resolve(Some(g), e.path.as_str()))
                            .unwrap_or(None)
                            .or_else(|| keep_path.as_deref().and_then(|p| resolve(None, p)))
                            // Clamp to the rendered cap: a selection past
                            // MAX_VISIBLE_ROWS would act on an undrawn row.
                            .filter(|&i| i < MAX_VISIBLE_ROWS);
                        store.load_failed = false;
                        store.wc = Some(wc);
                        store.selected = selected.or(if rows.is_empty() { None } else { Some(0) });
                        store.load_detail(cx);
                    }
                    Err(e) => {
                        // A failed FIRST snapshot must not leave the list
                        // rendering "Loading…" forever. The message is
                        // transient: the next successful refresh clears it.
                        store.load_failed = true;
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        });
                        store.busy_hint = true;
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
            // Cancel any in-flight load — otherwise it lands after this
            // clear and reinstates a detail for a row that's gone.
            self.detail_generation += 1;
            self.detail = None;
            self.detail_of = None;
            return;
        };
        // Detail loads use their own counter: a selection change must cancel
        // an in-flight diff load, but must NOT cancel a status refresh that
        // shares the other counter (and vice versa).
        self.detail_generation += 1;
        let gen = self.detail_generation;
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        if entry.unsupported {
            // The lossy-decoded path can never match a pathspec — don't
            // run git on it; show why the pane is empty instead.
            self.detail = Some(FileDetail::Failed(
                "non-UTF-8 filename — view it in a terminal".into(),
            ));
            self.detail_of = Some((entry.path.clone(), DetailKind::Preview));
            cx.notify();
            return;
        }
        let kind = match group {
            eng::Group::Staged => DetailKind::Staged,
            eng::Group::Unstaged => DetailKind::Unstaged,
            eng::Group::Conflicts | eng::Group::Untracked => DetailKind::Preview,
        };
        // The cached diff is about to be replaced: drop the staging trust
        // marker NOW, so an `s` landing between the refresh (`r`, or a
        // mutation's post-completion reload) and the fresh detail refuses
        // instead of patching from the pre-reload diff — if the index
        // changed in that window (the usual reason to press `r`), a
        // pure-insertion hunk with matching context would duplicate.
        // `detail` itself stays visible: the pane keeps showing the old
        // diff while the new one loads.
        self.detail_of = None;
        let loaded_path = path.clone();
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
                store.detail_of = Some((loaded_path, kind));
                // Landing is a new detail revision: the view watches this
                // counter and scrolls the (re-clamped) hovered hunk into
                // view on it.
                store.detail_generation += 1;
                // The new diff may have fewer hunks than the one the cursor
                // was hovering.
                if let Some(FileDetail::Diff(ud)) = &store.detail {
                    store.hunk_cursor = store
                        .hunk_cursor
                        .min(Self::hunk_render_bound(ud).saturating_sub(1));
                }
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
    /// During a commit-editor session the hint doubles as the discoverability
    /// for the escape hatch: esc abandons the commit.
    pub fn busy_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some(if self.editor_handle.is_some() {
            "Busy — the commit editor is open (esc abandons the commit)".into()
        } else {
            "Busy — wait for the current operation".into()
        });
        self.note_transient_hint();
        cx.notify();
    }

    /// True while a commit-editor session (not just any mutation) is in
    /// flight — the state in which `esc` abandons rather than refuses.
    pub fn commit_editor_active(&self) -> bool {
        self.editor_handle.is_some()
    }

    /// Escape hatch for a wedged or forgotten commit editor (`subl -w` tab
    /// left open, editor blocked on a network mount): kill the editor child;
    /// the session's completion then lands as `Abandoned` and re-arms the
    /// view. `mutating` stays set until that completion, so the staged index
    /// can't be mutated between the kill and the bookkeeping.
    pub fn abandon_commit(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = &self.editor_handle else {
            return;
        };
        handle.request_abandon();
        self.message = Some("Abandoning commit — closing the editor…".into());
        self.note_transient_hint();
        cx.notify();
    }

    /// Marks the current `message` as a transient hint (a later refresh
    /// completion clears it). Shell-facing callers that set custom text
    /// use this to keep the clear-on-refresh behavior.
    pub fn note_transient_hint(&mut self) {
        self.busy_hint = true;
    }

    /// `s` on a row: stage unstaged/untracked/conflict rows, unstage staged
    /// rows. Conflicts: staging marks them resolved.
    pub fn toggle_stage(&mut self, cx: &mut Context<Self>) {
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        if self.wc.is_none() {
            // A FAILED first load is a permanent error, not a transient
            // state — the list pane is already showing it; don't overwrite
            // it with a "Loading" hint.
            if !self.load_failed {
                self.loading_message(cx);
            }
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
        // - Unstaging a staged RENAME (`2 R`) must reset BOTH paths —
        //   resetting only the new path leaves the old path's deletion
        //   staged. A staged COPY (`2 C`) is different: the source path is
        //   an independent entry with its own staged changes, and must NOT
        //   be reset along with the copy.
        // - STAGING must NOT include `orig_path`: on a `2 RM` record (rename
        //   staged, new path edited again) the old path no longer exists, so
        //   `git add -- new :old` would abort the whole stage.
        let unstage = matches!(group, eng::Group::Staged);
        let mut paths = vec![entry.path.clone()];
        if unstage && entry.index_status == 'R' {
            if let Some(orig) = &entry.orig_path {
                paths.push(orig.clone());
            }
        }
        // Bump to cancel in-flight snapshot loads — and in-flight DETAIL
        // loads: one that started before the apply would land after
        // `after_mutation`'s invalidation with its generation still
        // current, reinstating a diff computed from the pre-apply index
        // (and re-recording a matching `detail_of`). The mutation
        // completion below applies regardless of generation (see
        // `after_mutation`).
        self.generation += 1;
        self.detail_generation += 1;
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
        // A failed first load keeps its error visible instead of a loading
        // hint (and can never stage anyway — there is no snapshot).
        if self.wc.is_none() {
            if !self.load_failed {
                self.loading_message(cx);
            }
            return;
        }
        let worktree = self.worktree.clone();
        // Lossy-decoded names can't be matched by a pathspec; including one
        // would abort the whole `git add` (a chunk may already have
        // applied), so they're skipped — the rows are visibly marked
        // "(non-UTF-8 name — unsupported)". Conflicts are also skipped:
        // they need resolution, not blind staging — say so instead of
        // letting `S` be a silent no-op.
        let mut skipped_conflicts = 0usize;
        let paths: Vec<String> = self
            .rows()
            .into_iter()
            .filter(|(g, i)| {
                let e = &self.wc.as_ref().unwrap().entries[*i];
                match g {
                    eng::Group::Conflicts => {
                        skipped_conflicts += 1;
                        false
                    }
                    eng::Group::Staged => false,
                    _ => !e.unsupported,
                }
            })
            .filter_map(|(_, i)| {
                self.wc.as_ref().and_then(|wc| {
                    let e = &wc.entries[i];
                    (!e.unsupported).then(|| e.path.clone())
                })
            })
            .collect();
        // Nothing stageable (clean tree, everything already staged, or a
        // conflicts-only tree): say so instead of running a no-op mutation.
        if paths.is_empty() {
            self.message = Some(if skipped_conflicts > 0 {
                "Conflicts must be resolved before they can be staged".into()
            } else {
                "Nothing to stage".into()
            });
            cx.notify();
            return;
        }
        // Proceeding with conflicts still skipped (mixed tree): hold the
        // notice so it survives after_mutation's reset and shows once the
        // staging completes.
        if skipped_conflicts > 0 {
            self.pending_notice =
                Some("Conflicts were skipped — resolve them, then stage with s".into());
        }
        self.generation += 1;
        self.detail_generation += 1;
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

    /// Index of the hovered hunk, clamped to the number of hunks the diff
    /// pane renders (`hunk_bound`) — never the raw hunk count, which the
    /// render cap can truncate (0 when there is no diff). `saturating`
    /// because a non-binary diff can legitimately have ZERO hunks — a
    /// mode-only change or a 100%-similarity rename renders header-only —
    /// and `n - 1` would underflow (panic in debug, usize::MAX in
    /// release).
    pub fn hunk_cursor(&self) -> usize {
        self.hunk_cursor.min(self.hunk_bound().saturating_sub(1))
    }

    /// Revision of the currently-loading-or-loaded detail. Bumped when a
    /// load STARTS (cancelling in-flight loads) and when one LANDS — the
    /// view watches it to scroll the hovered hunk into view on every new
    /// detail revision.
    pub fn detail_generation(&self) -> u64 {
        self.detail_generation
    }

    /// Stable identity of the currently-loaded diff ("kind:path"); the
    /// view compares it across detail revisions to tell "a different diff
    /// loaded" (reset to top) from "the same diff reloaded"
    /// (reveal the hovered hunk — the post-mutation case).
    pub fn detail_key(&self) -> Option<String> {
        self.detail_of.as_ref().map(|(p, k)| format!("{k:?}:{p}"))
    }

    /// True when the hovered-hunk flow is fully live: the selected row is
    /// an unstaged file whose diff is the one actually loaded (not a
    /// selection change still in flight) and at least one hunk renders.
    /// The footer gates its hunk hints on this; `stage_hunk`'s guards
    /// enforce the same conditions.
    pub fn hunk_stageable(&self) -> bool {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            return false;
        };
        if group != eng::Group::Unstaged {
            return false;
        }
        self.detail_of.as_ref() == Some(&(entry.path, DetailKind::Unstaged))
            && self.hunk_bound() > 0
    }

    /// Hunks the diff pane can actually render — only body lines count
    /// toward the line cap: the ceiling for the cursor and for
    /// `stage_hunk`. Zero-hunk
    /// non-binary diffs (mode-only change, pure rename) yield 0 — the
    /// clamp saturates rather than underflowing.
    pub fn hunk_bound(&self) -> usize {
        match self.detail.as_ref() {
            Some(FileDetail::Diff(ud)) if !ud.binary => Self::hunk_render_bound(ud),
            _ => 0,
        }
    }

    fn hunk_render_bound(ud: &diff::UnifiedDiff) -> usize {
        // Only hunks that fit ENTIRELY under the cap count as rendered:
        // a hunk whose body truncates mid-way must not be stageable, its
        // `raw` covers lines the user never saw.
        let mut rendered = 0usize;
        let mut n = 0usize;
        for h in &ud.hunks {
            if rendered + h.lines.len() > DIFF_RENDER_CAP {
                break;
            }
            rendered += h.lines.len();
            n += 1;
        }
        n
    }

    /// Number of hunks in the currently displayed diff, if any.
    pub fn hunk_count(&self) -> Option<usize> {
        match self.detail.as_ref() {
            Some(FileDetail::Diff(ud)) if !ud.binary => Some(ud.hunks.len()),
            _ => None,
        }
    }

    /// Moves the cursor down; returns false when it could not (dead on
    /// non-unstaged rows and pre-load diffs, or already at the bound) —
    /// callers must not scroll on a no-op.
    pub fn hunk_next(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.hunk_stageable() {
            return false;
        }
        if self.hunk_cursor + 1 < self.hunk_bound() {
            self.hunk_cursor += 1;
            cx.notify();
            return true;
        }
        false
    }

    pub fn hunk_prev(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.hunk_stageable() {
            return false;
        }
        if self.hunk_cursor > 0 {
            self.hunk_cursor -= 1;
            cx.notify();
            return true;
        }
        false
    }

    /// `s` with the diff pane focused: stages the hovered hunk. The patch —
    /// the diff's byte-exact header plus the hovered hunk's `raw` — is
    /// cloned from the current detail at keypress time and fed to
    /// `git apply --cached`. A stale patch (the index moved since the diff
    /// was rendered) fails git's preimage check cleanly: the index and
    /// worktree are untouched, and the error suggests staging the whole
    /// file. Binary, untracked, conflict, and non-UTF-8-named rows are
    /// file-level only.
    pub fn stage_hunk(&mut self, cx: &mut Context<Self>) {
        if self.history_busy {
            self.message = Some("Busy — a history action is finishing in this worktree".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        if self.wc.is_none() {
            // A FAILED first load keeps its error visible instead of a
            // loading hint.
            if !self.load_failed {
                self.loading_message(cx);
            }
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
            self.note_transient_hint();
            cx.notify();
            return;
        }
        // No is_dir early return: directory rows are Untracked, and the
        // group match below explains whole-file staging — a dead key where
        // the footer advertises `s stage hunk` reads as a freeze.
        match group {
            eng::Group::Unstaged => {}
            eng::Group::Staged => {
                self.message = Some(
                    "hunk staging applies to unstaged changes — select the file's unstaged row"
                        .into(),
                );
                self.note_transient_hint();
                cx.notify();
                return;
            }
            // Conflicts get their own hint: S (stage-all) deliberately
            // skips them, so recommending it would send the user in a loop.
            eng::Group::Conflicts => {
                self.message =
                    Some("resolve conflicts in your editor, then press s to mark resolved".into());
                self.note_transient_hint();
                cx.notify();
                return;
            }
            eng::Group::Untracked => {
                self.message = Some("no hunks here — stage it with s on the file row".into());
                self.note_transient_hint();
                cx.notify();
                return;
            }
        }
        // The detail lags the selection: `select()` only STARTS an async
        // load, and until it lands `self.detail` still describes the
        // PREVIOUS selection — possibly another file, or the same file's
        // OTHER surface (its staged diff). Building the patch from either
        // would stage unverified content (git's preimage net cannot catch
        // pure-insertion hunks — they'd be silently duplicated), so a
        // mismatch refuses until the new diff arrives.
        let wanted_kind = DetailKind::Unstaged;
        if self.detail_of.as_ref() != Some(&(entry.path.clone(), wanted_kind)) {
            self.message = Some("diff is loading — try again".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        // Never a silent no-op — the footer advertises `s stage hunk`, so
        // a dead key must explain itself. A FAILED load is distinct from a
        // genuinely hunk-less diff: "stage the whole file" is the wrong
        // remedy for a transient git error; a retry is.
        let Some(FileDetail::Diff(ud)) = self.detail.as_ref() else {
            self.message = Some(
                if matches!(self.detail.as_ref(), Some(FileDetail::Failed(_))) {
                    "the diff failed to load — press r to retry".to_string()
                } else {
                    "no hunks in this diff — stage the whole file with s on the file row"
                        .to_string()
                },
            );
            self.note_transient_hint();
            cx.notify();
            return;
        };
        if ud.binary {
            self.message = Some("binary file — stage it whole with s on the file row".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let cursor = self.hunk_cursor();
        let Some(hunk) = ud.hunks.get(cursor).filter(|_| cursor < self.hunk_bound()) else {
            // Zero-hunk non-binary diff (mode-only change, pure rename), or
            // every hunk truncated by the render cap: nothing the pane
            // actually shows is stageable hunk-wise.
            self.message =
                Some("no hunks in this diff — stage the whole file with s on the file row".into());
            self.note_transient_hint();
            cx.notify();
            return;
        };
        let patch = mutate::content_patch(&ud.header_raw, &hunk.raw);
        let pre_image = ud.index_pre_image.clone();
        let path = entry.path.clone();
        let worktree = self.worktree.clone();
        // Bump to cancel in-flight snapshot loads — and in-flight DETAIL
        // loads: one that started before the apply would land after
        // `after_mutation`'s invalidation with its generation still
        // current, reinstating a diff computed from the pre-apply index
        // (and re-recording a matching `detail_of`). The mutation
        // completion below applies regardless of generation (see
        // `after_mutation`).
        self.generation += 1;
        self.detail_generation += 1;
        self.mutating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    mutate::apply_cached(&worktree, &path, patch, pre_image.as_deref())
                })
                .await;
            this.update(cx, |store, cx| {
                store.after_mutation(
                    result.map_err(|e| engine::GitError {
                        // The blanket whole-file hint fits a rejected
                        // patch; the stale-index refusal's own remedy is a
                        // refresh — appending both would offer
                        // contradictory advice.
                        message: match e {
                            mutate::ApplyError::Git(git) => {
                                format!("{git} — stage the whole file instead (s on the file row)")
                            }
                            stale => stale.to_string(),
                        },
                    }),
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Discards a specific path. `untracked_at_confirm` is what the dialog
    /// showed the user; the executed action is derived from a LIVE
    /// `git ls-files` probe inside the background task, and a file whose
    /// live class contradicts what was confirmed (an external `git add` /
    /// `rm --cached` while the dialog sat open) is refused with a "reopen
    /// the dialog" message instead of acting on the stale snapshot — the
    /// probe is the single source of truth for both the refusal and the
    /// restore-vs-delete choice.
    pub fn discard_path(
        &mut self,
        untracked_at_confirm: bool,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if self.history_busy {
            self.message = Some("Busy — a history action is finishing in this worktree".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
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
        let worktree = self.worktree.clone();
        self.generation += 1;
        self.detail_generation += 1;
        self.mutating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    // Re-derive tracked-ness from LIVE git state: an
                    // external `git add`/`rm --cached` between dialog-open
                    // and confirm flips the file's class, and acting on the
                    // stale snapshot would either delete unique content or
                    // restore something unexpected. A probe ERROR is also a
                    // refusal: tracked-ness was never established, and
                    // defaulting to "untracked" would make an `ls-files`
                    // failure resolve into the destructive branch.
                    let tracked_now = engine::run_trimmed(
                        &worktree,
                        &["ls-files", "--", &format!(":(literal){path}")],
                    )
                    .ok()
                    .map(|out| !out.is_empty());
                    match tracked_now {
                        // Refuse (completion refreshes so a reopened
                        // dialog shows the flipped state).
                        None => None,
                        Some(t) if t == untracked_at_confirm => None,
                        Some(true) => Some(mutate::discard_unstaged(&worktree, &path)),
                        Some(false) => Some(mutate::discard_untracked(&worktree, &path)),
                    }
                })
                .await;
            this.update(cx, |store, cx| match outcome {
                Some(result) => store.after_mutation(result, cx),
                None => {
                    store.mutating = false;
                    store.busy_hint = false;
                    store.message =
                        Some("That file's state changed — reopen the dialog to try again".into());
                    store.refresh(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shared pre-flight for the sequence keys (g / K / A): an unrelated
    /// mutation in flight, a history action racing the worktree, or
    /// nothing actually paused all explain themselves.
    fn sequence_blocker(&self) -> Option<String> {
        if self.mutating {
            return Some("Busy — wait for the current operation".into());
        }
        if self.history_busy {
            return Some("Busy — a history action is finishing in this worktree".into());
        }
        None
    }

    /// `g`: continues the paused operation with its stored message (no
    /// editor). Still-unresolved files come back as git's own refusal.
    pub fn continue_op(&mut self, cx: &mut Context<Self>) {
        if let Some(blocked) = self.sequence_blocker() {
            self.message = Some(blocked);
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let Some(op) = self.in_progress else {
            self.message = Some("No operation in progress".into());
            self.note_transient_hint();
            cx.notify();
            return;
        };
        let worktree = self.worktree.clone();
        self.mutating = true;
        self.message = Some(format!("Continuing {}…", op.label()));
        self.note_transient_hint();
        cx.notify();
        let label = op.label().to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { sequence::continue_op(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                store.mutating = false;
                match result {
                    Ok(conflicts) if conflicts.is_empty() => {
                        store.message = Some(format!("{} completed", capitalize(&label)));
                        // The continue created a commit (merge/rebase/
                        // cherry-pick/revert all can) and touched files.
                        store.mutated = true;
                        store.history_changed = true;
                        store.busy_hint = false;
                        store.after_sequence_state_change(cx);
                    }
                    Ok(conflicts) => {
                        store.message = Some(format!(
                            "Still unresolved: {} — resolve, stage with s, then g",
                            conflicts.join(", ")
                        ));
                        store.busy_hint = true;
                        store.after_sequence_state_change(cx);
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        });
                        store.busy_hint = true;
                        store.after_sequence_state_change(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `K`: skips the current step of a rebase / cherry-pick. The next
    /// step may conflict immediately — reported the same way.
    pub fn skip_op(&mut self, cx: &mut Context<Self>) {
        if let Some(blocked) = self.sequence_blocker() {
            self.message = Some(blocked);
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let Some(op) = self.in_progress else {
            self.message = Some("No operation in progress".into());
            self.note_transient_hint();
            cx.notify();
            return;
        };
        if !op.skippable() {
            self.message = Some(format!("A paused {} has no step to skip", op.label()));
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let worktree = self.worktree.clone();
        self.mutating = true;
        self.message = Some("Skipping…".into());
        self.note_transient_hint();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { sequence::skip_op(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                store.mutating = false;
                match result {
                    Ok(conflicts) if conflicts.is_empty() => {
                        store.message = Some("Step skipped".into());
                        store.mutated = true;
                        store.history_changed = true;
                        store.busy_hint = false;
                        store.after_sequence_state_change(cx);
                    }
                    Ok(conflicts) => {
                        store.message = Some(format!(
                            "Next step conflicts in {} — resolve, stage with s, then g",
                            conflicts.join(", ")
                        ));
                        store.busy_hint = true;
                        store.after_sequence_state_change(cx);
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        });
                        store.busy_hint = true;
                        store.after_sequence_state_change(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `A`: aborts the paused operation, restoring the pre-operation
    /// state (no history change — aborts never create commits).
    pub fn abort_op(&mut self, cx: &mut Context<Self>) {
        if let Some(blocked) = self.sequence_blocker() {
            self.message = Some(blocked);
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let Some(op) = self.in_progress else {
            self.message = Some("No operation in progress".into());
            self.note_transient_hint();
            cx.notify();
            return;
        };
        let worktree = self.worktree.clone();
        self.mutating = true;
        self.message = Some(format!("Aborting {}…", op.label()));
        self.note_transient_hint();
        cx.notify();
        let label = op.label().to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { sequence::abort_op(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                store.mutating = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("{} aborted", capitalize(&label)));
                        store.mutated = true;
                        store.busy_hint = false;
                        store.after_sequence_state_change(cx);
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        });
                        store.busy_hint = true;
                        store.after_sequence_state_change(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The cached snapshot and every cached diff just went stale (the
    /// index/worktree changed under the sequence action). Same
    /// invalidation `after_mutation` does, plus a refresh that re-probes
    /// the paused state — the banner tracks the probe, not a flag.
    fn after_sequence_state_change(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.detail_of = None;
        self.refresh(cx);
    }

    fn after_mutation(&mut self, result: engine::Result<()>, cx: &mut Context<Self>) {
        // Mutation completions deliberately skip the generation guard: the
        // disk effect is real whenever it lands; only snapshots are guarded.
        self.mutating = false;
        self.busy_hint = false;
        match result {
            Ok(()) => {
                // The index just changed: every cached diff is stale, and
                // `stage_hunk`'s (path, kind) guard would happily re-apply
                // a pre-mutation hunk from it. Invalidate so the next `s`
                // waits for the fresh post-mutation diff (the refresh below
                // reloads it); the pane shows its loading placeholder for
                // the sub-second reload.
                self.detail = None;
                self.detail_of = None;
                // Surface a pending notice (e.g. skipped conflicts) instead
                // of clearing; the mutation still counts for the home list.
                self.message = self.pending_notice.take();
                self.mutated = true;
            }
            Err(e) => {
                self.pending_notice = None;
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
    /// The one exception is the escape hatch: during an editor session `esc`
    /// routes to `abandon_commit` (via the shell's close_detail) instead of
    /// being swallowed.
    pub fn commit_with_editor(&mut self, cx: &mut Context<Self>) {
        if self.history_busy {
            self.message = Some("Busy — a history action is finishing in this worktree".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        if self.mutating {
            self.busy_message(cx);
            return;
        }
        let Some(wc) = self.wc.clone() else {
            // First snapshot still loading: `c` would be a silent no-op.
            // (A FAILED load keeps its error visible instead.)
            if !self.load_failed {
                self.loading_message(cx);
            }
            return;
        };
        if self.staged_count() == 0 {
            self.message = Some("Nothing staged — press s on files to stage them first".into());
            cx.notify();
            return;
        }
        let summary = staged_summary(&wc);
        let worktree = self.worktree.clone();
        let editor_handle = commit::EditorHandle::new();
        self.editor_handle = Some(editor_handle.clone());
        self.mutating = true;
        self.message = Some("Waiting for commit editor — esc abandons the commit".into());
        // Bump to cancel in-flight snapshot loads; the completion below
        // applies regardless of generation (see `after_mutation`).
        self.generation += 1;
        self.detail_generation += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    commit::commit_with_editor(&worktree, &summary, Some(&editor_handle))
                })
                .await;
            this.update(cx, |store, cx| {
                store.mutating = false;
                store.editor_handle = None;
                // A hint raised mid-editor session (e.g. a swallowed `s`)
                // must not let the follow-up refresh completion erase the
                // commit outcome below.
                store.busy_hint = false;
                match result {
                    Ok(commit::CommitOutcome::Committed) => {
                        // The index changed (staged content is now history):
                        // cached diffs are stale until the refresh below
                        // reloads them.
                        store.detail = None;
                        store.detail_of = None;
                        store.message = Some("Committed".into());
                        store.mutated = true;
                        store.history_changed = true;
                    }
                    Ok(commit::CommitOutcome::AbortedEmpty { draft }) => {
                        store.message = Some(match draft {
                            Some(p) => format!(
                                "Commit aborted — empty message; \
                                 your draft is preserved at {}",
                                p.display()
                            ),
                            None => "Commit aborted — empty message".into(),
                        });
                    }
                    Ok(commit::CommitOutcome::Abandoned { draft }) => {
                        store.message = Some(match draft {
                            Some(p) => format!(
                                "Commit abandoned — staged changes kept; \
                                 your message draft is preserved at {}",
                                p.display()
                            ),
                            None => "Commit abandoned — staged changes kept".into(),
                        });
                    }
                    Err(e) => {
                        // Append (don't replace): a commit failure carries
                        // the preserved-message path that must reach the
                        // user even when this is also a lock error.
                        store.message = Some(if e.is_lock_error() {
                            format!("{e} — another git process may be using this worktree; retry")
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    // Paths are interpolated into a comment-prefixed template line: a
    // newline inside a name would smuggle the following template content
    // into the committed message. Quote such names git-style instead.
    let quoted: Vec<String> = names
        .iter()
        .map(|n| {
            if n.contains('\n') || n.contains('\r') {
                format!("{n:?}")
            } else {
                n.clone()
            }
        })
        .collect();
    format!("{count} staged {plural}: {}", quoted.join(", "))
}
