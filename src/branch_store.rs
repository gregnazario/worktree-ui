//! Store for the Branches section of the worktree detail view: branch
//! list, stash entries, and the branch/stash actions. Same async
//! discipline as the other stores.

use crate::engine::{branches, stash};
use gpui::{App, AppContext, Context, Entity};
use std::path::PathBuf;

/// Which list the section's keyboard selection operates on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Branches,
    Stashes,
}

pub struct BranchStore {
    pub worktree: PathBuf,
    pub branches: Vec<branches::BranchInfo>,
    pub selected: Option<usize>,
    pub stashes: Vec<stash::StashEntry>,
    pub selected_stash: Option<usize>,
    pub pane: Pane,
    pub load_failed: bool,
    pub message: Option<String>,
    pub busy_hint: bool,
    /// Mirrored from the app: a working-copy or history operation is
    /// running git on this worktree — switch / merge / rebase / stash
    /// must wait.
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
            selected_stash: None,
            pane: Pane::Branches,
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
            // Both lists in ONE background task: the stash listing must
            // not run on the main thread any more than the branch listing.
            let result = cx
                .background_executor()
                .spawn(async move {
                    let branches = branches::list(&worktree);
                    let stashes = stash::list(&worktree).unwrap_or_default();
                    (branches, stashes)
                })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.load_generation {
                    return;
                }
                // A transient message ("Switched to X") lives until the
                // next completed refresh, then yields to fresh state.
                if store.busy_hint && !store.busy {
                    store.message = None;
                    store.busy_hint = false;
                }
                let (branches, stashes) = result;
                match branches {
                    Ok(branches) => {
                        store.load_failed = false;
                        let current = branches.iter().position(|b| b.is_current).unwrap_or(0);
                        store.branches = branches;
                        // The list may have shrunk (a delete, a pruned
                        // remote): clamp so the highlight and actions
                        // always point at a rendered row.
                        store.selected = Some(
                            store
                                .selected
                                .unwrap_or(current)
                                .min(store.branches.len().saturating_sub(1)),
                        );
                        store.stashes = stashes;
                        store.selected_stash = if store.stashes.is_empty() {
                            None
                        } else {
                            Some(
                                store
                                    .selected_stash
                                    .unwrap_or(0)
                                    .min(store.stashes.len() - 1),
                            )
                        };
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
        match self.pane {
            Pane::Branches => self.selected = idx.filter(|&i| i < self.branches.len()),
            Pane::Stashes => self.selected_stash = idx.filter(|&i| i < self.stashes.len()),
        }
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = match self.pane {
            Pane::Branches => self.branches.len(),
            Pane::Stashes => self.stashes.len(),
        };
        if len == 0 {
            return;
        }
        let current = match self.pane {
            Pane::Branches => self.selected,
            Pane::Stashes => self.selected_stash,
        };
        let next = match current {
            None => 0,
            Some(s) if s + 1 >= len => s,
            Some(s) => s + 1,
        };
        self.select(Some(next), cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        let current = match self.pane {
            Pane::Branches => self.selected,
            Pane::Stashes => self.selected_stash,
        };
        let prev = match current {
            Some(0) | None => 0,
            Some(s) => s - 1,
        };
        self.select(Some(prev), cx);
    }

    /// Moves the keyboard selection between the branch list and the
    /// stash list (each list keeps its own cursor).
    pub fn toggle_pane(&mut self, cx: &mut Context<Self>) {
        self.pane = match self.pane {
            Pane::Branches => Pane::Stashes,
            Pane::Stashes => Pane::Branches,
        };
        match self.pane {
            Pane::Branches if self.selected.is_none() => self.selected = Some(0),
            Pane::Stashes if self.selected_stash.is_none() => self.selected_stash = Some(0),
            _ => {}
        }
        cx.notify();
    }

    pub fn selected_branch(&self) -> Option<&branches::BranchInfo> {
        self.selected.and_then(|i| self.branches.get(i))
    }

    pub fn selected_stash_entry(&self) -> Option<&stash::StashEntry> {
        self.selected_stash.and_then(|i| self.stashes.get(i))
    }

    /// The active pane's cursor position, for scroll reveal.
    pub fn active_selected(&self) -> Option<usize> {
        match self.pane {
            Pane::Branches => self.selected,
            Pane::Stashes => self.selected_stash,
        }
    }

    /// The selected row when it is a LOCAL branch, for actions that
    /// cannot target a remote-tracking ref (switch, delete). The message
    /// explains the refusal instead of failing silently.
    fn selected_local_branch(
        &mut self,
        action: &str,
        cx: &mut Context<Self>,
    ) -> Option<branches::BranchInfo> {
        let branch = self.selected_branch()?.clone();
        if branch.is_remote {
            self.message = Some(format!(
                "{action} needs a local branch — '{}' is remote-tracking",
                branch.short
            ));
            self.note_transient_hint();
            cx.notify();
            return None;
        }
        Some(branch)
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

    /// Shared pre-flight for mutating actions: false = refused (message
    /// already set).
    fn ready_for_action(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy {
            self.busy_message(cx);
            return false;
        }
        if self.wc_mutating {
            self.refuse_while_other_section_busy(cx);
            return false;
        }
        true
    }

    /// Switches to the selected branch. Refuses on dirty working copy.
    pub fn switch(&mut self, cx: &mut Context<Self>) {
        if !self.ready_for_action(cx) {
            return;
        }
        let Some(branch) = self.selected_local_branch("Switch", cx) else {
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

    /// Creates a new branch at the current HEAD (does not switch).
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

    /// Renames the selected local branch.
    pub fn rename_branch(&mut self, new_name: &str, cx: &mut Context<Self>) {
        if !self.ready_for_action(cx) {
            return;
        }
        let Some(branch) = self.selected_local_branch("Rename", cx) else {
            return;
        };
        let old = branch.short.clone();
        let worktree = self.worktree.clone();
        let new_name = new_name.to_string();
        self.busy = true;
        self.message = Some(format!("Renaming {old} to {new_name}…"));
        self.note_transient_hint();
        cx.notify();
        let display_new = new_name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { branches::rename(&worktree, &old, &new_name) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("Renamed to {display_new}"));
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

    /// Deletes the selected branch (refuses current and remote-tracking).
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
        let Some(branch) = self.selected_local_branch("Delete", cx) else {
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
        if !self.ready_for_action(cx) {
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
                        // The engine aborted the merge: the worktree is
                        // back to its pre-merge state, nothing to resolve.
                        store.message = Some(format!(
                            "Merge conflicts in {} — merge aborted, worktree restored",
                            conflicts.join(", ")
                        ));
                        store.busy_hint = true;
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

    /// Rebases the current branch onto the selected branch.
    pub fn rebase_onto(&mut self, cx: &mut Context<Self>) {
        if !self.ready_for_action(cx) {
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
                        // The engine aborted the rebase: nothing wedged.
                        store.message = Some(format!(
                            "Rebase onto {display_name} hit conflicts — rebase aborted, worktree restored"
                        ));
                        store.busy_hint = true;
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

    /// Stashes the working copy's changes (tracked + untracked).
    pub fn stash_push(&mut self, cx: &mut Context<Self>) {
        if !self.ready_for_action(cx) {
            return;
        }
        let worktree = self.worktree.clone();
        self.busy = true;
        self.message = Some("Stashing changes…".into());
        self.note_transient_hint();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { stash::push(&worktree, None) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        // Stashing reverts the working tree: the Working
                        // Copy section must re-read it.
                        store.mutated = true;
                        store.message = Some("Stashed working copy".into());
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

    /// Applies the selected stash and drops it.
    pub fn stash_pop(&mut self, cx: &mut Context<Self>) {
        self.stash_apply_impl(true, cx);
    }

    /// Applies the selected stash, keeping the entry.
    pub fn stash_apply(&mut self, cx: &mut Context<Self>) {
        self.stash_apply_impl(false, cx);
    }

    fn stash_apply_impl(&mut self, drop_after: bool, cx: &mut Context<Self>) {
        if !self.ready_for_action(cx) {
            return;
        }
        let Some(entry) = self.selected_stash_entry().cloned() else {
            self.message = Some("No stash selected".into());
            self.note_transient_hint();
            cx.notify();
            return;
        };
        let worktree = self.worktree.clone();
        let index = entry.index;
        let what = if drop_after { "Popping" } else { "Applying" };
        self.busy = true;
        self.message = Some(format!("{what} stash@{{{index}}}…"));
        self.note_transient_hint();
        cx.notify();
        let display_ref = entry.ref_name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if drop_after {
                        stash::pop_at(&worktree, index)
                    } else {
                        stash::apply_at(&worktree, index)
                    }
                })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.mutated = true;
                        store.message = Some(format!("Applied {display_ref}"));
                        store.refresh(cx);
                    }
                    Err(e) => {
                        // A conflicted apply leaves unmerged paths: they
                        // resolve like any conflict (stage in Working
                        // Copy), the stash entry survives a failed pop.
                        store.message = Some(format!(
                            "{} — resolve in the Working Copy section",
                            e.message
                        ));
                        store.busy_hint = true;
                        store.refresh(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drops the selected stash entry (ref-only; the working copy is
    /// untouched, but the entry is gone for good).
    pub fn stash_drop(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.busy_message(cx);
            return;
        }
        let Some(entry) = self.selected_stash_entry().cloned() else {
            return;
        };
        let worktree = self.worktree.clone();
        let index = entry.index;
        self.busy = true;
        self.message = Some(format!("Dropping stash@{{{index}}}…"));
        self.note_transient_hint();
        cx.notify();
        let display_ref = entry.ref_name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { stash::drop(&worktree, index) })
                .await;
            this.update(cx, |store, cx| {
                store.busy = false;
                match result {
                    Ok(()) => {
                        store.message = Some(format!("Dropped {display_ref}"));
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

    pub fn busy_message(&mut self, cx: &mut Context<Self>) {
        self.message = Some("Busy — wait for the current operation".into());
        self.note_transient_hint();
        cx.notify();
    }

    /// Refusal shown when a working-copy or history operation holds the
    /// worktree: branch and stash mutations touch the same index and
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
