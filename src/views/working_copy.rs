//! Working Copy detail view rendering. Listeners attach against `RootView`
//! (same pattern as dialogs.rs).

use crate::app::{RootView, ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, YELLOW};
use crate::engine::working_copy::Group;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

/// Caps rendered diff lines. Wired into the diff pane by Task 11.
#[allow(dead_code)]
const DIFF_RENDER_CAP: usize = 5000;

/// One visible file-list row: (group, entry index, selected?, status letter
/// for this surface, display path, +/- line counts).
type FileRow = (Group, usize, bool, char, String, Option<(u64, u64)>);

/// The full detail view: header (branch + ahead/behind + path + tabs), file
/// list pane, diff placeholder (Task 11), footer (hints + author + message).
///
/// LOAD-BEARING: all three detail focus handles (`detail_focus`,
/// `detail_list_focus`, `detail_diff_focus`) MUST be `.track_focus`ed
/// somewhere in the rendered tree. gpui 0.2.2 only dispatches keystrokes
/// along the ancestry of the focused element's node; an untracked focused
/// handle falls back to the window root — bypassing `RootView`'s
/// `on_key_down` and silently killing `detail_keydown` routing.
pub fn render(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let Some(wc) = this.detail.clone() else {
        return div().id("detail-view").into_any_element();
    };
    let (branch_label, arrows, path) = {
        let store = wc.read(cx);
        let branch = store
            .wc
            .as_ref()
            .map(|w| w.branch.head.clone())
            .unwrap_or_else(|| "…".into());
        let (ahead, behind) = store
            .wc
            .as_ref()
            .map(|w| (w.branch.ahead, w.branch.behind))
            .unwrap_or((0, 0));
        let mut arrows = String::new();
        if ahead > 0 {
            arrows.push_str(&format!("↑{ahead} "));
        }
        if behind > 0 {
            arrows.push_str(&format!("↓{behind}"));
        }
        (branch, arrows, store.worktree.display().to_string())
    };
    // One row per visible list entry: (group, entry index, selected?, status
    // letter for THIS surface, display path, +/- line counts). Staged rows
    // show the index status letter, Unstaged rows the worktree letter.
    let rows: Vec<FileRow> = {
        let store = wc.read(cx);
        store
            .rows()
            .iter()
            .enumerate()
            .map(|(pos, (group, i))| {
                let e = &store.wc.as_ref().expect("rows implies wc").entries[*i];
                let letter = match group {
                    Group::Staged => e.index_status,
                    Group::Unstaged => e.wt_status,
                    Group::Conflicts => 'U',
                    Group::Untracked => '?',
                };
                let counts = match group {
                    Group::Staged => e.staged_lines,
                    Group::Unstaged => e.unstaged_lines,
                    _ => None,
                };
                let path = match &e.orig_path {
                    Some(old) => format!("{old} → {}", e.path),
                    None => e.path.clone(),
                };
                (
                    *group,
                    *i,
                    pos == store.selected.unwrap_or(usize::MAX),
                    letter,
                    path,
                    counts,
                )
            })
            .collect()
    };
    let author = wc.read(cx).author.clone();
    let message = wc.read(cx).message.clone();

    let container_focus = this.detail_focus.clone();
    let body = div()
        .id("wc-body")
        .flex()
        .flex_1()
        .min_h_0()
        .child(render_file_list(this, cx, rows))
        .child(render_diff_pane(this, cx));

    div()
        .id("detail-view")
        .track_focus(&container_focus)
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        // ---- header: branch, arrows, path, tabs, back hint ----
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(format!("{branch_label} {arrows}")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .text_color(DIM)
                        .child(path),
                )
                .child(tab_label("1 Working Copy", true))
                .child(tab_label("2 History — v0.3", false))
                .child(tab_label("3 Branches — v0.4", false))
                .child(div().text_size(px(11.)).text_color(DIM).child("esc back")),
        )
        // ---- body: files | diff ----
        .child(body)
        // ---- footer: hints + author + message ----
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(BORDER)
                .bg(PANEL)
                .child(
                    div().text_size(px(11.)).text_color(DIM).child(
                        "↑↓ move · s stage/unstage · S stage all · d discard · c commit · tab pane · r refresh · t terminal · esc back"
                            .to_string(),
                    ),
                )
                .child(div().flex_1())
                .when_some(author, |f, (name, email)| {
                    f.child(
                        div()
                            .text_size(px(11.))
                            .text_color(DIM)
                            .child(format!("{name} <{email}>")),
                    )
                })
                .when_some(message, |f, msg| {
                    f.child(div().text_size(px(11.)).text_color(YELLOW).child(msg))
                }),
        )
        .into_any_element()
}

fn tab_label(text: &str, active: bool) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .text_color(if active { ACCENT } else { DIM })
        .child(text.to_string())
}

fn status_color(c: char) -> gpui::Rgba {
    match c {
        'A' => GREEN,
        'M' | 'R' | 'C' => YELLOW,
        'D' | 'U' => RED,
        _ => DIM,
    }
}

fn group_header(title: &str) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(DIM)
        .child(title.to_string())
}

fn render_file_list(
    this: &mut RootView,
    cx: &mut Context<RootView>,
    rows: Vec<FileRow>,
) -> impl IntoElement {
    let list_focus = this.detail_list_focus.clone();
    let mut last_group: Option<Group> = None;
    let mut list = div()
        .id("wc-files")
        .track_focus(&list_focus)
        .w(px(340.))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .border_r_1()
        .border_color(BORDER)
        .overflow_y_scroll();
    let wc_entity = this.detail.clone();
    let is_empty = rows.is_empty();
    for (pos, (group, _i, is_selected, letter, path, counts)) in rows.into_iter().enumerate() {
        if last_group != Some(group) {
            list = list.child(group_header(group.title()));
            last_group = Some(group);
        }
        let wc_clone = wc_entity.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("wc-row-{pos}")))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .when(is_selected, |row| row.bg(ROW_SELECTED))
                .child(
                    div()
                        .w(px(12.))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(status_color(letter))
                        .child(letter.to_string()),
                )
                .child(div().flex_1().min_w_0().text_size(px(12.)).child(path))
                .when_some(counts, |row, (a, d)| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .child(div().text_color(GREEN).child(format!("+{a}")))
                            .child(div().text_color(RED).child(format!("−{d}"))),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if let Some(wc) = &wc_clone {
                            wc.update(cx, |store, cx| store.select(Some(pos), cx));
                        }
                        window.focus(&this.detail_list_focus);
                    }),
                ),
        );
    }
    if is_empty {
        list = list.child(
            div()
                .p_4()
                .text_size(px(13.))
                .text_color(DIM)
                .child("Working tree clean"),
        );
    }
    list
}

/// Placeholder right pane (Task 11 fills it in). Must exist and track the
/// diff focus handle so tab routing and `detail_keydown` keep dispatching.
fn render_diff_pane(this: &mut RootView, _cx: &mut Context<RootView>) -> impl IntoElement {
    let diff_focus = this.detail_diff_focus.clone();
    div()
        .id("wc-diff")
        .track_focus(&diff_focus)
        .flex_1()
        .min_w_0()
        .bg(PANEL)
}
