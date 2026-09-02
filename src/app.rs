use crate::dialogs::{self, DialogState};
use crate::model::WorktreeEntry;
use crate::model::WorktreeStatus;
use crate::platform;
use crate::store::WorktreeStore;
use crate::terminal;
use crate::text_field::TextField;
use crate::views::working_copy;
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

fn open_terminal(path: &std::path::Path) {
    #[cfg(test)]
    TERMINAL_REQUESTS.lock().unwrap().push(path.to_path_buf());
    #[cfg(not(test))]
    terminal::open_in_terminal(path);
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

fn toolbar_button(
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
    fn focus_active_surface(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if self.detail.is_some() {
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
        if let (Some(wc), DialogState::Discard { path, .. }) = (&self.detail, &self.dialog) {
            // Discard exactly the path the dialog was opened for — never
            // "the current selection", which a refresh can move while the
            // dialog sits open. The store derives HOW from the file's
            // current state, so a mid-dialog untracked→tracked flip can't
            // redirect the destructive action.
            let path = path.clone();
            wc.update(cx, |store, cx| store.discard_path(path, cx));
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
        let eligible = matches!(
            group,
            crate::engine::working_copy::Group::Unstaged
                | crate::engine::working_copy::Group::Untracked
        ) && !entry.is_dir();
        if !eligible {
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
        let wc = WorkingCopyStore::new(entry.path.clone(), cx);
        // One successful mutation inside the detail view must refresh the home
        // worktree list (status, ahead/behind, dirty badge all change). The
        // subscription is stored on the view, not detached: a previous drill-in's
        // observer would otherwise accumulate (dropped stores make old observers
        // inert but never remove their subscription entries).
        self.detail_subscription = Some(cx.observe(&wc, move |this, wc, cx| {
            if wc.update(cx, |store, _cx| store.take_mutated()) {
                this.store.update(cx, |store, cx| store.refresh(cx));
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
        // A busy detail view means an operation is in flight — possibly the
        // commit editor, which can run for minutes. Dropping the store now
        // would orphan it: re-drilling opens a fresh, idle store while the
        // old commit is still pending, re-opening the mutate-under-pending-
        // commit hole the busy-gating exists to close.
        let busy = self.detail.as_ref().is_some_and(|wc| wc.read(cx).mutating);
        if busy {
            if let Some(wc) = &self.detail {
                wc.update(cx, |store, cx| store.busy_message(cx));
            }
            return;
        }
        // Drop the observer first: a dropped Subscription unsubscribes.
        self.detail_subscription = None;
        self.detail = None;
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
            "c" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.commit_with_editor(cx));
                }
            }
            // Diff-pane hunk keys ("s" with diff focus) arrive in Phase 1b.
            _ => {}
        }
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
                this.store.update(cx, |store, cx| store.refresh(cx));
            }))
            .on_action(cx.listener(|this, _: &Prune, _window, cx| {
                this.store.update(cx, |store, cx| store.prune(cx));
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, _window, cx| {
                if let Some(entry) = this.store.read(cx).selected_entry() {
                    let path = entry.path.clone();
                    terminal::open_in_terminal(&path);
                }
            }))
            .on_action(cx.listener(|this, _: &RemoveSelected, window, cx| {
                if !this.dialog.is_open() {
                    this.open_remove_dialog(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
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
                    .child(self.search.clone())
                    .child(toolbar_button(
                        "btn-new",
                        "New (n)",
                        cx.listener(|this, _, window, cx| this.open_create_dialog(window, cx)),
                    ))
                    .child(toolbar_button(
                        "btn-refresh",
                        "Refresh (r)",
                        cx.listener(|this, _, _window, cx| {
                            this.store.update(cx, |store, cx| store.refresh(cx))
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
                main.child(working_copy::render(self, window, cx).into_any_element())
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
                                    terminal::open_in_terminal(&terminal_path);
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
    fn discard_derives_action_from_live_state_not_dialog_snapshot(cx: &mut TestAppContext) {
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

        // The file flipped untracked → tracked while the dialog was open.
        // The confirm must derive from the LIVE state (tracked → restore
        // from index), never the snapshot (untracked → delete the file).
        assert!(
            repo.join("u.txt").exists(),
            "a file that became tracked mid-dialog must not be deleted"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("u.txt")).unwrap(),
            "brand new",
            "discard restores the index copy for a tracked file"
        );
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
