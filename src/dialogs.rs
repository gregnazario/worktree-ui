//! Modal dialog state + rendering. Dialogs are plain state on `RootView`
//! (not separate entities); the render helpers below take the root view
//! directly and attach listeners against it.

use crate::app::RootView;
use crate::app::{ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT, YELLOW};
use crate::feedback;
use crate::platform;
use crate::terminal::{self, InstalledTerminal};
use crate::text_field::TextField;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, rgb, App, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use std::path::PathBuf;

pub enum DialogState {
    None,
    Create {
        branch: Entity<TextField>,
        base: Entity<TextField>,
        dest: Entity<TextField>,
        /// false = check out an existing branch instead of creating one.
        new_branch: bool,
        /// Set once the user edits the destination manually; disables the
        /// live default-path derivation.
        dest_edited: bool,
        /// Last destination value derived programmatically from the branch
        /// name; a dest observer seeing exactly this value knows the change
        /// came from the derivation, not the user.
        last_derived: String,
    },
    Remove {
        path: PathBuf,
        branch_label: SharedString,
        dirty: bool,
        force: bool,
    },
    Settings {
        terminals: Vec<InstalledTerminal>,
        selected: Option<String>,
        saved_to: Option<String>,
    },
    /// Confirm before discarding one file's uncommitted changes. `untracked`
    /// means the file itself is deleted, not just reverted.
    Discard {
        path: String,
        untracked: bool,
    },
    /// Create a branch at HEAD, or rename an existing one.
    BranchName {
        title: String,
        /// Some(branch) = rename that branch; None = create at HEAD.
        rename_from: Option<String>,
        name: Entity<TextField>,
    },
    /// Scripted interactive rebase: the todo rows (oldest first), the
    /// rebase base, and the dialog's own cursor.
    RebaseTodo {
        base: String,
        entries: Vec<RebaseEntry>,
        selected: usize,
    },
    /// In-app commit editor: a multi-line message draft plus the staged
    /// summary for the hint pre-fill. The comment char is the repo's
    /// (`core.commentChar`) so the hints and the confirm-strip agree
    /// with what the engine will strip.
    CommitEditor {
        field: Entity<crate::text_area::TextArea>,
        staged_summary: String,
        comment_char: char,
    },
    /// Remote management (home screen). The list loads on the background
    /// executor after the dialog opens; `loading` covers the gap.
    RemotesDialog {
        repo: PathBuf,
        entries: Vec<crate::engine::remotes::RemoteInfo>,
        selected: usize,
        loading: bool,
        load_failed: Option<String>,
    },
    /// Add a remote: name + URL.
    AddRemote {
        repo: PathBuf,
        name: Entity<TextField>,
        url: Entity<TextField>,
    },
    /// Confirm before `git remote remove` (ref surgery: tracking refs go).
    RemoveRemote {
        repo: PathBuf,
        name: String,
    },
}

/// One todo row. `action` mutates in place via the dialog keys; the
/// confirm converts these into the engine's `TodoStep`s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseEntry {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub action: crate::engine::rewrite::TodoAction,
}

impl DialogState {
    pub fn is_open(&self) -> bool {
        !matches!(self, DialogState::None)
    }
}

fn label(text: String) -> impl IntoElement {
    div().text_size(px(12.)).text_color(DIM).child(text)
}

fn field_row(label_text: &str, field: Entity<TextField>, hint: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(label(label_text.to_string()))
        .child(div().flex().items_center().child(field))
        .child(label(hint))
}

fn button(
    id: &'static str,
    text: &str,
    fg: gpui::Rgba,
    bg: Option<gpui::Rgba>,
    border: Option<gpui::Rgba>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .text_size(px(13.))
        .text_color(fg)
        .when_some(bg, |btn, bg| btn.bg(bg))
        .when_some(border, |btn, border| btn.border_1().border_color(border))
        .child(text.to_string())
        .on_click(on_click)
}

fn confirm_create(this: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) {
    if let DialogState::Create {
        branch,
        base,
        dest,
        new_branch,
        ..
    } = &this.dialog
    {
        let branch_value = branch.read(cx).value.trim().to_string();
        let base_value = base.read(cx).value.trim().to_string();
        let dest_value = dest.read(cx).value.trim().to_string();
        if branch_value.is_empty() || dest_value.is_empty() {
            return;
        }
        // Defense in depth on top of the `--` separators in git.rs: reject
        // dash-prefixed names outright so they can never confuse other
        // tooling downstream.
        if branch_value.starts_with('-')
            || dest_value.starts_with('-')
            || base_value.starts_with('-')
        {
            this.store.update(cx, |store, cx| {
                store.status_message = Some("Names may not start with '-'".into());
                cx.notify();
            });
            return;
        }
        // new-branch mode: create `branch` off `base` (or the repo default).
        // existing-branch mode: check out `branch` itself.
        let dest_path = crate::model::expand_tilde(&dest_value);
        let (branch_arg, base_arg) = if *new_branch {
            let default_base = this.store.read(cx).default_base.clone();
            (
                Some(branch_value.clone()),
                if base_value.is_empty() {
                    default_base
                } else {
                    base_value
                },
            )
        } else {
            (None, branch_value.clone())
        };
        this.store.update(cx, |store, cx| {
            store.add(dest_path, branch_arg, base_arg, cx)
        });
        this.close_dialog(window, cx);
    }
}

fn cancel(this: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) {
    this.close_dialog(window, cx);
}

pub fn render_create_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::Create {
        branch,
        base,
        dest,
        new_branch,
        ..
    } = &this.dialog
    else {
        unreachable!("create dialog rendered without create state")
    };

    let branch_value = branch.read(cx).value.trim().to_string();
    let base_value = base.read(cx).value.trim().to_string();
    let branches = this.store.read(cx).local_branches.clone();
    let default_base = this.store.read(cx).default_base.clone();

    let mode_label = if *new_branch {
        "Branch (new)"
    } else {
        "Branch (existing)"
    };
    let hint = if *new_branch {
        if base_value.is_empty() {
            format!("base: {default_base}")
        } else {
            format!("base: {base_value}")
        }
    } else {
        format!("available: {}", branches.join(", "))
    };
    let can_confirm = !branch_value.is_empty() && !dest.read(cx).value.trim().is_empty();
    let toggle_label = if *new_branch {
        "Creating a new branch — switch to existing branch"
    } else {
        "Using an existing branch — switch to new branch"
    };

    let base_field = base.clone();
    let branch_field = branch.clone();
    let dest_field = dest.clone();
    let dialog_focus = this.dialog_focus.clone();

    div()
        .id("create-dialog")
        .track_focus(&dialog_focus)
        .w(px(560.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "enter" => confirm_create(this, window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("New worktree"),
        )
        .child(field_row(mode_label, branch_field, hint))
        .when(*new_branch, |card| {
            card.child(field_row("Base", base_field, String::new()))
        })
        .child(field_row("Destination", dest_field, String::new()))
        .child(
            div()
                .id("toggle-new-branch")
                .text_size(px(12.))
                .text_color(ACCENT)
                .child(toggle_label.to_string())
                .on_click(cx.listener(|this, _, _window, cx| {
                    if let DialogState::Create { new_branch, .. } = &mut this.dialog {
                        *new_branch = !*new_branch;
                        cx.notify();
                    }
                })),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .when(!can_confirm, |row| row.opacity(0.4))
                .child(button(
                    "create-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "create-confirm",
                    "Create",
                    rgb(0x11111b),
                    Some(GREEN),
                    None,
                    cx.listener(|this, _, window, cx| confirm_create(this, window, cx)),
                )),
        )
}

pub fn render_branch_name_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::BranchName {
        title,
        rename_from,
        name,
    } = &this.dialog
    else {
        unreachable!("branch dialog rendered without state")
    };
    let value = name.read(cx).value.trim().to_string();
    let can_confirm = !value.is_empty() && !value.starts_with('-');
    let confirm_label = if rename_from.is_some() {
        "Rename"
    } else {
        "Create"
    };

    let dialog_focus = this.dialog_focus.clone();
    let name_field = name.clone();
    div()
        .id("branch-name-dialog")
        .track_focus(&dialog_focus)
        .w(px(420.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "enter" => this.confirm_branch_name_dialog(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child(title.clone()),
        )
        .child(field_row("Name", name_field, String::new()))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .when(!can_confirm, |row| row.opacity(0.4))
                .child(button(
                    "branch-name-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "branch-name-confirm",
                    confirm_label,
                    rgb(0x11111b),
                    Some(GREEN),
                    None,
                    cx.listener(|this, _, window, cx| this.confirm_branch_name_dialog(window, cx)),
                )),
        )
}

pub fn render_remove_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::Remove {
        path,
        branch_label,
        dirty,
        force,
    } = &this.dialog
    else {
        unreachable!("remove dialog rendered without remove state")
    };
    let warning = if *dirty {
        "This worktree contains uncommitted changes."
    } else {
        "This worktree is clean."
    };
    let dialog_focus = this.dialog_focus.clone();

    div()
        .id("remove-dialog")
        .track_focus(&dialog_focus)
        .w(px(500.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "enter" => this.confirm_remove(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("Remove worktree"),
        )
        .child(label(format!("Branch: {branch_label}")))
        .child(label(format!("Path: {}", path.display())))
        .child(
            div()
                .text_size(px(12.))
                .text_color(if *dirty { RED } else { DIM })
                .child(warning.to_string()),
        )
        .when(*dirty, |card| {
            card.child(
                div()
                    .id("remove-force")
                    .text_size(px(12.))
                    .text_color(if *force { RED } else { DIM })
                    .child(format!(
                        "[{}] Force removal (discards uncommitted changes)",
                        if *force { "x" } else { " " }
                    ))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        if let DialogState::Remove { force, .. } = &mut this.dialog {
                            *force = !*force;
                            cx.notify();
                        }
                    })),
            )
        })
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(button(
                    "remove-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "remove-confirm",
                    "Remove",
                    rgb(0x11111b),
                    Some(RED),
                    None,
                    cx.listener(|this, _, window, cx| this.confirm_remove(window, cx)),
                )),
        )
}

pub fn render_discard_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::Discard { path, untracked } = &this.dialog else {
        unreachable!("discard dialog rendered without discard state")
    };
    let path = path.clone();
    let untracked = *untracked;
    let dialog_focus = this.dialog_focus.clone();

    div()
        .id("discard-dialog")
        .track_focus(&dialog_focus)
        .w(px(500.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "enter" => this.confirm_discard(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("Discard changes"),
        )
        .child(label(format!("Path: {path}")))
        .child(
            div()
                .text_size(px(12.))
                .text_color(RED)
                .child("This cannot be undone."),
        )
        .when(untracked, |card| {
            card.child(
                div()
                    .text_size(px(12.))
                    .text_color(RED)
                    .child("The file itself will be deleted."),
            )
        })
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(button(
                    "discard-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "discard-confirm",
                    "Discard",
                    rgb(0x11111b),
                    Some(RED),
                    None,
                    cx.listener(|this, _, window, cx| this.confirm_discard(window, cx)),
                )),
        )
}

pub fn render_settings_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::Settings {
        terminals,
        selected,
        saved_to,
    } = &this.dialog
    else {
        unreachable!("settings dialog rendered without settings state")
    };
    let dialog_focus = this.dialog_focus.clone();
    let config_path = terminal::settings_path().display().to_string();
    let saved_note = saved_to.clone().unwrap_or_default();

    let mut card = div()
        .id("settings-dialog")
        .track_focus(&dialog_focus)
        .w(px(480.))
        .max_h(px(420.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            if event.keystroke.key == "escape" || event.keystroke.key == "enter" {
                this.close_dialog(window, cx);
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("Settings"),
        )
        .child(label("Terminal for \"Open in Terminal\"".into()));

    // Automatic (auto-detect) row. A stale saved id (terminal since
    // uninstalled) effectively behaves as automatic, so highlight it too.
    let auto_is_selected = selected
        .as_ref()
        .is_none_or(|s| !terminals.iter().any(|t| t.id == s.as_str()));
    card = card.child(
        div()
            .id("settings-terminal-auto")
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .when(auto_is_selected, |row| row.bg(ROW_SELECTED))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if auto_is_selected { ACCENT } else { TEXT })
                    .child("Automatic".to_string()),
            )
            .child(label(
                "first detected terminal (falls back to Terminal.app)".into(),
            ))
            .on_click(cx.listener(|this, _, _window, cx| {
                if let DialogState::Settings {
                    selected, saved_to, ..
                } = &mut this.dialog
                {
                    *selected = None;
                    match terminal::save_settings(&terminal::Settings::default()) {
                        Ok(path) => *saved_to = Some(path.display().to_string()),
                        Err(e) => *saved_to = Some(format!("save failed: {e}")),
                    }
                    cx.notify();
                }
            })),
    );

    // One row per installed terminal, registry order.
    for t in terminals {
        let id = t.id;
        let name = t.name;
        let is_selected = !auto_is_selected && selected.as_deref() == Some(id);
        let describe = terminal::describe_launch(&t.launch);
        card = card.child(
            div()
                .id(SharedString::from(format!("settings-terminal-{id}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .when(is_selected, |row| row.bg(ROW_SELECTED))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(if is_selected { ACCENT } else { TEXT })
                        .child(name.to_string()),
                )
                .child(label(describe))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let DialogState::Settings {
                        selected, saved_to, ..
                    } = &mut this.dialog
                    {
                        *selected = Some(id.to_string());
                        match terminal::save_settings(&terminal::Settings {
                            terminal: Some(id.to_string()),
                        }) {
                            Ok(path) => *saved_to = Some(path.display().to_string()),
                            Err(e) => *saved_to = Some(format!("save failed: {e}")),
                        }
                        cx.notify();
                    }
                })),
        );
    }

    card = card
        .child(label(format!("Config file: {config_path}")))
        .child(label(
            "$TERMCMD is used when no terminal is set above.".into(),
        ))
        .when(!saved_note.is_empty(), |c| {
            c.child(label(format!("Saved: {saved_note}")))
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(button(
                    "settings-report-bug",
                    "Report a bug",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|_, _, _window, _cx| {
                        platform::open_url(&feedback::report_bug_url());
                    }),
                ))
                .child(button(
                    "settings-close",
                    "Done",
                    rgb(0x11111b),
                    Some(GREEN),
                    None,
                    cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
                )),
        );
    card
}

pub fn render_rebase_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::RebaseTodo {
        entries, selected, ..
    } = &this.dialog
    else {
        unreachable!("rebase dialog rendered without state")
    };
    let entries = entries.clone();
    let selected = *selected;
    let drop_count = entries
        .iter()
        .filter(|e| e.action == crate::engine::rewrite::TodoAction::Drop)
        .count();
    let fixup_count = entries
        .iter()
        .filter(|e| e.action == crate::engine::rewrite::TodoAction::Fixup)
        .count();
    let mut summary = format!("{} commits", entries.len());
    if drop_count > 0 {
        summary.push_str(&format!(", {drop_count} dropped"));
    }
    if fixup_count > 0 {
        summary.push_str(&format!(", {fixup_count} fixupped"));
    }

    let dialog_focus = this.dialog_focus.clone();
    let mut list = div().id("rebase-rows").flex().flex_col().gap_px();
    for (pos, entry) in entries.iter().enumerate() {
        let is_selected = pos == selected;
        let (keyword, color) = match entry.action {
            crate::engine::rewrite::TodoAction::Pick => ("pick ", TEXT),
            crate::engine::rewrite::TodoAction::Drop => ("drop ", RED),
            crate::engine::rewrite::TodoAction::Fixup => ("fixup", YELLOW),
        };
        let row = div()
            .id(SharedString::from(format!("rebase-row-{pos}")))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_0p5()
            .rounded_sm()
            .when(is_selected, |r| r.bg(ROW_SELECTED))
            .on_click(cx.listener(move |this, _, _window, cx| {
                if let DialogState::RebaseTodo { selected, .. } = &mut this.dialog {
                    *selected = pos;
                    cx.notify();
                }
            }));
        let mut row = row;
        row = row.child(
            div()
                .w(px(44.))
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(color)
                .child(keyword),
        );
        row = row.child(
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(DIM)
                .child(entry.short.clone()),
        );
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(
                    if entry.action == crate::engine::rewrite::TodoAction::Drop {
                        DIM
                    } else {
                        TEXT
                    },
                )
                .truncate()
                .child(entry.subject.clone()),
        );
        list = list.child(row);
    }

    div()
        .id("rebase-dialog")
        .track_focus(&dialog_focus)
        .w(px(620.))
        .max_h(px(520.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .overflow_y_scroll()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            let DialogState::RebaseTodo {
                entries, selected, ..
            } = &mut this.dialog
            else {
                return;
            };
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "up" => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                    cx.notify();
                }
                "down" => {
                    if *selected + 1 < entries.len() {
                        *selected += 1;
                    }
                    cx.notify();
                }
                "d" => {
                    if let Some(entry) = entries.get_mut(*selected) {
                        entry.action = match entry.action {
                            crate::engine::rewrite::TodoAction::Pick => {
                                crate::engine::rewrite::TodoAction::Drop
                            }
                            _ => crate::engine::rewrite::TodoAction::Pick,
                        };
                    }
                    cx.notify();
                }
                "f" => {
                    if let Some(entry) = entries.get_mut(*selected) {
                        entry.action = match entry.action {
                            crate::engine::rewrite::TodoAction::Pick => {
                                crate::engine::rewrite::TodoAction::Fixup
                            }
                            _ => crate::engine::rewrite::TodoAction::Pick,
                        };
                    }
                    cx.notify();
                }
                "enter" => this.confirm_rebase_dialog(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child(format!("Interactive rebase — {summary}")),
        )
        .child(list)
        .child(label(
            "up/down move · d drop · f fixup · enter rebase · esc cancel".to_string(),
        ))
}

pub fn render_commit_editor_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::CommitEditor {
        field,
        staged_summary,
        comment_char,
    } = &this.dialog
    else {
        unreachable!("commit dialog rendered without state")
    };
    let staged_summary = staged_summary.clone();
    let comment_char = *comment_char;

    let dialog_focus = this.dialog_focus.clone();
    div()
        .id("commit-editor-dialog")
        .track_focus(&dialog_focus)
        .w(px(720.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            let ks = &event.keystroke;
            if ks.key == "enter" && (ks.modifiers.platform || ks.modifiers.control) {
                cx.stop_propagation();
                this.confirm_commit_dialog(window, cx);
                return;
            }
            if ks.key == "escape" {
                cx.stop_propagation();
                this.close_dialog(window, cx);
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("Commit"),
        )
        .child(div().text_size(px(11.)).text_color(DIM).child(format!(
            "{staged_summary} — lines starting with {comment_char:?} are removed from the message"
        )))
        .child(field.clone())
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(label(
                    "enter newline · cmd/ctrl+enter commit · esc cancel".to_string(),
                ))
                .child(button(
                    "commit-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "commit-confirm",
                    "Commit",
                    rgb(0x11111b),
                    Some(GREEN),
                    None,
                    cx.listener(|this, _, window, cx| this.confirm_commit_dialog(window, cx)),
                )),
        )
}

pub fn render_remotes_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::RemotesDialog {
        entries,
        selected,
        loading,
        load_failed,
        ..
    } = &this.dialog
    else {
        unreachable!("remotes dialog rendered without state")
    };
    let entries = entries.clone();
    let selected = *selected;
    let loading = *loading;
    let load_failed = load_failed.clone();

    let dialog_focus = this.dialog_focus.clone();
    let mut card = div()
        .id("remotes-dialog")
        .track_focus(&dialog_focus)
        .w(px(640.))
        .max_h(px(420.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_2()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            let DialogState::RemotesDialog {
                entries, selected, ..
            } = &mut this.dialog
            else {
                return;
            };
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "up" => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                    cx.notify();
                }
                "down" => {
                    if *selected + 1 < entries.len() {
                        *selected += 1;
                    }
                    cx.notify();
                }
                "a" => {
                    let DialogState::RemotesDialog { repo, .. } = &this.dialog else {
                        return;
                    };
                    let repo = repo.clone();
                    this.open_add_remote_dialog(repo, window, cx);
                }
                "d" | "delete" => {
                    let DialogState::RemotesDialog {
                        repo,
                        entries,
                        selected,
                        ..
                    } = &this.dialog
                    else {
                        return;
                    };
                    let Some(entry) = entries.get(*selected) else {
                        return;
                    };
                    let (repo, name) = (repo.clone(), entry.name.clone());
                    this.open_remove_remote_dialog(repo, name, window, cx);
                }
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("Remotes"),
        );

    if let Some(err) = load_failed {
        card = card.child(
            div()
                .text_size(px(12.))
                .text_color(RED)
                .child(format!("Loading remotes failed: {err}")),
        );
    } else if loading {
        card = card.child(div().text_size(px(12.)).text_color(DIM).child("Loading…"));
    } else if entries.is_empty() {
        card = card.child(
            div()
                .text_size(px(12.))
                .text_color(DIM)
                .child("No remotes — a adds one"),
        );
    } else {
        let mut list = div().id("remote-rows").flex().flex_col().gap_px();
        for (pos, entry) in entries.iter().enumerate() {
            let is_selected = pos == selected;
            let push_note = if entry.push_url != entry.fetch_url {
                format!("  (push: {})", entry.push_url)
            } else {
                String::new()
            };
            let row = div()
                .id(SharedString::from(format!("remote-row-{pos}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .when(is_selected, |r| r.bg(ROW_SELECTED))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let DialogState::RemotesDialog { selected, .. } = &mut this.dialog {
                        *selected = pos;
                        cx.notify();
                    }
                }));
            let mut row = row.child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(TEXT)
                    .truncate()
                    .child(entry.name.clone()),
            );
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(DIM)
                    .truncate()
                    .child(format!("{}{push_note}", entry.fetch_url)),
            );
            list = list.child(row);
        }
        card = card.child(list);
    }

    card.child(label(
        "a add remote · d remove selected · up/down move · esc close".to_string(),
    ))
}

pub fn render_add_remote_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::AddRemote { name, url, .. } = &this.dialog else {
        unreachable!("add-remote dialog rendered without state")
    };
    let name_value = name.read(cx).value.trim().to_string();
    let url_value = url.read(cx).value.trim().to_string();
    let can_confirm = !name_value.is_empty() && !url_value.is_empty();

    let dialog_focus = this.dialog_focus.clone();
    let name_field = name.clone();
    let url_field = url.clone();
    div()
        .id("add-remote-dialog")
        .track_focus(&dialog_focus)
        .w(px(560.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "enter" => this.confirm_add_remote(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(TEXT)
                .child("Add remote"),
        )
        .child(field_row("Name", name_field, "e.g. origin".to_string()))
        .child(field_row(
            "URL",
            url_field,
            "https:// or git@ssh path".to_string(),
        ))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .when(!can_confirm, |row| row.opacity(0.4))
                .child(button(
                    "add-remote-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "add-remote-confirm",
                    "Add",
                    rgb(0x11111b),
                    Some(GREEN),
                    None,
                    cx.listener(|this, _, window, cx| this.confirm_add_remote(window, cx)),
                )),
        )
}

pub fn render_remove_remote_dialog(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let DialogState::RemoveRemote { name, .. } = &this.dialog else {
        unreachable!("remove-remote dialog rendered without state")
    };
    let name = name.clone();

    let dialog_focus = this.dialog_focus.clone();
    div()
        .id("remove-remote-dialog")
        .track_focus(&dialog_focus)
        .w(px(480.))
        .p_4()
        .rounded_lg()
        .bg(PANEL)
        .border_1()
        .border_color(BORDER)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            cx.stop_propagation();
            match event.keystroke.key.as_str() {
                "escape" => this.close_dialog(window, cx),
                "enter" => this.confirm_remove_remote(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(RED)
                .child(format!("Remove remote {name}?")),
        )
        .child(label(
            "Its remote-tracking refs (origin/*) are removed with it. This cannot be undone."
                .to_string(),
        ))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(button(
                    "remove-remote-cancel",
                    "Cancel",
                    TEXT,
                    None,
                    Some(BORDER),
                    cx.listener(|this, _, window, cx| cancel(this, window, cx)),
                ))
                .child(button(
                    "remove-remote-confirm",
                    "Remove",
                    rgb(0x11111b),
                    Some(RED),
                    None,
                    cx.listener(|this, _, window, cx| this.confirm_remove_remote(window, cx)),
                )),
        )
}
