//! Modal dialog state + rendering. Dialogs are plain state on `RootView`
//! (not separate entities); the render helpers below take the root view
//! directly and attach listeners against it.

use crate::terminal::{self, InstalledTerminal};
use crate::text_field::TextField;
use crate::ui::RootView;
use crate::ui::{ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT};
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
            store.add(PathBuf::from(&dest_value), branch_arg, base_arg, cx)
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

    // Automatic (auto-detect) row.
    let auto_is_selected = selected.is_none();
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
        let is_selected = selected.as_deref() == Some(id);
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
        .child(div().flex().justify_end().child(button(
            "settings-close",
            "Done",
            rgb(0x11111b),
            Some(GREEN),
            None,
            cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
        )));
    card
}
