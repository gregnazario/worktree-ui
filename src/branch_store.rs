//! Store for the Branches section of the worktree detail view: branch
//! list, stash entries, and the branch/stash/remote actions. Same async
//! discipline as the other stores.

use crate::engine::{branches, stash};
use gpui::{App, AppContext, Context, Entity};
use std::path::PathBuf;

pub struct BranchStore {
    pub worktree: PathBuf,
    pub branches: Vec<branches::BranchInfo>,
    pub selected: Option<usize>,
    pub stashes: Vec<stash::StashEntry>,
    pub load_failed: bool,
    pub message: Option<String>,
    pub busy_hint: bool,
    /// Mirrored from the app: a working-copy or history operation is
    /// running git on this worktree — switch / merge / rebase must wait.
    pub wc_mutating: bool,
    mutated: bool,
    load_generation: u64,
    pub busy: bool,
}

impl BranchStore {
    pub fn new(worktree: PathBuf, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_cx| Self {
            worktree: worktree.clone(),
            branches: Vec::new(),
            selected: None,
            stashes: Vec::new(),
            load_failed: false,
            message: None,
            busy_hint: false,
            wc_mutating: false,
            mutated: false,
            load_generation: 0,
            busy: false,
        });
        entity.update(cx, |store, cx| store.refresh(cx));
        entity
    }

    pub fn take_mutated(&mut self) -> bool {
        std::mem::take(&mut self.mutated)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let gen = self.load_generation;
        let worktree = self.worktree.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::list(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.load_generation {
                    return;
                }
                match result {
                    Ok(branches) => {
                        store.load_failed = false;
                        let current = branches.iter().position(|b| b.is_current).unwrap_or(0);
                        store.branches = branches;
                        store.selected = Some(store.selected.unwrap_or(current));
                        // Refresh stash list too.
                        let wt = store.worktree.clone();
                        let stashes = stash::list(&wt).unwrap_or_default();
                        store.stashes = stashes;
                    }
                    Err(e) => {
                        store.load_failed = true;
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

    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected = idx.filter(|&i| i < self.branches.len());
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.branches.is_empty() {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(s) if s + 1 >= self.branches.len() => s,
            Some(s) => s + 1,
        };
        self.select(Some(next), cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        let prev = match self.selected {
            Some(0) | None => 0,
            Some(s) => s - 1,
        };
        self.select(Some(prev), cx);
    }

    pub fn selected_branch(&self) -> Option<&branches::BranchInfo> {
        self.selected.and_then(|i| self.branches.get(i))
    }

    pub fn copy_name(&mut self, cx: &mut Context<Self>) {
        let Some(branch) = self.selected_branch() else {
            return;
        };
        let name = branch.ref_name.clone();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(name.clone()));
        self.message = Some(format!("Copied {name}"));
        self.note_transient_hint();
        cx.notify();
    }

    /// Switches to the selected branch. Refuses on dirty working copy.
    pub fn switch(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.busy_message(cx);
            return;
        }
        if self.wc_mutating {
            self.refuse_while_other_section_busy(cx);
            return;
        }
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if branch.is_current {
            self.message = Some("Already on this branch".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let name = branch.short.clone();
        let worktree = self.worktree.clone();
        self.busy = true;
        self.message = Some(format!("Switching to {name}…"));
        self.note_transient_hint();
        cx.notify();
        let display_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::switch(&worktree, &name) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("Switched to {display_name}"));
                        store.mutated = true;
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

    /// Creates a new branch at the current HEAD.
    pub fn create_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.busy {
            self.busy_message(cx);
            return;
        }
        let worktree = self.worktree.clone();
        let name = name.to_string();
        self.busy = true;
        self.message = Some(format!("Creating {name}…"));
        self.note_transient_hint();
        cx.notify();
        let display_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::create(&worktree, &name) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("Created {display_name}"));
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

    /// Deletes the selected branch (refuses current).
    pub fn delete_branch(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.busy_message(cx);
            return;
        }
        let current = self
            .branches
            .iter()
            .find(|b| b.is_current)
            .map(|b| b.short.clone())
            .unwrap_or_default();
        let Some(branch) = self.selected_branch() else {
            return;
        };
        let name = branch.short.clone();
        let worktree = self.worktree.clone();
        self.busy = true;
        self.message = Some(format!("Deleting {name}…"));
        self.note_transient_hint();
        cx.notify();
        let display_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::delete(&worktree, &name, &current) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("Deleted {display_name}"));
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

    /// Merges the selected branch into the current branch.
    pub fn merge(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.busy_message(cx);
            return;
        }
        if self.wc_mutating {
            self.refuse_while_other_section_busy(cx);
            return;
        }
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if branch.is_current {
            self.message = Some("Cannot merge a branch into itself".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let name = branch.short.clone();
        let worktree = self.worktree.clone();
        self.busy = true;
        self.message = Some(format!("Merging {name}…"));
        self.note_transient_hint();
        cx.notify();
        let display_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::merge(&worktree, &name) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(conflicts) if conflicts.is_empty() => {
                        store.message = Some(format!("Merged {display_name}"));
                        store.mutated = true;
                        store.refresh(cx);
                    }
                    Ok(conflicts) => {
                        store.message = Some(format!(
                            "Conflicts in {} — resolve them in the Working Copy section",
                            conflicts.join(", ")
                        ));
                        store.busy_hint = true;
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

    /// Rebases the current branch onto the selected branch.
    pub fn rebase_onto(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.busy_message(cx);
            return;
        }
        if self.wc_mutating {
            self.refuse_while_other_section_busy(cx);
            return;
        }
        let Some(branch) = self.selected_branch() else {
            return;
        };
        if branch.is_current {
            self.message = Some("Cannot rebase onto the current branch".into());
            self.note_transient_hint();
            cx.notify();
            return;
        }
        let name = branch.short.clone();
        let worktree = self.worktree.clone();
        self.busy = true;
        self.message = Some(format!("Rebasing onto {name}…"));
        self.note_transient_hint();
        cx.notify();
        let display_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::rebase(&worktree, &name) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(conflicts) if conflicts.is_empty() => {
                        store.message = Some(format!("Rebased onto {display_name}"));
                        store.mutated = true;
                        store.refresh(cx);
                    }
                    Ok(_) => {
                        store.message = Some(
                            "Rebase conflicts — resolve them in the Working Copy section".into(),
                        );
                        store.busy_hint = true;
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

    pub fn busy_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Busy — wait for the current operation".into());
        self.note_transient_hint();
        cx.notify();
    }

    /// Refusal shown when a working-copy or history operation holds the
    /// worktree: branch switch / merge / rebase touch the same index and
    /// working tree.
    fn refuse_while_other_section_busy(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Busy — another section is changing this worktree".into());
        self.note_transient_hint();
        cx.notify();
    }

    pub fn note_transient_hint(&mut self) {
        self.busy_hint = true;
    }
}
