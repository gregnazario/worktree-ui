use crate::git;
use crate::model::{self, WorktreeEntry};
use gpui::{App, AppContext, Context, Entity};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct WorktreeStore {
    pub repo_root: Option<PathBuf>,
    pub entries: Vec<WorktreeEntry>,
    /// Indices into `entries` that match the current filter.
    pub filtered: Vec<usize>,
    pub filter: String,
    /// Index into `filtered` (not `entries`).
    pub selected: Option<usize>,
    pub status_message: Option<String>,
    pub busy: bool,
    pub last_refreshed: Option<Instant>,
    /// Loaded after repo detection; "main" until the git call returns.
    pub default_base: String,
    pub local_branches: Vec<String>,
}

/// Pure re-filtering: returns matching indices and the new selection,
/// keeping the previously selected path selected when it still matches.
pub fn apply_filter(
    entries: &[WorktreeEntry],
    filter: &str,
    keep_path: Option<&Path>,
) -> (Vec<usize>, Option<usize>) {
    let indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| model::matches_filter(e, filter))
        .map(|(i, _)| i)
        .collect();
    let selected = keep_path
        .and_then(|p| {
            indices
                .iter()
                .position(|&i| entries[i].path == p)
                .or_else(|| None)
        })
        .or(Some(0).filter(|_| !indices.is_empty()))
        .filter(|_| !indices.is_empty());
    (indices, selected)
}

impl WorktreeStore {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_cx| Self {
            repo_root: None,
            entries: Vec::new(),
            filtered: Vec::new(),
            filter: String::new(),
            selected: None,
            status_message: None,
            busy: false,
            last_refreshed: None,
            default_base: "main".into(),
            local_branches: Vec::new(),
        })
    }

    pub fn selected_entry(&self) -> Option<&WorktreeEntry> {
        self.selected
            .and_then(|s| self.filtered.get(s))
            .and_then(|&i| self.entries.get(i))
    }

    pub fn set_entries(&mut self, entries: Vec<WorktreeEntry>, cx: &mut Context<Self>) {
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.entries = entries;
        let (filtered, selected) = apply_filter(&self.entries, &self.filter, keep.as_deref());
        self.filtered = filtered;
        self.selected = selected;
        cx.notify();
    }

    pub fn set_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.filter = filter;
        let (filtered, selected) = apply_filter(&self.entries, &self.filter, keep.as_deref());
        self.filtered = filtered;
        self.selected = selected;
        cx.notify();
    }

    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected = idx.filter(|&i| i < self.filtered.len());
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let next = self.selected.map(|s| s + 1).unwrap_or(0);
        self.select(Some(next.min(self.filtered.len().saturating_sub(1))), cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        let prev = self.selected.unwrap_or(0).saturating_sub(1);
        self.select(Some(prev), cx);
    }

    /// Repo detected at launch: `start` is the process cwd.
    pub fn detect_repo(&mut self, start: PathBuf, cx: &mut Context<Self>) {
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git::repo_root(&start).await })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(root) => store.load_repo(root, cx),
                    Err(_) => {
                        store.status_message =
                            Some("Not inside a git repository — enter a path below.".into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn load_repo(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.repo_root = Some(root.clone());
        self.filter.clear();
        self.status_message = None;
        cx.notify();
        self.load_branch_metadata(root.clone(), cx);
        self.refresh(cx);
    }

    /// Validates a user-entered path and loads it as the active repository.
    pub fn load_repo_from_user_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.busy = true;
        self.status_message = Some("Opening repository…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git::repo_root(&path).await })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(root) => store.load_repo(root, cx),
                    Err(e) => {
                        store.status_message = Some(format!("Not a git repository: {}", e.message));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_branch_metadata(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (default_base, branches) = cx
                .background_executor()
                .spawn(async move {
                    let default = git::default_branch(&root).await;
                    let branches = git::local_branches(&root).await.unwrap_or_default();
                    (default, branches)
                })
                .await;
            this.update(cx, |store, cx| {
                store.default_base = default_base;
                store.local_branches = branches;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root.clone() else {
            return;
        };
        self.busy = true;
        self.status_message = Some("Refreshing…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let entries = git::list_worktrees(&root).await?;
                    Ok::<_, git::GitError>(git::status_pass(entries).await)
                })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(entries) => {
                        store.last_refreshed = Some(Instant::now());
                        store.status_message = None;
                        store.set_entries(entries, cx);
                    }
                    Err(e) => store.status_message = Some(e.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn add(&mut self, path: PathBuf, branch: Option<String>, base: String, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root.clone() else {
            return;
        };
        self.busy = true;
        self.status_message = Some("Creating worktree…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = path.display().to_string();
            let result = cx
                .background_executor()
                .spawn(async move {
                    git::add_worktree(&root, &path, branch.as_deref(), &base).await
                })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.status_message = Some(format!("Created {created}"));
                    }
                    Err(e) => store.status_message = Some(e.message),
                }
                store.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn remove(&mut self, path: PathBuf, force: bool, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root.clone() else {
            return;
        };
        self.busy = true;
        self.status_message = Some("Removing worktree…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let removed = path.display().to_string();
            let result = cx
                .background_executor()
                .spawn(async move { git::remove_worktree(&root, &path, force).await })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => store.status_message = Some(format!("Removed {removed}")),
                    Err(e) => store.status_message = Some(e.message),
                }
                store.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn prune(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root.clone() else {
            return;
        };
        self.busy = true;
        self.status_message = Some("Pruning…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { git::prune(&root).await })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => store.status_message = Some("Pruned stale entries".into()),
                    Err(e) => store.status_message = Some(e.message),
                }
                store.refresh(cx);
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorktreeStatus;

    fn entry(path: &str, branch: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: path.into(),
            head: None,
            branch: Some(branch.into()),
            is_main: false,
            status: WorktreeStatus::Pending,
        }
    }

    #[test]
    fn empty_filter_selects_first_row() {
        let entries = vec![entry("/a", "main"), entry("/b", "feat")];
        let (idx, sel) = apply_filter(&entries, "", None);
        assert_eq!(idx, vec![0, 1]);
        assert_eq!(sel, Some(0));
    }

    #[test]
    fn filter_narrows_and_keeps_selection_by_path() {
        let entries = vec![entry("/a", "main"), entry("/b/feat-x", "feat-x")];
        let (idx, sel) = apply_filter(&entries, "feat", Some(Path::new("/b/feat-x")));
        assert_eq!(idx, vec![1]);
        assert_eq!(sel, Some(0)); // position within filtered
    }

    #[test]
    fn selection_resets_when_kept_path_filtered_out() {
        let entries = vec![entry("/a", "main"), entry("/b", "feat")];
        let (idx, sel) = apply_filter(&entries, "main", Some(Path::new("/b")));
        assert_eq!(idx, vec![0]);
        assert_eq!(sel, Some(0)); // falls back to first match
    }

    #[test]
    fn empty_result_clears_selection() {
        let entries = vec![entry("/a", "main")];
        let (idx, sel) = apply_filter(&entries, "zzz", None);
        assert!(idx.is_empty());
        assert_eq!(sel, None);
    }
}
