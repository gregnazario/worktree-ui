use crate::dialogs::{self, DialogState};
use crate::model::WorktreeEntry;
use crate::model::WorktreeStatus;
use crate::platform;
use crate::store::WorktreeStore;
use crate::terminal;
use crate::text_field::TextField;
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

pub struct RootView {
    pub store: Entity<WorktreeStore>,
    pub search: Entity<TextField>,
    /// Path input shown in the empty state (no repo detected).
    pub path_input: Entity<TextField>,
    pub dialog: DialogState,
    pub root_focus: FocusHandle,
    pub dialog_focus: FocusHandle,
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
        window.focus(&root_focus);
        let view = cx.new(|_| Self {
            store,
            search,
            path_input,
            dialog: DialogState::None,
            root_focus,
            dialog_focus,
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
        window.focus(&self.root_focus);
        cx.notify();
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

    pub fn open_create_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.read(cx).repo_root.is_none() {
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
        // edits the destination directly.
        cx.observe(&branch.clone(), move |this, field, cx| {
            if let DialogState::Create {
                dest, dest_edited, ..
            } = &mut this.dialog
            {
                if !*dest_edited {
                    let branch = field.read(cx).value.trim().to_string();
                    if !branch.is_empty() {
                        if let Some(root) = this.store.read(cx).repo_root.clone() {
                            let path = crate::model::default_worktree_path(&root, &branch);
                            let display = path.display().to_string();
                            dest.update(cx, |dest, cx| dest.set_value(&display, cx));
                        }
                    }
                }
            }
            cx.notify();
        })
        .detach();
        cx.observe(&dest.clone(), move |this, _field, _cx| {
            if let DialogState::Create { dest_edited, .. } = &mut this.dialog {
                *dest_edited = true;
            }
        })
        .detach();

        self.dialog = DialogState::Create {
            branch,
            base,
            dest,
            new_branch: true,
            dest_edited: false,
        };
        window.focus(&self.dialog_focus);
        cx.notify();
    }

    fn open_remove_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.store.read(cx).selected_entry().cloned() else {
            return;
        };
        let dirty = !matches!(entry.status, WorktreeStatus::Clean { .. });
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

    fn open_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    window.focus(&this.root_focus);
                } else if ks.key == "enter" {
                    window.focus(&this.root_focus);
                }
                return;
            }
            if ks.modifiers.control || ks.modifiers.platform || ks.modifiers.alt {
                return;
            }
            match ks.key.as_str() {
                "up" => this.store.update(cx, |store, cx| store.select_prev(cx)),
                "down" => this.store.update(cx, |store, cx| store.select_next(cx)),
                "enter" => {
                    if let Some(entry) = this.store.read(cx).selected_entry() {
                        let path = entry.path.clone();
                        terminal::open_in_terminal(&path);
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
                                store.update(cx, |store, cx| {
                                    store.load_repo_from_user_path(PathBuf::from(&value), cx)
                                });
                            }
                        })),
                )
        } else {
            div()
                .id("main")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
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
                            cx.listener(|this, _, window, cx| {
                                this.open_settings_dialog(window, cx)
                            }),
                        )),
                )
                .child(
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
                                "Open in Terminal ⏎",
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
}
