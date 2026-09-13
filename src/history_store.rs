//! Store for the History section of the worktree detail view: commit list
//! with graph lanes, selection-driven commit detail (changed files +
//! per-file diff), and the history actions (copy hash, checkout,
//! open-in-new-worktree). Same async discipline as `wc_store`: git runs on
//! the background executor, loads are guarded by a detail-generation
//! counter, and every refusal explains itself.

use crate::engine::{diff, history};
use gpui::{App, AppContext, Context, Entity};
use std::path::PathBuf;

/// Commits fetched per batch; `L` grows this by another batch.
pub const HISTORY_BATCH: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Commits,
    Files,
}

pub struct HistoryStore {
    pub worktree: PathBuf,
    pub commits: Vec<history::LogCommit>,
    pub rows: Vec<history::GraphRow>,
    pub selected: Option<usize>,
    /// Files changed by the selected commit.
    pub files: Option<Vec<history::CommitFile>>,
    pub selected_file: Option<usize>,
    /// Unified diff of the selected file, or the load error text.
    pub file_diff: Result<diff::UnifiedDiff, String>,
    /// Error from the last commit-files load (None while empty means
    /// "loading", Some means the pane must show the failure).
    pub files_error: Option<String>,
    pub pane: Pane,
    /// Batch size for the next load-more.
    pub max_count: usize,
    pub message: Option<String>,
    pub busy_hint: bool,
    /// One successful worktree-affecting action (checkout, worktree add) →
    /// one home-list refresh.
    mutated: bool,
    /// Guards detail loads (files/diff), independent per load kind is not
    /// needed here: files and diff both belong to (commit, file) and are
    /// re-issued together on every selection change.
    detail_generation: u64,
    /// Guards log loads (refresh / load-more).
    pub load_generation: u64,
    /// A load-more fetch is in flight; extra presses get a hint instead
    /// of re-requesting the same window.
    load_more_in_flight: bool,
    /// True when the FIRST log load failed; the view shows the error
    /// instead of an eternal "Loading history…".
    pub load_failed: bool,
    /// A worktree-affecting action (checkout / worktree add) is in flight;
    /// blocks further actions until its completion lands.
    action_in_flight: bool,
    /// True when the log fetch returned the full batch — older commits
    /// likely exist and `L` will find them.
    pub has_more: bool,
    /// True once the first log load completed (either way): the view
    /// distinguishes "loading" from "no commits yet".
    pub initial_load_done: bool,
}

impl HistoryStore {
    pub fn new(worktree: PathBuf, cx: &mut App) -> Entity<Self> {
        Self::new_with_batch(worktree, HISTORY_BATCH, cx)
    }

    /// Batch size injectable for tests (has_more/load-more behavior needs
    /// a batch smaller than the fixture's history).
    pub fn new_with_batch(worktree: PathBuf, batch: usize, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_cx| Self {
            worktree: worktree.clone(),
            commits: Vec::new(),
            rows: Vec::new(),
            selected: None,
            files: None,
            selected_file: None,
            files_error: None,
            file_diff: Err(String::new()),
            pane: Pane::Commits,
            max_count: batch,
            message: None,
            busy_hint: false,
            mutated: false,
            detail_generation: 0,
            load_generation: 0,
            load_more_in_flight: false,
            load_failed: false,
            action_in_flight: false,
            has_more: false,
            initial_load_done: false,
        });
        entity.update(cx, |store, cx| store.refresh(cx));
        entity
    }

    /// One worktree-affecting action consumed by the app shell (home-list
    /// refresh), mirroring `wc_store::take_mutated`.
    pub fn take_mutated(&mut self) -> bool {
        std::mem::take(&mut self.mutated)
    }

    /// Re-fetches the log (keeping the batch size), preserving the
    /// selection by commit hash.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let gen = self.load_generation;
        let worktree = self.worktree.clone();
        // Fetch one extra row so truncation (more history exists) is
        // distinguishable from exhaustion.
        let skip = 0;
        let max_count = self.max_count + 1;
        let keep_hash = self
            .selected
            .and_then(|i| self.commits.get(i))
            .map(|c| c.hash.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::log(&worktree, skip, max_count) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.load_generation {
                    return;
                }
                // A refresh supersedes any in-flight load-more: that
                // completion will hit this stale generation and bail, so
                // the in-flight flag is released HERE or `L` stays off.
                store.load_more_in_flight = false;
                match result {
                    Ok(mut fetched) => {
                        store.load_failed = false;
                        store.initial_load_done = true;
                        // A transient hint ("Loading N commits…") from the
                        // load that just landed must not outlive it.
                        store.has_more = fetched.len() > store.max_count;
                        fetched.truncate(store.max_count);
                        let mut commits = fetched;
                        store.rows = history::assign_lanes(&mut commits);
                        store.commits = commits;
                        // Keep the selection on the same commit; clamp to
                        // the list otherwise.
                        store.selected = keep_hash
                            .and_then(|h| store.commits.iter().position(|c| c.hash == h))
                            .or_else(|| {
                                // Same commit may have dropped out of the
                                // batch: clamp instead of losing the slot.
                                let sel = store.selected?;
                                store.commits.get(sel)?;
                                Some(sel)
                            })
                            // First load: start at the newest commit.
                            .or(if store.commits.is_empty() {
                                None
                            } else {
                                Some(0)
                            });
                        // A reload invalidates the commit detail.
                        store.files = None;
                        store.selected_file = None;
                        store.file_diff = Err(String::new());
                        if store.busy_hint {
                            store.message = None;
                            store.busy_hint = false;
                        }
                        store.load_commit_files(cx);
                    }
                    Err(e) => {
                        // Only a failed FIRST load replaces the list with
                        // the error pane; later failures (manual `r`,
                        // transient lock contention) keep the working list
                        // and surface the error in the footer.
                        store.load_failed = !store.initial_load_done;
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

    /// Loads the next batch of OLDER commits and appends it. Only the new
    /// window is fetched (`--skip`); lanes are re-assigned over the whole
    /// list in memory — a forward-only sweep, so already-rendered lanes
    /// can never change. Selection is untouched.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.has_more {
            self.message = Some("No older commits to load".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        if self.load_more_in_flight {
            // The fetch is already running: extra presses must not
            // re-request the same window (the generation check would
            // silently drop them).
            self.message = Some("Already loading older commits…".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        self.load_more_in_flight = true;
        self.load_generation += 1;
        let gen = self.load_generation;
        let worktree = self.worktree.clone();
        let skip = self.commits.len();
        self.message = Some(format!("Loading {} older commits…", HISTORY_BATCH));
        self.note_transient_hint();
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Batch + 1 distinguishes truncation from exhaustion.
            let result = cx
                .background_executor()
                .spawn(async move { history::log(&worktree, skip, HISTORY_BATCH + 1) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.load_generation {
                    return;
                }
                store.load_more_in_flight = false;
                match result {
                    Ok(mut fetched) => {
                        store.load_failed = false;
                        store.has_more = fetched.len() > HISTORY_BATCH;
                        fetched.truncate(HISTORY_BATCH);
                        let mut commits = std::mem::take(&mut store.commits);
                        commits.append(&mut fetched);
                        store.rows = history::assign_lanes(&mut commits);
                        store.commits = commits;
                        // Sync the loaded depth so a later `r` re-fetches
                        // everything `L` loaded instead of truncating.
                        store.max_count = store.commits.len();
                        if store.busy_hint {
                            store.message = None;
                            store.busy_hint = false;
                        }
                    }
                    Err(e) => {
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

    /// Transient hint for keys swallowed while the log is still loading.
    pub fn loading_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Loading history…".into());
        self.note_transient_hint();
        cx.notify();
    }

    /// Transient "busy" hint for keys swallowed while an action runs.
    pub fn busy_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Busy — wait for the current operation".into());
        self.note_transient_hint();
        cx.notify();
    }

    pub fn note_transient_hint(&mut self) {
        self.busy_hint = true;
    }

    /// Selection is bounded by the fetched commits.
    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected = idx.filter(|&i| i < self.commits.len());
        self.selected_file = None;
        self.files = None;
        self.files_error = None;
        self.file_diff = Err(String::new());
        if self.pane == Pane::Files {
            self.pane = Pane::Commits;
        }
        self.load_commit_files(cx);
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.commits.is_empty() {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(s) if s + 1 >= self.commits.len() => return, // boundary: no-op
            Some(s) => s + 1,
        };
        self.select(Some(next), cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        if self.commits.is_empty() {
            return;
        }
        let prev = match self.selected {
            Some(0) | None => return, // boundary: no-op
            Some(s) => s - 1,
        };
        self.select(Some(prev), cx);
    }

    pub fn select_file(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected_file = idx.filter(|&i| self.files.as_ref().is_some_and(|f| i < f.len()));
        if self.pane == Pane::Commits {
            self.pane = Pane::Files;
        }
        self.load_file_diff(cx);
        cx.notify();
    }

    pub fn select_file_next(&mut self, cx: &mut Context<Self>) {
        let len = self.files.as_ref().map_or(0, |f| f.len());
        if len == 0 {
            return;
        }
        let next = match self.selected_file {
            None => 0,
            Some(s) if s + 1 >= len => return, // boundary: no-op
            Some(s) => s + 1,
        };
        self.select_file(Some(next), cx);
    }

    pub fn select_file_prev(&mut self, cx: &mut Context<Self>) {
        let prev = match self.selected_file {
            Some(0) | None => return, // boundary: no-op
            Some(s) => s - 1,
        };
        self.select_file(Some(prev), cx);
    }

    pub fn toggle_pane(&mut self, cx: &mut Context<Self>) {
        self.pane = match self.pane {
            Pane::Commits => Pane::Files,
            Pane::Files => Pane::Commits,
        };
        cx.notify();
    }

    /// Loads the selected commit's changed files; the file diff is dropped
    /// (a file selection re-issues it).
    pub fn load_commit_files(&mut self, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        // Every (re)issue starts from the loading state: a stale error
        // from a previous transient failure must not mask fresh files.
        self.files_error = None;
        let Some(selected) = self.selected else {
            self.files = None;
            return;
        };
        let Some(commit) = self.commits.get(selected) else {
            self.files = None;
            return;
        };
        let gen = self.detail_generation;
        let worktree = self.worktree.clone();
        let sha = commit.hash.clone();
        let is_merge = commit.parents.len() > 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::commit_files(&worktree, &sha, is_merge) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.detail_generation {
                    return;
                }
                match result {
                    Ok(files) => {
                        store.files = Some(files);
                        // Pre-select the first file and load its diff, so
                        // the detail pane is never empty on arrival.
                        store.selected_file = if store.files.as_ref().is_some_and(|f| !f.is_empty())
                        {
                            Some(0)
                        } else {
                            None
                        };
                        store.load_file_diff(cx);
                    }
                    Err(e) => {
                        let text = if e.is_lock_error() {
                            "another git process may be using this worktree — retry".to_string()
                        } else {
                            e.message
                        };
                        store.files = None;
                        store.files_error = Some(text.clone());
                        store.message = Some(text);
                        store.busy_hint = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the selected file's unified diff.
    pub fn load_file_diff(&mut self, cx: &mut Context<Self>) {
        self.detail_generation += 1;
        let Some(commit) = self.selected.and_then(|i| self.commits.get(i)) else {
            return;
        };
        let Some(file_idx) = self.selected_file else {
            self.file_diff = Err(String::new());
            return;
        };
        let Some(file) = self.files.as_ref().and_then(|f| f.get(file_idx)).cloned() else {
            self.file_diff = Err(String::new());
            return;
        };
        let gen = self.detail_generation;
        let worktree = self.worktree.clone();
        let sha = commit.hash.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::commit_diff(&worktree, &sha, &file.path) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.detail_generation {
                    return;
                }
                store.file_diff = match result {
                    Ok(d) => Ok(d),
                    Err(e) => Err(e.message),
                };
                // Without this, the diff pane keeps the PREVIOUS file's
                // diff until some unrelated keystroke re-renders.
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `y`: copies the selected commit's full hash to the clipboard.
    pub fn copy_hash(&mut self, cx: &mut Context<Self>) {
        let Some(commit) = self.selected.and_then(|i| self.commits.get(i)) else {
            return;
        };
        let short = commit.short.clone();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(commit.hash.clone()));
        self.message = Some(format!("Copied {short} to the clipboard"));
        self.note_transient_hint();
        cx.notify();
    }

    /// `x`: checks the commit out in this worktree (detached). Runs on the
    /// background executor; a success flags a home-list mutation (HEAD and
    /// status change).
    pub fn checkout(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            self.busy_message(cx);
            return;
        }
        let Some(commit) = self.selected.and_then(|i| self.commits.get(i)) else {
            return;
        };
        let sha = commit.hash.clone();
        let short = commit.short.clone();
        let worktree = self.worktree.clone();
        self.message = Some(format!("Checking out {short}…"));
        self.note_transient_hint();
        self.action_in_flight = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::checkout(&worktree, &sha) })
                .await;
            this.update(cx, |store, cx| {
                store.action_in_flight = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("Checked out {short} (detached HEAD)"));
                        store.mutated = true;
                        store.busy_hint = false;
                        // HEAD moved: the list (reachability, decorations)
                        // must reflect the detached state, not just the
                        // home list.
                        store.refresh(cx);
                    }
                    Err(e) => {
                        store.message = Some(e.message);
                        store.busy_hint = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `w`: opens a new worktree at the selected commit.
    pub fn open_worktree(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            self.busy_message(cx);
            return;
        }
        let Some(commit) = self.selected.and_then(|i| self.commits.get(i)) else {
            return;
        };
        let sha = commit.hash.clone();
        let short = commit.short.clone();
        let worktree = self.worktree.clone();
        self.message = Some(format!("Creating worktree at {short}…"));
        self.note_transient_hint();
        self.action_in_flight = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::open_worktree_at(&worktree, &sha, &short) })
                .await;
            this.update(cx, |store, cx| {
                store.action_in_flight = false;
                match result {
                    Ok(path) => {
                        store.message = Some(format!("Worktree created at {}", path.display()));
                        store.mutated = true;
                        store.busy_hint = false;
                    }
                    Err(e) => {
                        store.message = Some(e.message);
                        store.busy_hint = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// An action (checkout / worktree add) is in flight.
    pub fn busy(&self) -> bool {
        self.action_in_flight
    }
}
