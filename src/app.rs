use crate::dialogs::{self, DialogState};
use crate::history_store::HistoryStore;
use crate::model::WorktreeEntry;
use crate::model::WorktreeStatus;
use crate::platform;
use crate::store::WorktreeStore;
use crate::terminal;
use crate::text_field::TextField;
use crate::views::{history as history_view, working_copy};
use crate::wc_store::{Pane, WorkingCopyStore};
use gpui::prelude::FluentBuilder;
use gpui::{
    actions, div, px, rgba, App, AppContext, ClipboardItem, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window,
};
use std::path::PathBuf;

actions!(
    worktree_tool,
    [
        NewWorktree,
        Refresh,
        Prune,
        OpenSelected,
        RemoveSelected,
        FocusSearch,
        Quit
    ]
);

/// Const equivalent of `gpui::rgb` (which is not a const fn).
const fn hex_rgb(hex: u32) -> gpui::Rgba {
    gpui::Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

pub const BG: gpui::Rgba = hex_rgb(0x1e1e2e);
pub const PANEL: gpui::Rgba = hex_rgb(0x181825);
pub const ROW_SELECTED: gpui::Rgba = hex_rgb(0x313244);
pub const BORDER: gpui::Rgba = hex_rgb(0x45475a);
pub const TEXT: gpui::Rgba = hex_rgb(0xcdd6f4);
pub const DIM: gpui::Rgba = hex_rgb(0x6c7086);
pub const ACCENT: gpui::Rgba = hex_rgb(0x89b4fa);
pub const GREEN: gpui::Rgba = hex_rgb(0xa6e3a1);
pub const YELLOW: gpui::Rgba = hex_rgb(0xf9e2af);
pub const RED: gpui::Rgba = hex_rgb(0xf38ba8);

/// Test seam: records requested terminal opens instead of spawning a real
/// terminal (tests run on headless CI machines).
#[cfg(test)]
pub(crate) static TERMINAL_REQUESTS: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

pub(crate) fn open_terminal(path: &std::path::Path) {
    #[cfg(test)]
    TERMINAL_REQUESTS.lock().unwrap().push(path.to_path_buf());
    #[cfg(not(test))]
    terminal::open_in_terminal(path);
}

/// Which section of the open detail view is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    WorkingCopy,
    History,
}

pub struct RootView {
    pub store: Entity<WorktreeStore>,
    pub search: Entity<TextField>,
    /// Path input shown in the empty state (no repo detected).
    pub path_input: Entity<TextField>,
    pub dialog: DialogState,
    pub root_focus: FocusHandle,
    pub dialog_focus: FocusHandle,
    /// Open Working Copy drill-in. When set, the detail view replaces the
    /// worktree list as the content branch.
    pub detail: Option<Entity<WorkingCopyStore>>,
    /// Observation of the open detail store. Kept on the view (not
    /// `.detach()`ed) so re-drilling in replaces it instead of accumulating
    /// one subscription per `open_detail`; dropping it unsubscribes.
    pub detail_subscription: Option<gpui::Subscription>,
    pub detail_focus: FocusHandle,
    pub detail_list_focus: FocusHandle,
    pub detail_diff_focus: FocusHandle,
    /// Active section of the open detail view.
    pub section: Section,
    /// History section store (created on first entry, dropped with the
    /// drill-in).
    pub history: Option<Entity<HistoryStore>>,
    /// Observation of the history store: checkout / worktree-add actions
    /// flag `mutated`, which refreshes the home worktree list.
    pub history_subscription: Option<gpui::Subscription>,
    /// Set when working-copy mutations make the history log stale; the
    /// next entry into the History section revalidates it.
    pub history_stale: bool,
    pub history_list_focus: FocusHandle,
    pub history_files_focus: FocusHandle,
    /// Scroll position of the history commit list (virtualized).
    pub history_list_scroll: gpui::UniformListScrollHandle,
    /// Scroll position of the diff pane. Keyboard hunk movement scrolls
    /// the hovered hunk into view through it — without this, `down` on a
    /// tall diff moves the cursor to a hunk that is rendered but scrolled
    /// off-screen, and `s` stages content the user cannot see.
    pub diff_scroll: gpui::ScrollHandle,
    /// Detail revision the diff pane last reacted to. On every new
    /// revision (the store bumps when a load lands) the pane either resets
    /// to the top — when a DIFFERENT diff loaded (file switch, surface
    /// switch) — or reveals the hovered hunk — when the SAME diff
    /// reloaded (the post-mutation case, where the cursor deliberately
    /// waits on the shrunken diff's next hunk).
    pub diff_scroll_generation: u64,
    /// Diff identity ("kind:path") seen at that revision.
    pub diff_scroll_key: Option<String>,
}

fn status_badge(status: &WorktreeStatus) -> (String, gpui::Rgba) {
    let (ahead, behind) = match status {
        WorktreeStatus::Clean { ahead, behind } | WorktreeStatus::Dirty { ahead, behind, .. } => {
            (*ahead, *behind)
        }
        _ => (0, 0),
    };
    let mut arrows = String::new();
    if ahead > 0 {
        arrows.push_str(&format!("↑{ahead} "));
    }
    if behind > 0 {
        arrows.push_str(&format!("↓{behind}"));
    }
    match status {
        WorktreeStatus::Pending => ("…".into(), DIM),
        WorktreeStatus::Unavailable(_) => ("unavailable".into(), RED),
        WorktreeStatus::Clean { .. } => (
            if arrows.is_empty() {
                "clean".into()
            } else {
                arrows
            },
            DIM,
        ),
        WorktreeStatus::Dirty {
            staged,
            unstaged,
            untracked,
            ..
        } => {
            let mut parts = Vec::new();
            if *staged > 0 {
                parts.push(format!("{staged} staged"));
            }
            if *unstaged > 0 {
                parts.push(format!("{unstaged} modified"));
            }
            if *untracked > 0 {
                parts.push(format!("{untracked} untracked"));
            }
            if !arrows.is_empty() {
                parts.push(arrows.trim().to_string());
            }
            (format!("● {}", parts.join(" · ")), YELLOW)
        }
    }
}

pub(crate) fn toolbar_button(
    id: &'static str,
    text: &str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(ROW_SELECTED)
        .text_color(TEXT)
        .text_size(px(13.))
        .child(text.to_string())
        .on_click(on_click)
}

impl RootView {
    pub fn new(store: Entity<WorktreeStore>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new_with_start(store, cwd, window, cx)
    }

    pub fn new_with_start(
        store: Entity<WorktreeStore>,
        start_dir: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let search = cx.new(|cx| TextField::new("Search… (branch or path)", cx));
        let path_input = cx.new(|cx| TextField::new("/path/to/repository", cx));
        let root_focus = cx.focus_handle();
        let dialog_focus = cx.focus_handle();
        let detail_focus = cx.focus_handle();
        let detail_list_focus = cx.focus_handle();
        let detail_diff_focus = cx.focus_handle();
        let history_list_focus = cx.focus_handle();
        let history_files_focus = cx.focus_handle();
        let history_list_scroll = gpui::UniformListScrollHandle::new();
        let diff_scroll = gpui::ScrollHandle::new();
        // Forced mismatch: the first detail land of any drill-in runs the
        // reset branch, so no previous session's scroll offset can leak in.
        let diff_scroll_generation = u64::MAX;
        let diff_scroll_key = None;
        window.focus(&root_focus);
        let view = cx.new(|_| Self {
            store,
            search,
            path_input,
            dialog: DialogState::None,
            root_focus,
            dialog_focus,
            detail: None,
            detail_subscription: None,
            detail_focus,
            detail_list_focus,
            detail_diff_focus,
            section: Section::WorkingCopy,
            history: None,
            history_subscription: None,
            history_stale: false,
            history_list_focus,
            history_files_focus,
            history_list_scroll,
            diff_scroll,
            diff_scroll_generation,
            diff_scroll_key,
        });
        view.update(cx, |this, cx| {
            // Typing in the search field drives the store filter; the
            // observation re-renders the root view on every keystroke.
            let search = this.search.clone();
            let store = this.store.clone();
            cx.observe(&search, move |_, field, cx| {
                let value = field.read(cx).value.clone();
                store.update(cx, |store, cx| store.set_filter(value, cx));
            })
            .detach();
            this.store
                .update(cx, |store, cx| store.detect_repo(start_dir, cx));
        });
        view
    }

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = DialogState::None;
        self.focus_active_surface(window, cx);
        cx.notify();
    }

    /// Refocuses whichever surface is active: the detail list while a
    /// detail view is open, the worktree list otherwise. Every "hand focus
    /// back" path must go through this — `detail_keydown` early-returns
    /// when no detail handle is focused, so refocusing the root while the
    /// detail view is open leaves every detail key dead (keyboard trap).
    fn focus_active_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detail.is_some() {
            // Keep the store's pane state in sync with the focused pane.
            if let Some(wc) = &self.detail {
                wc.update(cx, |store, cx| {
                    store.pane = crate::wc_store::Pane::Files;
                    cx.notify();
                });
            }
            window.focus(&self.detail_list_focus);
        } else {
            window.focus(&self.root_focus);
        }
    }

    pub fn confirm_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let DialogState::Remove { path, force, .. } = &self.dialog {
            let path = path.clone();
            let force = *force;
            self.store
                .update(cx, |store, cx| store.remove(path, force, cx));
        }
        self.close_dialog(window, cx);
    }

    pub fn confirm_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (
            Some(wc),
            DialogState::Discard {
                path, untracked, ..
            },
        ) = (&self.detail, &self.dialog)
        {
            // Discard exactly the path the dialog was opened for — never
            // "the current selection", which a refresh can move while the
            // dialog sits open. `untracked_at_confirm` lets the store
            // refuse the action if the file's state flipped mid-dialog
            // (an external `git rm --cached` could turn a safe
            // restore-from-index into a permanent delete).
            let untracked = *untracked;
            let path = path.clone();
            wc.update(cx, |store, cx| store.discard_path(untracked, path, cx));
        }
        self.close_dialog(window, cx);
    }

    pub fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() || self.store.read(cx).repo_root.is_none() {
            return;
        }
        let default_base = self.store.read(cx).default_base.clone();
        let branch = cx.new(|cx| TextField::new("feature/…", cx));
        let base = cx.new(|cx| {
            let mut field = TextField::new("base", cx);
            field.set_value(&default_base, cx);
            field
        });
        let dest = cx.new(|cx| TextField::new("worktree destination", cx));

        // Live-update the destination from the branch name until the user
        // edits the destination directly. The dest observer can't tell user
        // input from programmatic set_value on its own, so record the last
        // derived value and treat a match as "still automatic".
        cx.observe(&branch.clone(), move |this, field, cx| {
            if let DialogState::Create {
                dest,
                dest_edited,
                last_derived,
                ..
            } = &mut this.dialog
            {
                if !*dest_edited {
                    let branch = field.read(cx).value.trim().to_string();
                    if !branch.is_empty() {
                        if let Some(root) = this.store.read(cx).repo_root.clone() {
                            let path = crate::model::default_worktree_path(&root, &branch);
                            let display = path.display().to_string();
                            *last_derived = display.clone();
                            dest.update(cx, |dest, cx| dest.set_value(&display, cx));
                        }
                    }
                }
            }
            cx.notify();
        })
        .detach();
        cx.observe(&dest.clone(), move |this, field, cx| {
            if let DialogState::Create {
                dest_edited,
                last_derived,
                ..
            } = &mut this.dialog
            {
                if field.read(cx).value != *last_derived {
                    *dest_edited = true;
                }
            }
        })
        .detach();

        let branch_handle = branch.clone();
        self.dialog = DialogState::Create {
            branch,
            base,
            dest,
            new_branch: true,
            dest_edited: false,
            last_derived: String::new(),
        };
        // Start typing the branch name immediately.
        let handle = branch_handle.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    fn open_remove_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        let Some(entry) = self.store.read(cx).selected_entry().cloned() else {
            return;
        };
        // Only genuinely dirty worktrees get the warning; Pending (status
        // pass unfinished) and Unavailable (directory gone) are not "you
        // have uncommitted changes".
        let dirty = matches!(entry.status, WorktreeStatus::Dirty { .. });
        let branch_label = entry
            .branch
            .clone()
            .unwrap_or_else(|| entry.head.clone().unwrap_or_else(|| "?".into()));
        self.dialog = DialogState::Remove {
            path: entry.path,
            branch_label: SharedString::from(branch_label),
            dirty,
            force: false,
        };
        window.focus(&self.dialog_focus);
        cx.notify();
    }

    /// Only Unstaged/Untracked file rows can be discarded (staged changes
    /// unstage first; conflicts and directories are not offered in Phase 1).
    fn open_discard_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        let Some(wc) = &self.detail else { return };
        // First snapshot still loading: there is no row to describe yet.
        if wc.read(cx).wc.is_none() {
            // A FAILED first load keeps its error visible (already rendered
            // in the list pane) — don't overwrite it with a loading hint.
            if !wc.read(cx).load_failed {
                wc.update(cx, |store, cx| store.loading_message(cx));
            }
            return;
        }
        // Block only a running MUTATION (its state is about to change).
        // Snapshot refreshes are deliberately not covered — a refresh can
        // complete while the dialog is open, and that's fine:
        // `discard_path` derives the executed action from the file's live
        // state when the confirm lands.
        if wc.read(cx).mutating {
            wc.update(cx, |store, cx| store.busy_message(cx));
            return;
        }
        let Some((group, entry)) = wc.read(cx).selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        // Ineligible rows get a footer hint instead of a silent no-op: the
        // footer advertises `d discard`, so an unexplained dead key reads
        // as the app ignoring the keystroke.
        let hint = match group {
            crate::engine::working_copy::Group::Staged => Some(
                "staged changes — press s to unstage first, then discard if needed".to_string(),
            ),
            crate::engine::working_copy::Group::Conflicts => {
                Some("resolve conflicts in your editor, then press s to mark resolved".to_string())
            }
            _ if entry.is_dir() => Some(
                "directories can't be discarded — stage with S, then remove the path in a terminal"
                    .to_string(),
            ),
            _ if entry.unsupported => {
                Some("non-UTF-8 name — discard this one in a terminal".to_string())
            }
            crate::engine::working_copy::Group::Unstaged
            | crate::engine::working_copy::Group::Untracked => None,
        };
        if let Some(hint) = hint {
            wc.update(cx, |store, cx| {
                store.message = Some(hint);
                // Marked as a transient hint so a landing refresh clears it.
                store.note_transient_hint();
                cx.notify();
            });
            return;
        }
        self.dialog = DialogState::Discard {
            path: entry.path.clone(),
            untracked: entry.untracked,
        };
        window.focus(&self.dialog_focus);
        cx.notify();
    }

    fn open_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        let terminals = terminal::detect_installed();
        let selected = terminal::load_settings().terminal;
        self.dialog = DialogState::Settings {
            terminals,
            selected,
            saved_to: None,
        };
        window.focus(&self.dialog_focus);
        cx.notify();
    }

    fn search_focused(&self, window: &Window, cx: &App) -> bool {
        self.search.read(cx).focus_handle.is_focused(window)
    }

    /// Drills into the selected worktree's Working Copy view. Focus moves to
    /// the detail list handle; keys are routed through `detail_keydown` until
    /// `close_detail`.
    pub fn open_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        let Some(entry) = self.store.read(cx).selected_entry().cloned() else {
            return;
        };
        self.section = Section::WorkingCopy;
        self.history = None;
        self.history_subscription = None;
        self.history_stale = false;
        let wc = WorkingCopyStore::new(entry.path.clone(), cx);
        // One successful mutation inside the detail view must refresh the home
        // worktree list (status, ahead/behind, dirty badge all change). The
        // subscription is stored on the view, not detached: a previous drill-in's
        // observer would otherwise accumulate (dropped stores make old observers
        // inert but never remove their subscription entries).
        self.detail_subscription = Some(cx.observe(&wc, move |this, wc, cx| {
            // Mirror the mutation state FIRST (success or failure): an
            // already-open History section must release its `wc_mutating`
            // refusal as soon as the working-copy operation ends, not
            // only when the user re-enters the section.
            let mutating = wc.read(cx).mutating;
            if let Some(hs) = &this.history {
                hs.update(cx, |store, _cx| store.wc_mutating = mutating);
            }
            if wc.update(cx, |store, _cx| store.take_mutated()) {
                this.store.update(cx, |store, cx| store.refresh(cx));
                // Only a COMMIT changes reachable history (stage/unstage/
                // discard move things between index and worktree): only
                // that makes the History section's log stale.
                if wc.update(cx, |store, _cx| store.take_history_changed()) {
                    // If History is already on screen, refresh the log
                    // NOW instead of leaving it stale until re-entry.
                    if this.section == Section::History {
                        if let Some(hs) = &this.history {
                            let busy = hs.read(cx).busy();
                            if !busy {
                                hs.update(cx, |h, cx| h.refresh(cx));
                            } else {
                                this.history_stale = true;
                            }
                        }
                    } else {
                        this.history_stale = true;
                    }
                }
            }
            cx.notify();
        }));
        self.detail = Some(wc);
        window.focus(&self.detail_list_focus);
        cx.notify();
    }

    /// Returns to the home list: refocus it and refresh, since the user may
    /// have mutated the worktree from the detail view.
    pub fn close_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An in-flight history action (checkout / worktree add) would
        // lose its completion when the store drops — the home list would
        // go stale. Keep the drill-in until it lands.
        if let Some(hs) = &self.history {
            if hs.read(cx).busy() {
                // Surface the blockage in the VISIBLE section: pressing
                // esc from the Working Copy section would otherwise show
                // nothing (the History view isn't rendered there). Name
                // the actual in-flight action.
                let what = hs.read(cx).action_name().unwrap_or("history action");
                let msg = format!("Busy — {what} is finishing in this worktree");
                if self.section == Section::WorkingCopy {
                    if let Some(wc) = &self.detail {
                        wc.update(cx, |store, cx| {
                            store.message = Some(msg);
                            store.note_transient_hint();
                            cx.notify();
                        });
                    }
                } else {
                    hs.update(cx, |store, cx| {
                        store.message = Some(msg);
                        store.note_transient_hint();
                        cx.notify();
                    });
                }
                return;
            }
        }
        // A busy detail view means an operation is in flight — possibly the
        // commit editor, which can run for minutes. Dropping the store now
        // would orphan it: re-drilling opens a fresh, idle store while the
        // old commit is still pending, re-opening the mutate-under-pending-
        // commit hole the busy-gating exists to close. The editor session
        // has an escape hatch instead of a bare refusal: esc kills the
        // editor child and unwinds the pending commit, so a wedged or
        // forgotten editor can never keyboard-lock the view until quit.
        let busy = self.detail.as_ref().is_some_and(|wc| wc.read(cx).mutating);
        if busy {
            // A commit editor waits on the USER (an editor that can run
            // for minutes) — esc abandons it regardless of which section
            // is showing.
            let editor_active = self
                .detail
                .as_ref()
                .is_some_and(|wc| wc.read(cx).commit_editor_active());
            if editor_active {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.abandon_commit(cx));
                }
                return;
            }
            // Write the hint to the VISIBLE section's store: a user
            // sitting in History would otherwise see nothing.
            if self.section == Section::History {
                if let Some(hs) = &self.history {
                    hs.update(cx, |store, cx| store.busy_message(cx));
                }
            } else if let Some(wc) = &self.detail {
                wc.update(cx, |store, cx| store.busy_message(cx));
            }
            return;
        }
        // Drop the observer first: a dropped Subscription unsubscribes.
        self.detail_subscription = None;
        self.history = None;
        self.history_subscription = None;
        self.section = Section::WorkingCopy;
        // A fresh handle: the next drill-in's History opens at the top
        // instead of inheriting this session's scroll offset.
        self.history_list_scroll = gpui::UniformListScrollHandle::new();
        self.history_stale = false;
        self.detail = None;
        // Re-arm the diff-pane scroll bookkeeping: each store's detail
        // generation restarts at 0, so a later drill-in could otherwise
        // collide with the generation cached here and skip the reveal,
        // leaking this session's scroll offset into the next diff.
        self.diff_scroll_generation = u64::MAX;
        self.diff_scroll_key = None;
        window.focus(&self.root_focus);
        self.store.update(cx, |store, cx| store.refresh(cx));
        cx.notify();
    }

    fn detail_keydown(
        &mut self,
        ks: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Route by section FIRST: the History section's focused handles
        // (history_list/history_files) are not the working-copy handles
        // checked below, and its container is a different subtree.
        if self.section == Section::History {
            return self.history_keydown(ks, window, cx);
        }
        // A history action (checkout / worktree add) in flight races the
        // Working Copy keys in the same worktree (index.lock contention,
        // staging post-checkout content): refuse with an explanation.
        if let Some(hs) = &self.history {
            if hs.read(cx).busy() {
                // Navigation/terminal/section-switch stay live (none run a
                // git command); only worktree-MUTATING keys (s / discard /
                // commit / r) are refused while the action finishes.
                match ks.key.as_str() {
                    "t" => {
                        if let Some(wc) = &self.detail {
                            let path = wc.read(cx).worktree.clone();
                            open_terminal(&path);
                        }
                        return;
                    }
                    "2" => {
                        self.open_history(window, cx);
                        return;
                    }
                    "up" | "down" | "tab" | "escape" | "n" | "1" => {
                        // Pure UI navigation: handle in the normal router
                        // (which doesn't run git commands for these keys).
                    }
                    _ => {
                        // Everything else this section binds mutates the
                        // worktree (s/S/d/c) or re-runs git (r): refuse.
                        if let Some(wc) = &self.detail {
                            wc.update(cx, |store, cx| {
                                store.message = Some(
                                    "Busy — a history action is finishing in this worktree".into(),
                                );
                                store.note_transient_hint();
                                cx.notify();
                            });
                        }
                        return;
                    }
                }
            }
        }
        let list_focused = self.detail_list_focus.is_focused(window);
        let diff_focused = self.detail_diff_focus.is_focused(window);
        let container_focused = self.detail_focus.is_focused(window);
        if !list_focused && !diff_focused && !container_focused {
            return; // don't steal keys from other focused surfaces
        }
        match ks.key.as_str() {
            "escape" => self.close_detail(window, cx),
            "t" => {
                if let Some(wc) = &self.detail {
                    let path = wc.read(cx).worktree.clone();
                    open_terminal(&path);
                }
            }
            "r" => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.refresh(cx));
                }
            }
            "tab" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| {
                        store.pane = Pane::Diff;
                        cx.notify();
                    });
                }
                window.focus(&self.detail_diff_focus);
            }
            "tab" if diff_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| {
                        store.pane = Pane::Files;
                        cx.notify();
                    });
                }
                window.focus(&self.detail_list_focus);
            }
            "up" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.select_prev(cx));
                }
            }
            "down" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.select_next(cx));
                }
            }
            // gpui normalizes an uppercase keystroke to lowercase key +
            // shift modifier (platform/keystroke.rs), so stage-all must
            // match shift+s — a literal "S" key never occurs.
            "s" if list_focused && ks.modifiers.shift => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.stage_all(cx));
                }
            }
            "s" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.toggle_stage(cx));
                }
            }
            "d" if list_focused => self.open_discard_dialog(window, cx),
            // The toolbar advertises "New (n)" on every surface.
            "n" => self.open_create_dialog(window, cx),
            "c" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.commit_with_editor(cx));
                }
            }
            // ---- diff pane (hunk staging, Phase 1b) ----
            "up" if diff_focused => {
                if let Some(wc) = &self.detail {
                    let hovered = wc.update(cx, |store, cx| {
                        let moved = store.hunk_prev(cx);
                        (moved, store.hunk_cursor())
                    });
                    // Scroll only on actual movement: a no-op key (staged
                    // row, loading diff, at the bound) must not jump the
                    // pane to another hunk.
                    if let (true, hovered) = hovered {
                        // +1: the file-header summary is the pane's child 0.
                        self.diff_scroll.scroll_to_item(hovered + 1);
                    }
                }
            }
            "down" if diff_focused => {
                if let Some(wc) = &self.detail {
                    let (moved, hovered) = wc.update(cx, |store, cx| {
                        let moved = store.hunk_next(cx);
                        (moved, store.hunk_cursor())
                    });
                    if moved {
                        self.diff_scroll.scroll_to_item(hovered + 1);
                    }
                }
            }
            // Same capital-normalization as the list pane: shift+s must
            // stay "stage all" with the diff pane focused, or a user's
            // muscle memory silently becomes a one-hunk index mutation.
            "s" if diff_focused && ks.modifiers.shift => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.stage_all(cx));
                }
            }
            "s" if diff_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.stage_hunk(cx));
                }
            }
            // Section switching: 2 opens History (1 is a no-op here).
            // open_history is idempotent for an existing store and
            // already focuses the remembered pane.
            "2" => self.open_history(window, cx),
            _ => {}
        }
    }

    /// Key routing for the History section (see `open_history`).
    fn history_keydown(
        &mut self,
        ks: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_open() {
            return; // dialogs handle their own keys (same as detail_keydown)
        }
        let Some(hs) = self.history.clone() else {
            return;
        };
        let list_focused = self.history_list_focus.is_focused(window);
        let files_focused = self.history_files_focus.is_focused(window);
        let container_focused = self.detail_focus.is_focused(window);
        if !list_focused && !files_focused && !container_focused {
            return;
        }
        // Keys that work on every focused surface in this section.
        match ks.key.as_str() {
            "escape" => return self.close_detail(window, cx),
            "1" => {
                self.section = Section::WorkingCopy;
                // Restore the Working Copy section's remembered pane focus.
                if let Some(wc) = &self.detail {
                    let pane = wc.read(cx).pane;
                    if pane == crate::wc_store::Pane::Diff {
                        window.focus(&self.detail_diff_focus);
                    } else {
                        window.focus(&self.detail_list_focus);
                    }
                } else {
                    window.focus(&self.detail_list_focus);
                }
                cx.notify();
                return;
            }
            "t" => {
                let path = hs.read(cx).worktree.clone();
                open_terminal(&path);
                return;
            }
            _ => {}
        }
        match ks.key.as_str() {
            // Re-entry retries a pending revalidation skipped while an
            // action was in flight (open_history is idempotent here).
            "2" => self.open_history(window, cx),
            // r retries failed loads and reloads empty repos, so it must
            // NOT be gated by action_blocker (whose failed-load message
            // says "press r to retry" — that would block the very key it
            // advertises). Gate only on busy/retrying: a mid-flight
            // checkout must not spawn a redundant concurrent log.
            "r" => hs.update(cx, |h, cx| {
                if h.busy() || h.retrying {
                    h.busy_message(cx);
                    cx.notify();
                } else {
                    h.refresh(cx);
                }
            }),
            "up" if list_focused => {
                // First load still in flight (or failed): explain instead
                // of silently no-oping.
                if hs.read(cx).commits.is_empty() {
                    if let Some(blocked) = hs.read(cx).action_blocker() {
                        hs.update(cx, |h, cx| {
                            h.message = Some(blocked);
                            h.note_transient_hint();
                            cx.notify();
                        });
                    }
                    return;
                }
                let pos = hs.update(cx, |h, cx| {
                    h.select_prev(cx);
                    h.selected
                });
                // Keep the selected commit on screen as the cursor moves.
                if let Some(pos) = pos {
                    self.history_list_scroll
                        .scroll_to_item(pos, gpui::ScrollStrategy::Center);
                }
            }
            "down" if list_focused => {
                if hs.read(cx).commits.is_empty() {
                    if let Some(blocked) = hs.read(cx).action_blocker() {
                        hs.update(cx, |h, cx| {
                            h.message = Some(blocked);
                            h.note_transient_hint();
                            cx.notify();
                        });
                    }
                    return;
                }
                let pos = hs.update(cx, |h, cx| {
                    h.select_next(cx);
                    h.selected
                });
                if let Some(pos) = pos {
                    self.history_list_scroll
                        .scroll_to_item(pos, gpui::ScrollStrategy::Center);
                }
            }
            "up" if files_focused => hs.update(cx, |h, cx| h.select_file_prev(cx)),
            "down" if files_focused => hs.update(cx, |h, cx| h.select_file_next(cx)),
            "tab" if list_focused => {
                hs.update(cx, |h, cx| h.toggle_pane(cx));
                window.focus(&self.history_files_focus);
            }
            "tab" if files_focused => {
                hs.update(cx, |h, cx| h.toggle_pane(cx));
                window.focus(&self.history_list_focus);
            }
            // Actions explain themselves when swallowed: busy (an action
            // in flight), still loading, a failed first load, or an empty
            // repo — each state gets its own accurate message.
            // Action keys work from any history-section surface; when the
            // preconditions aren't met, the blocker explains why instead
            // of silently dropping the key.
            "y" => hs.update(cx, |h, cx| {
                if let Some(blocked) = h.action_blocker() {
                    h.message = Some(blocked);
                    h.note_transient_hint();
                } else {
                    h.copy_hash(cx);
                }
                cx.notify();
            }),
            "x" => hs.update(cx, |h, cx| {
                if let Some(blocked) = h.action_blocker() {
                    h.message = Some(blocked);
                    h.note_transient_hint();
                } else {
                    h.checkout(cx);
                }
                cx.notify();
            }),
            "w" => hs.update(cx, |h, cx| {
                if let Some(blocked) = h.action_blocker() {
                    h.message = Some(blocked);
                    h.note_transient_hint();
                } else {
                    h.open_worktree(cx);
                }
                cx.notify();
            }),
            // gpui normalizes capitals to lowercase + shift.
            "l" if ks.modifiers.shift => hs.update(cx, |h, cx| {
                if let Some(blocked) = h.action_blocker() {
                    h.message = Some(blocked);
                    h.note_transient_hint();
                } else if !h.has_more {
                    h.message = Some("No older commits to load".into());
                    h.note_transient_hint();
                } else {
                    h.load_more(cx);
                }
                cx.notify();
            }),
            _ => {}
        }
    }

    /// Opens (or re-focuses) the History section of the open detail view.
    pub fn open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        self.section = Section::History;
        if let Some(hs) = &self.history {
            // Revalidate only when the working copy mutated since the last
            // visit — an unconditional refetch on every tab press costs a
            // full-depth log run for nothing.
            // Clear the flag only when the refresh actually runs; while
            // an action is in flight the pending revalidation must stay
            // pending, not vanish.
            if self.history_stale && !hs.read(cx).busy() {
                self.history_stale = false;
                hs.update(cx, |h, cx| h.refresh(cx));
            }
            // Restore the section's remembered pane focus.
            if hs.read(cx).pane == crate::history_store::Pane::Files {
                window.focus(&self.history_files_focus);
            } else {
                window.focus(&self.history_list_focus);
            }
            // Mirror the Working Copy store's mutation state: history
            // actions must not race an in-flight stage/discard/commit.
            let wc_mutating = self.detail.as_ref().is_some_and(|wc| wc.read(cx).mutating);
            hs.update(cx, |store, _cx| store.wc_mutating = wc_mutating);
            cx.notify();
            return;
        }
        let Some(entry) = self.store.read(cx).selected_entry().cloned() else {
            self.section = Section::WorkingCopy;
            return;
        };
        let hs = HistoryStore::new(entry.path.clone(), cx);
        self.history_subscription = Some(cx.observe(&hs, move |this, hs, cx| {
            // Mirror the history action state into the wc store BOTH ways:
            // its mutating entry points must refuse while a checkout/
            // worktree-add runs, regardless of which entry point (keys,
            // mouse, future callers) launched them.
            let busy = hs.read(cx).busy();
            if let Some(wc) = &this.detail {
                wc.update(cx, |store, _cx| store.history_busy = busy);
            }
            let mutated = hs.update(cx, |store, _cx| store.take_mutated());
            let files_changed = hs.update(cx, |store, _cx| store.take_worktree_files_changed());
            if mutated {
                this.store.update(cx, |store, cx| store.refresh(cx));
                // Only a CHECKOUT rewrites this worktree's files (worktree
                // add touches a different directory) — refresh section 1
                // selectively so its in-progress state isn't churned.
                if files_changed {
                    if let Some(wc) = &this.detail {
                        wc.update(cx, |store, cx| store.refresh(cx));
                    }
                    // Checkout's completion re-ran the log: the flag is
                    // satisfied. A worktree add did NOT re-run it, so a
                    // pending revalidation must stay pending.
                    this.history_stale = false;
                }
            }
            cx.notify();
        }));
        // The fresh store just loaded the current log: a stale flag set
        // by an earlier working-copy mutation no longer applies.
        self.history_stale = false;
        // Mirror the Working Copy store's mutation state for the new
        // store too (the re-entry path syncs it for existing stores).
        let wc_mutating = self.detail.as_ref().is_some_and(|wc| wc.read(cx).mutating);
        hs.update(cx, |store, _cx| store.wc_mutating = wc_mutating);
        self.history = Some(hs);
        window.focus(&self.history_list_focus);
        cx.notify();
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let repo_root = store.repo_root.clone();
        let repo_name = repo_root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let repo_path = repo_root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let busy = store.busy;
        let status_message = store.status_message.clone();
        let last_refreshed = store.last_refreshed;
        let rows: Vec<(usize, WorktreeEntry)> = store
            .filtered
            .iter()
            .enumerate()
            .map(|(pos, &i)| (pos, store.entries[i].clone()))
            .collect();
        let selected = store.selected;
        let selected_entry = store.selected_entry().cloned();
        let dialog_open = self.dialog.is_open();

        let root_keydown = cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if this.dialog.is_open() {
                return; // dialogs handle their own keys
            }
            let ks = &event.keystroke;
            if this.search_focused(window, cx) {
                if ks.key == "escape" {
                    this.search.update(cx, |field, cx| field.set_value("", cx));
                    this.store
                        .update(cx, |store, cx| store.set_filter(String::new(), cx));
                    this.focus_active_surface(window, cx);
                } else if ks.key == "enter" {
                    this.focus_active_surface(window, cx);
                }
                return;
            }
            if ks.modifiers.control || ks.modifiers.platform || ks.modifiers.alt {
                return;
            }
            if this.detail.is_some() {
                this.detail_keydown(ks, window, cx);
                return;
            }
            // Home list: only act when the list itself is focused;
            // otherwise we'd steal typing from any other focused text field
            // (e.g. the empty-state path input, where "/" is unavoidable).
            if !this.root_focus.is_focused(window) {
                return;
            }
            match ks.key.as_str() {
                "up" => this.store.update(cx, |store, cx| store.select_prev(cx)),
                "down" => this.store.update(cx, |store, cx| store.select_next(cx)),
                "enter" => this.open_detail(window, cx),
                "t" => {
                    if let Some(entry) = this.store.read(cx).selected_entry() {
                        let path = entry.path.clone();
                        open_terminal(&path);
                    }
                }
                "backspace" | "delete" => this.open_remove_dialog(window, cx),
                "/" => {
                    let handle = this.search.read(cx).focus_handle.clone();
                    window.focus(&handle);
                }
                "n" => this.open_create_dialog(window, cx),
                "r" => this.store.update(cx, |store, cx| store.refresh(cx)),
                _ => {}
            }
        });

        let mut root = div()
            .id("root")
            .key_context("Root")
            .track_focus(&self.root_focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(TEXT)
            .on_action(cx.listener(|this, _: &NewWorktree, window, cx| {
                if !this.dialog.is_open() {
                    this.open_create_dialog(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Refresh, _window, cx| {
                // Context-aware, matching the `r` key and the toolbar
                // button: refresh whatever the user is looking at.
                if let Some(wc) = &this.detail {
                    wc.update(cx, |store, cx| store.refresh(cx));
                } else {
                    this.store.update(cx, |store, cx| store.refresh(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Prune, _window, cx| {
                this.store.update(cx, |store, cx| store.prune(cx));
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, _window, cx| {
                if let Some(entry) = this.store.read(cx).selected_entry() {
                    let path = entry.path.clone();
                    open_terminal(&path);
                }
            }))
            .on_action(cx.listener(|this, _: &RemoveSelected, window, cx| {
                if !this.dialog.is_open() {
                    this.open_remove_dialog(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                // The search field filters the worktree list — focusing it
                // while the detail view is open (or mid-dialog, or in the
                // empty state where it isn't rendered) is a keyboard trap.
                if this.dialog.is_open()
                    || this.detail.is_some()
                    || this.store.read(cx).repo_root.is_none()
                {
                    return;
                }
                let handle = this.search.read(cx).focus_handle.clone();
                window.focus(&handle);
            }))
            .on_key_down(root_keydown);

        let content = if repo_root.is_none() {
            let path_input = self.path_input.clone();
            let store = self.store.clone();
            div()
                .id("empty-state")
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_size(px(16.)).child("Open a git repository"))
                .child(
                    div()
                        .id("empty-state-input")
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.path_input.clone())
                        .child(toolbar_button("load-repo", "Load", move |_, _, cx| {
                            let value = path_input.read(cx).value.trim().to_string();
                            if !value.is_empty() {
                                let path = crate::model::expand_tilde(&value);
                                store.update(cx, |store, cx| {
                                    store.load_repo_from_user_path(path, cx)
                                });
                            }
                        })),
                )
        } else {
            let main = div().id("main").flex().flex_col().flex_1().min_h_0().child(
                div()
                    .id("toolbar")
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(repo_name),
                            )
                            .child(div().text_size(px(11.)).text_color(DIM).child(repo_path)),
                    )
                    .child(div().flex_1())
                    // The search field filters the worktree list — hidden
                    // while a detail view is open, so it can't invisibly
                    // filter a list the user can't see.
                    .when(self.detail.is_none(), |toolbar| {
                        toolbar.child(self.search.clone())
                    })
                    .child(toolbar_button(
                        "btn-new",
                        "New (n)",
                        cx.listener(|this, _, window, cx| this.open_create_dialog(window, cx)),
                    ))
                    .child(toolbar_button(
                        "btn-refresh",
                        "Refresh (r)",
                        cx.listener(|this, _, _window, cx| {
                            // Context-aware, matching the `r` key: refresh
                            // whatever the user is actually looking at.
                            if let Some(wc) = &this.detail {
                                wc.update(cx, |store, cx| store.refresh(cx));
                            } else {
                                this.store.update(cx, |store, cx| store.refresh(cx));
                            }
                        }),
                    ))
                    .child(toolbar_button(
                        "btn-prune",
                        "Prune",
                        cx.listener(|this, _, _window, cx| {
                            this.store.update(cx, |store, cx| store.prune(cx))
                        }),
                    ))
                    .child(toolbar_button(
                        "btn-settings",
                        "Settings",
                        cx.listener(|this, _, window, cx| this.open_settings_dialog(window, cx)),
                    )),
            );

            if self.detail.is_some() {
                let section = match self.section {
                    Section::WorkingCopy => {
                        working_copy::render(self, window, cx).into_any_element()
                    }
                    Section::History => history_view::render(self, window, cx).into_any_element(),
                };
                main.child(section)
            } else {
                main.child(
                    div()
                        .id("worktree-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(rows.iter().map(|(pos, entry)| {
                            let pos = *pos;
                            let is_selected = selected == Some(pos);
                            let (badge, badge_color) = status_badge(&entry.status);
                            let branch = entry.branch.clone().unwrap_or_else(|| {
                                format!("({})", entry.head.clone().unwrap_or_default())
                            });
                            let path = entry.path.display().to_string();
                            let kind = if entry.is_main { "main" } else { "linked" };
                            let row_id =
                                SharedString::from(format!("wt-row-{}", entry.path.display()));
                            div()
                                .id(row_id)
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(ROW_SELECTED)
                                .when(is_selected, |row| row.bg(ROW_SELECTED))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .flex()
                                                .items_baseline()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                                        .child(branch),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .text_color(ACCENT)
                                                        .child(kind),
                                                ),
                                        )
                                        .child(
                                            div().text_size(px(11.)).text_color(DIM).child(path),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(badge_color)
                                        .child(badge),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.store
                                            .update(cx, |store, cx| store.select(Some(pos), cx));
                                    }),
                                )
                        })),
                )
                .when(selected_entry.is_some(), |main| {
                    let entry = selected_entry.expect("checked is_some");
                    let branch = entry.branch.clone().unwrap_or_else(|| "detached".into());
                    let path = entry.path.display().to_string();
                    let (badge, _badge_color) = status_badge(&entry.status);
                    let terminal_path = entry.path.clone();
                    let reveal_path = entry.path.clone();
                    let copy_path = entry.path.clone();
                    main.child(
                        div()
                            .id("detail")
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .border_t_1()
                            .border_color(BORDER)
                            .bg(PANEL)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .child(format!("{branch} — {badge}")),
                                    )
                                    .child(div().text_size(px(11.)).text_color(DIM).child(path)),
                            )
                            .child(toolbar_button(
                                "detail-terminal",
                                "Open in Terminal (t)",
                                cx.listener(move |_, _, _window, _cx| {
                                    open_terminal(&terminal_path);
                                }),
                            ))
                            .child(toolbar_button(
                                "detail-reveal",
                                platform::SHOW_IN_FILE_MANAGER_LABEL,
                                cx.listener(move |_, _, _window, _cx| {
                                    platform::reveal_in_file_manager(&reveal_path);
                                }),
                            ))
                            .child(toolbar_button(
                                "detail-copy",
                                "Copy Path",
                                cx.listener(move |_, _, _window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_path.display().to_string(),
                                    ));
                                }),
                            ))
                            .child(toolbar_button(
                                "detail-remove",
                                "Remove…",
                                cx.listener(|this, _, window, cx| {
                                    this.open_remove_dialog(window, cx)
                                }),
                            )),
                    )
                })
            }
        };

        root = root.child(content);

        let status_left = if busy {
            "Working…".to_string()
        } else {
            match last_refreshed {
                Some(t) => format!("refreshed {}s ago", t.elapsed().as_secs()),
                None => "never refreshed".to_string(),
            }
        };
        root = root.child(
            div()
                .id("status-bar")
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(BORDER)
                .bg(PANEL)
                .child(div().text_size(px(11.)).text_color(DIM).child(status_left))
                .child(div().flex_1())
                .when_some(status_message, |bar, msg| {
                    bar.child(
                        div()
                            .text_size(px(11.))
                            .text_color(if busy { DIM } else { YELLOW })
                            .child(msg),
                    )
                }),
        );

        if dialog_open {
            let card = match &self.dialog {
                DialogState::None => None,
                DialogState::Create { .. } => {
                    Some(dialogs::render_create_dialog(self, window, cx).into_any_element())
                }
                DialogState::Remove { .. } => {
                    Some(dialogs::render_remove_dialog(self, window, cx).into_any_element())
                }
                DialogState::Settings { .. } => {
                    Some(dialogs::render_settings_dialog(self, window, cx).into_any_element())
                }
                DialogState::Discard { .. } => {
                    Some(dialogs::render_discard_dialog(self, window, cx).into_any_element())
                }
            };
            if let Some(card) = card {
                root = root.child(
                    div()
                        .id("dialog-overlay")
                        .absolute()
                        .size_full()
                        .top_0()
                        .left_0()
                        .bg(rgba(0x00000080))
                        .flex()
                        .items_center()
                        .justify_center()
                        // Consume clicks so the dialog is truly modal.
                        .on_mouse_down(MouseButton::Left, |_, _, _| {})
                        .child(card),
                );
            }
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wc_store::FileDetail;
    use gpui::TestAppContext;

    fn sh(dir: &std::path::Path, cmd: &[&str]) {
        let status = std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "failed: {cmd:?}");
    }

    fn fixture_repo(dir: &std::path::Path) {
        sh(dir, &["git", "init", "-q", "-b", "main"]);
        sh(dir, &["git", "config", "user.email", "t@t.t"]);
        sh(dir, &["git", "config", "user.name", "t"]);
        sh(dir, &["git", "config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("f.txt"), "one").unwrap();
        sh(dir, &["git", "add", "."]);
        sh(dir, &["git", "commit", "-qm", "init"]);
        sh(
            dir,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                &dir.parent().unwrap().join("feat").display().to_string(),
                "-b",
                "feat",
                "main",
            ],
        );
    }

    fn open_root(
        cx: &mut TestAppContext,
        repo: &std::path::Path,
    ) -> (Entity<RootView>, gpui::VisualTestContext) {
        let view_cell = std::cell::RefCell::new(None);
        cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let store = WorktreeStore::new(cx);
                let view = RootView::new_with_start(store, repo.to_path_buf(), window, cx);
                *view_cell.borrow_mut() = Some(view.clone());
                view
            })
            .unwrap();
        });
        let view = view_cell.into_inner().unwrap();
        cx.run_until_parked();
        let window = cx.windows()[0];
        let vcx = gpui::VisualTestContext::from_window(window, cx);
        (view, vcx)
    }

    fn open_root_no_repo(cx: &mut TestAppContext) -> (Entity<RootView>, gpui::VisualTestContext) {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        open_root(cx, &plain)
    }

    #[gpui::test]
    fn create_dialog_destination_tracks_full_branch_name(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        // "n" opens the create dialog with the branch field focused.
        vcx.simulate_keystrokes("n");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("f e a t");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let DialogState::Create {
                branch,
                dest,
                dest_edited,
                ..
            } = &root.dialog
            else {
                panic!("create dialog not open");
            };
            assert_eq!(branch.read(cx).value, "feat");
            let root_dir = root.store.read(cx).repo_root.clone().unwrap();
            let expected = crate::model::default_worktree_path(&root_dir, "feat");
            assert_eq!(dest.read(cx).value, expected.display().to_string());
            assert!(
                !*dest_edited,
                "programmatic dest updates must not count as user edits"
            );
        });
    }

    #[gpui::test]
    fn empty_state_path_input_keeps_typing(cx: &mut TestAppContext) {
        let (view, mut vcx) = open_root_no_repo(cx);
        vcx.run_until_parked();

        // Focus the path input, then type an absolute path. "/" must be
        // inserted, not repurposed to focus the (unrendered) search field.
        let handle = view.update(&mut vcx.cx, |root, cx| {
            root.path_input.read(cx).focus_handle.clone()
        });
        vcx.update(|window, _cx| window.focus(&handle));
        vcx.simulate_keystrokes("/ U s e r s / g r e g");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            // If focus had been stolen to the unrendered search field after
            // the first "/", the remaining characters would never arrive.
            assert_eq!(root.path_input.read(cx).value, "/Users/greg");
            assert!(root.store.read(cx).repo_root.is_none(), "still empty state");
        });
    }

    #[gpui::test]
    fn lists_worktrees_and_opens_create_dialog(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        view.update(&mut vcx.cx, |root, cx| {
            let store = root.store.read(cx);
            assert_eq!(store.entries.len(), 2);
            assert!(store.entries[0].is_main);
            assert!(matches!(
                store.entries[0].status,
                crate::model::WorktreeStatus::Clean { .. }
            ));
            assert_eq!(store.entries[1].branch.as_deref(), Some("feat"));
            assert!(matches!(root.dialog, DialogState::None));
        });

        // "n" opens the create dialog pre-filled with the default base.
        vcx.simulate_keystrokes("n");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert!(matches!(root.dialog, DialogState::Create { .. }));
            if let DialogState::Create { base, .. } = &root.dialog {
                assert_eq!(base.read(cx).value, "main");
            }
        });

        // escape closes it.
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(matches!(root.dialog, DialogState::None));
        });
    }

    #[gpui::test]
    fn typing_in_search_filters_the_list(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("/ f e a t");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let store = root.store.read(cx);
            assert_eq!(store.filter, "feat");
            assert_eq!(store.filtered.len(), 1);
            assert_eq!(
                store.entries[store.filtered[0]].branch.as_deref(),
                Some("feat")
            );
        });

        // escape clears the filter and refocuses the list.
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let store = root.store.read(cx);
            assert!(store.filter.is_empty());
            assert_eq!(store.filtered.len(), 2);
        });
    }

    #[gpui::test]
    fn enter_drills_into_detail_and_esc_returns(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert!(root.detail.is_some(), "detail opens on enter");
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(wc.wc.is_some(), "working copy loaded");
        });

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(root.detail.is_none(), "esc returns to the list");
        });
    }

    #[gpui::test]
    fn t_still_opens_terminal_from_list(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);
        let selected_path = view.update(&mut vcx.cx, |root, cx| {
            root.store.read(cx).selected_entry().unwrap().path.clone()
        });

        TERMINAL_REQUESTS.lock().unwrap().clear();
        vcx.simulate_keystrokes("t");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(root.detail.is_none(), "t must not open the detail view");
            assert!(matches!(root.dialog, DialogState::None));
        });
        assert_eq!(
            TERMINAL_REQUESTS.lock().unwrap().clone(),
            vec![selected_path.clone()],
            "t must request exactly one terminal open at the selected worktree"
        );

        // Drilled in, "t" opens a terminal at the same worktree via the
        // detail-context path.
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        TERMINAL_REQUESTS.lock().unwrap().clear();
        vcx.simulate_keystrokes("t");
        vcx.run_until_parked();
        assert_eq!(
            TERMINAL_REQUESTS.lock().unwrap().clone(),
            vec![selected_path],
            "detail-context t records the same worktree path"
        );
    }

    #[gpui::test]
    fn stage_keys_toggle_files(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("new.txt"), "untracked").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // navigate to the untracked row and stage it
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        let refreshed_before =
            view.update(&mut vcx.cx, |root, cx| root.store.read(cx).last_refreshed);
        vcx.simulate_keystrokes("s");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(wc.staged_count(), 1, "s stages the selected untracked file");
            // `mutated` itself is consumed by the detail observer to
            // trigger the home refresh, so assert the observable effect: the
            // home list re-refreshed with fresh status data.
            let home = root.store.read(cx);
            assert_ne!(
                home.last_refreshed, refreshed_before,
                "staging flags home refresh"
            );
            // The refresh carried fresh data: the untracked file moved into
            // the index (staging makes the worktree dirty-staged, not clean).
            match home.entries[0].status {
                crate::model::WorktreeStatus::Dirty {
                    staged, untracked, ..
                } => assert_eq!(
                    (staged, untracked),
                    (1, 0),
                    "untracked file moved to staged on the home badge"
                ),
                _ => panic!("expected a dirty badge after staging"),
            }
        });

        // "S" (stage all): gpui normalizes capital keystrokes to lowercase
        // key + shift, so simulate the same shape real keyboards produce.
        std::fs::write(repo.join("another.txt"), "x").unwrap();
        vcx.simulate_keystrokes("r");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("shift-s");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            let groups: Vec<_> = wc.rows().iter().map(|(g, _)| *g).collect();
            assert!(
                !groups.contains(&crate::engine::working_copy::Group::Untracked),
                "shift-s stages everything, including untracked rows"
            );
            assert!(
                matches!(
                    wc.selected_row(),
                    Some((crate::engine::working_copy::Group::Staged, _))
                ),
                "selection stays on a staged row after stage-all"
            );
        });
    }

    #[gpui::test]
    fn discard_opens_confirm_dialog_and_esc_cancels(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "changed").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // f.txt is modified-unstaged; it's row 0 of Unstaged (only row)
        vcx.simulate_keystrokes("d");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(
                matches!(root.dialog, DialogState::Discard { .. }),
                "discard needs confirmation"
            );
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        let list_focus = view.update(&mut vcx.cx, |root, _cx| root.detail_list_focus.clone());
        vcx.update(|window, _cx| {
            assert!(
                list_focus.is_focused(window),
                "esc over the detail view must hand focus back to the detail list"
            );
        });
        view.update(&mut vcx.cx, |root, cx| {
            assert!(matches!(root.dialog, DialogState::None));
            assert_eq!(
                root.detail
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .wc
                    .as_ref()
                    .unwrap()
                    .entries
                    .len(),
                1,
                "nothing discarded"
            );
        });
    }

    #[gpui::test]
    fn discard_confirms_the_dialog_path_not_the_current_selection(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("h.txt"), "one").unwrap();
        sh(&repo, &["git", "add", "--", "h.txt"]);
        sh(&repo, &["git", "commit", "-qm", "add h.txt"]);
        std::fs::write(repo.join("f.txt"), "F-EDIT").unwrap();
        std::fs::write(repo.join("h.txt"), "H-EDIT").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // Two unstaged rows: f.txt (0), h.txt (1). Open the dialog for
        // h.txt…
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("d");
        vcx.run_until_parked();
        // …then move the selection back to f.txt, the way a refresh
        // completing mid-dialog could.
        view.update(&mut vcx.cx, |root, cx| {
            root.detail
                .as_ref()
                .unwrap()
                .update(cx, |wc, cx| wc.select(Some(0), cx));
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter"); // confirm
        vcx.run_until_parked();

        // The dialog's path (h.txt) was discarded; the newly selected file
        // (f.txt) must be untouched.
        assert_eq!(
            std::fs::read_to_string(repo.join("h.txt")).unwrap(),
            "one",
            "h.txt reverted to committed content"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "F-EDIT",
            "f.txt must NOT be discarded — the dialog was opened for h.txt"
        );
    }

    #[gpui::test]
    fn d_on_a_staged_row_explains_instead_of_no_op(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "changed").unwrap();
        sh(&repo, &["git", "add", "--", "f.txt"]);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // Row 0 = Staged f.txt: discard is not offered.
        vcx.simulate_keystrokes("d");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert!(
                matches!(root.dialog, DialogState::None),
                "no dialog for staged rows"
            );
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(
                wc.message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("unstage first"),
                "expected the staged-row hint, got {:?}",
                wc.message
            );
        });
    }

    #[gpui::test]
    fn refresh_keeps_the_selected_surface_of_a_dual_group_file(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        // f.txt is staged ("one two") and further modified unstaged
        // ("three FOUR"): it appears as BOTH a Staged row and an Unstaged row.
        std::fs::write(repo.join("f.txt"), "one two\nthree\n").unwrap();
        sh(&repo, &["git", "add", "--", "f.txt"]);
        std::fs::write(repo.join("f.txt"), "one two\nthree FOUR\n").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // Row 0 = Staged f.txt; row 1 = Unstaged f.txt. Move to the
        // Unstaged row, then let a refresh land.
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            let (group, entry) = wc.read(cx).selected_row().unwrap();
            assert_eq!(
                (group, entry.path.as_str()),
                (crate::engine::working_copy::Group::Unstaged, "f.txt"),
                "precondition: Unstaged f.txt selected"
            );
            wc.update(cx, |store, cx| store.refresh(cx));
        });
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            let (group, entry) = wc.read(cx).selected_row().unwrap();
            assert_eq!(
                (group, entry.path.as_str()),
                (crate::engine::working_copy::Group::Unstaged, "f.txt"),
                "a landing refresh must keep the Unstaged surface selected — \
                 snapping to Staged would flip the next `s` into an unstage"
            );
        });
    }

    #[gpui::test]
    fn esc_is_ignored_while_an_operation_is_in_flight(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "changed").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // Simulate an in-flight operation (e.g. the commit editor session):
        // closing the detail view now would orphan it — a re-drill opens a
        // fresh idle store while the old commit is still pending.
        view.update(&mut vcx.cx, |root, cx| {
            root.detail.as_ref().unwrap().update(cx, |wc, _cx| {
                wc.mutating = true;
            });
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().expect("busy detail must not close");
            assert!(
                wc.read(cx).message.as_deref() == Some("Busy — wait for the current operation"),
                "expected the busy hint, got {:?}",
                wc.read(cx).message
            );
        });
        // Once the operation completes, esc works again.
        view.update(&mut vcx.cx, |root, cx| {
            root.detail.as_ref().unwrap().update(cx, |wc, _cx| {
                wc.mutating = false;
                wc.message = None;
            });
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(root.detail.is_none(), "idle detail closes normally");
        });
    }

    #[gpui::test]
    fn two_opens_history_one_returns_to_working_copy(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "changed").unwrap();
        sh(&repo, &["git", "add", "f.txt"]);
        sh(&repo, &["git", "commit", "-qm", "second commit"]);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert_eq!(root.section, Section::WorkingCopy);
            assert!(root.history.is_none());
        });
        vcx.simulate_keystrokes("2");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert_eq!(root.section, Section::History);
            let hs = root.history.as_ref().expect("history store created");
            assert!(!hs.read(cx).commits.is_empty(), "log loaded");
        });
        // The history list has focus; down moves the commit selection.
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let hs = root.history.as_ref().unwrap().read(cx);
            assert_eq!(hs.selected, Some(1), "down moved the commit selection");
        });
        vcx.simulate_keystrokes("1");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert_eq!(root.section, Section::WorkingCopy);
            assert!(root.detail.is_some(), "still drilled in");
        });
    }

    #[gpui::test]
    fn history_files_pane_loads_the_commit_diff(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("h.txt"), "one").unwrap();
        sh(&repo, &["git", "add", "h.txt"]);
        sh(&repo, &["git", "commit", "-qm", "add h"]);
        std::fs::write(repo.join("h.txt"), "two").unwrap();
        sh(&repo, &["git", "add", "h.txt"]);
        sh(&repo, &["git", "commit", "-qm", "edit h"]);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("2");
        vcx.run_until_parked();
        // Newest commit ("edit h") is pre-selected; tab to the files pane
        // and walk to its only file.
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let hs = root.history.as_ref().unwrap().read(cx);
            assert_eq!(hs.pane, crate::history_store::Pane::Files);
            let files = hs.files.as_ref().expect("files loaded");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].letter, 'M');
            assert_eq!(files[0].path, "h.txt");
            let diff = hs.file_diff.as_ref().expect("diff loaded");
            assert!(diff
                .hunks
                .iter()
                .any(|h| h.lines.iter().any(
                    |l| l.kind == crate::engine::diff::DiffLineKind::Add && l.content == "two"
                )));
        });
    }

    #[gpui::test]
    fn hunk_movement_scrolls_the_hovered_hunk_into_view(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        // Hunk 1 is ~800 diff lines (taller than the viewport); hunk 2 sits
        // far below it. Both fit the render cap — only SCROLLING puts hunk
        // 2 on screen.
        let lines: Vec<String> = (1..=3000).map(|i| format!("line {i}")).collect();
        std::fs::write(repo.join("t.txt"), lines.join("\n") + "\n").unwrap();
        sh(&repo, &["git", "add", "t.txt"]);
        sh(&repo, &["git", "commit", "-qm", "t"]);
        let mut edited = lines.clone();
        for (i, l) in edited.iter_mut().enumerate().take(400) {
            *l = format!("edited {i}");
        }
        edited[2899] = "line 2900 edited".into();
        std::fs::write(repo.join("t.txt"), edited.join("\n") + "\n").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(wc.hunk_count(), Some(2));
            assert_eq!(root.diff_scroll.offset().y, gpui::px(0.), "starts at top");
        });
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(wc.hunk_cursor(), 1, "cursor on the second hunk");
            // gpui scrolls DOWN by making the content offset negative.
            assert!(
                root.diff_scroll.offset().y < gpui::px(0.),
                "the pane must scroll the hovered hunk into view, got {:?}",
                root.diff_scroll.offset()
            );
        });
    }

    #[gpui::test]
    fn hunk_movement_reveals_the_hovered_middle_hunk(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        // Three hunks: tall (h1), small (h2), small (h3). Hovering the
        // MIDDLE hunk must reveal h2 — if scroll_to_item resolved the
        // child after the hovered one (or missed entirely), h2 would sit
        // off-screen above or below the viewport.
        let lines: Vec<String> = (1..=1500).map(|i| format!("line {i}")).collect();
        std::fs::write(repo.join("m.txt"), lines.join("\n") + "\n").unwrap();
        sh(&repo, &["git", "add", "m.txt"]);
        sh(&repo, &["git", "commit", "-qm", "m"]);
        let mut edited = lines.clone();
        for (i, l) in edited.iter_mut().enumerate().take(250) {
            *l = format!("edited {i}");
        }
        edited[699] = "line 700 edited".into();
        edited[1399] = "line 1400 edited".into();
        std::fs::write(repo.join("m.txt"), edited.join("\n") + "\n").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("down"); // hover hunk 2 (the middle one)
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(wc.hunk_cursor(), 1);
            let handle = &root.diff_scroll;
            // The hovered block is hunk 2: child index 2 (file-header
            // summary is child 0). Its TOP must be on screen — if
            // scroll_to_item resolved the child AFTER the hovered one (or
            // nothing), hunk 2's top would sit above the viewport.
            let bounds = handle
                .bounds_for_item(2)
                .expect("hunk 2 block has recorded bounds");
            let offset = handle.offset().y;
            let viewport = handle.bounds().size.height;
            assert!(
                bounds.top() + offset < viewport - gpui::px(10.),
                "hunk 2's top must be on screen (top {:#?} + offset {offset:#?} vs viewport {viewport:#?})",
                bounds.top()
            );
            assert!(
                bounds.bottom() + offset > gpui::px(0.),
                "hunk 2 must not be scrolled past (bottom {:#?} + offset {offset:#?})",
                bounds.bottom()
            );
        });
    }

    #[gpui::test]
    fn staging_a_hunk_reveals_the_next_hovered_hunk(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        // Three hunks with a tall first hunk: after staging hunk 3 the
        // cursor clamps onto hunk 2, which sits BELOW the viewport at the
        // old scroll offset — the post-mutation reload must reveal it, or
        // the next `s` stages content the user cannot see.
        let lines: Vec<String> = (1..=1500).map(|i| format!("line {i}")).collect();
        std::fs::write(repo.join("m.txt"), lines.join("\n") + "\n").unwrap();
        sh(&repo, &["git", "add", "m.txt"]);
        sh(&repo, &["git", "commit", "-qm", "m"]);
        let mut edited = lines.clone();
        for (i, l) in edited.iter_mut().enumerate().take(250) {
            *l = format!("edited {i}");
        }
        edited[699] = "line 700 edited".into();
        edited[1399] = "line 1400 edited".into();
        std::fs::write(repo.join("m.txt"), edited.join("\n") + "\n").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("down");
        vcx.simulate_keystrokes("down"); // hover hunk 3 (the last one)
        vcx.simulate_keystrokes("s"); // stage it
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(wc.hunk_count(), Some(2), "hunk 3 staged and gone");
            assert_eq!(wc.hunk_cursor(), 1, "cursor clamped onto hunk 2");
            let handle = &root.diff_scroll;
            // The hovered block is hunk 2: child index 2 (file-header
            // summary is child 0). Its TOP must be on screen after the
            // post-mutation reload.
            let bounds = handle
                .bounds_for_item(2)
                .expect("hunk 2 block has recorded bounds");
            let offset = handle.offset().y;
            let viewport = handle.bounds().size.height;
            assert!(
                bounds.top() + offset < viewport - gpui::px(10.)
                    && bounds.bottom() + offset > gpui::px(0.),
                "hunk 2 must be revealed after staging hunk 3 (top {:#?} + offset {offset:#?} vs viewport {viewport:#?})",
                bounds.top()
            );
        });
    }

    #[gpui::test]
    fn diff_pane_shift_s_still_stages_all(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let lines: Vec<String> = (1..=12).map(|i| format!("line {i}")).collect();
        std::fs::write(repo.join("h.txt"), lines.join("\n") + "\n").unwrap();
        sh(&repo, &["git", "add", "h.txt"]);
        sh(&repo, &["git", "commit", "-qm", "h"]);
        std::fs::write(repo.join("h.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("u.txt"), "brand new").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        // Muscle memory: S means stage-ALL on every surface, never a
        // one-hunk index mutation.
        vcx.simulate_keystrokes("shift-s");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(
                wc.staged_count(),
                2,
                "shift+s staged the whole working copy from the diff pane"
            );
        });
    }

    #[gpui::test]
    fn diff_pane_hunk_keys_stage_the_hovered_hunk(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        // A committed 12-line file edited on lines 1 and 10 → an unstaged
        // row with two hunks, plus an untracked file below it.
        let lines: Vec<String> = (1..=12).map(|i| format!("line {i}")).collect();
        std::fs::write(repo.join("h.txt"), lines.join("\n") + "\n").unwrap();
        sh(&repo, &["git", "add", "h.txt"]);
        sh(&repo, &["git", "commit", "-qm", "h"]);
        let mut edited = lines.clone();
        edited[0] = "line 1 edited".into();
        edited[9] = "line 10 edited".into();
        std::fs::write(repo.join("h.txt"), edited.join("\n") + "\n").unwrap();
        std::fs::write(repo.join("u.txt"), "brand new").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // Row 0 is the unstaged h.txt. Tab to the diff pane, hover the
        // second hunk, stage it.
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            assert_eq!(wc.read(cx).pane, Pane::Diff);
            assert_eq!(wc.read(cx).hunk_count(), Some(2));
        });
        vcx.simulate_keystrokes("down");
        vcx.simulate_keystrokes("s");
        vcx.run_until_parked();

        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert_eq!(wc.hunk_count(), Some(1), "unstaged diff shrank to one hunk");
            assert_eq!(wc.hunk_cursor(), 0, "cursor clamped after the stage");
            assert_eq!(wc.staged_count(), 1, "h.txt gained a Staged row");
        });
        // git agrees: the index holds only the line-10 edit; the worktree
        // still holds both.
        let out = std::process::Command::new("git")
            .args(["diff", "--cached", "--no-color", "-U3", "--", "h.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success());
        let staged = String::from_utf8_lossy(&out.stdout);
        assert!(staged.contains("line 10 edited"), "staged: {staged}");
        assert!(!staged.contains("line 1 edited"), "staged: {staged}");
        let on_disk = std::fs::read_to_string(repo.join("h.txt")).unwrap();
        assert!(on_disk.contains("line 1 edited"));
    }

    #[gpui::test]
    fn discard_refuses_when_live_state_contradicts_the_dialog(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("u.txt"), "brand new").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // Only change is the untracked u.txt → row 0. Open the dialog for
        // it while it's untracked…
        vcx.simulate_keystrokes("d");
        vcx.run_until_parked();
        // …then stage it mid-dialog (as a pre-dialog mutation landing would).
        view.update(&mut vcx.cx, |root, cx| {
            root.detail
                .as_ref()
                .unwrap()
                .update(cx, |wc, cx| wc.toggle_stage(cx));
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter"); // confirm discard
        vcx.run_until_parked();

        // The file flipped untracked → tracked while the dialog was open,
        // which contradicts what the user confirmed. The confirm must
        // refuse (the completion refreshes the snapshot) rather than act:
        // acting on either state risks restoring or deleting on stale
        // information. Either way the file — and its content — survives.
        assert!(
            repo.join("u.txt").exists(),
            "a refused discard must not touch the file"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("u.txt")).unwrap(),
            "brand new",
            "a refused discard must not modify the file"
        );
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().expect("refusal keeps the detail open");
            assert!(
                wc.read(cx)
                    .message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("state changed"),
                "expected the flip refusal, got {:?}",
                wc.read(cx).message
            );
            // The post-refusal refresh caught the mid-dialog stage: u.txt is
            // now a Staged row, so reopening the dialog shows the new state.
            let store = wc.read(cx);
            let staged = store.rows().iter().any(|(g, i)| {
                *g == crate::engine::working_copy::Group::Staged
                    && store.wc.as_ref().unwrap().entries[*i].path == "u.txt"
            });
            assert!(staged, "snapshot refreshed after the refusal");
        });
    }

    #[gpui::test]
    fn diff_pane_renders_selected_file_and_caps_long_files(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            // f.txt is the default selection (its unstaged modification is
            // the only row); the pane's data — the unified diff — must be
            // loaded and contain the modification's hunk.
            let wc = root.detail.as_ref().unwrap().read(cx);
            match &wc.detail {
                Some(FileDetail::Diff(d)) => {
                    assert!(!d.hunks.is_empty(), "f.txt modification has a hunk");
                    assert!(
                        d.hunks.iter().any(|h| {
                            h.lines.iter().any(|l| {
                                l.kind == crate::engine::diff::DiffLineKind::Add
                                    && l.content == "two"
                            })
                        }),
                        "the +two modification line is in the rendered diff"
                    );
                }
                other => panic!("expected a loaded diff for f.txt, got {other:?}"),
            }
        });
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            assert_eq!(
                wc.read(cx).pane,
                Pane::Diff,
                "tab moves the pane state to the diff"
            );
        });
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            assert_eq!(
                wc.read(cx).pane,
                Pane::Files,
                "second tab returns to the file list"
            );
        });
    }

    #[gpui::test]
    fn commit_key_requires_staged_changes(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert_eq!(
                root.detail.as_ref().unwrap().read(cx).staged_count(),
                0,
                "fixture starts with nothing staged"
            );
        });
        vcx.simulate_keystrokes("c");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(
                wc.message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Nothing staged"),
                "expected the nothing-staged hint, got {:?}",
                wc.message
            );
            assert!(!wc.mutating, "no editor spawned");
        });
    }
}
