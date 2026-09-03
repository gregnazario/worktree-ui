//! Working Copy detail view rendering. Listeners attach against `RootView`
//! (same pattern as dialogs.rs).

use crate::app::{RootView, ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT, YELLOW};
use crate::engine::diff::{self, DiffLineKind};
use crate::engine::working_copy::Group;
use crate::wc_store::FileDetail;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, rgba, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window,
};

/// Caps rendered diff lines in the detail view's diff pane.
const DIFF_RENDER_CAP: usize = 5000;
/// Caps the interactive file-list rows: every row is a stateful element,
/// and monorepo-scale lists would make each keystroke rebuild thousands of
/// them. Truncated lists show a trailer pointing at the terminal.
/// Kept in sync with the store's selection clamp — never select an
/// undrawn row.
const FILE_ROW_RENDER_CAP: usize = crate::wc_store::MAX_VISIBLE_ROWS;

/// One visible file-list row: (group, entry index, selected?, status letter
/// for this surface, display path, +/- line counts).
type FileRow = (Group, usize, bool, char, String, Option<(u64, u64)>);

/// The full detail view: header (branch + ahead/behind + path + tabs), file
/// list pane, diff pane, footer (hints + author + message).
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
                let mut path = match &e.orig_path {
                    Some(old) => format!("{old} → {}", e.path),
                    None => e.path.clone(),
                };
                if e.unsupported {
                    // The name git gave us was lossy-decoded; show that it
                    // can't be acted on instead of presenting a fake path.
                    path.push_str("  (non-UTF-8 name — unsupported)");
                }
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
    // `wc` is None until the first status snapshot lands — that's
    // "loading", not "clean".
    let loading = wc.read(cx).wc.is_none();
    let body = div()
        .id("wc-body")
        .flex()
        .flex_1()
        .min_h_0()
        .child(render_file_list(this, cx, rows, loading))
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
    loading: bool,
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
    let total = rows.len();
    let is_empty = total == 0;
    for (pos, (group, _i, is_selected, letter, path, counts)) in rows.into_iter().enumerate() {
        if pos >= FILE_ROW_RENDER_CAP {
            break; // cap the interactive elements; a trailer reports the rest
        }
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
        // An empty row list means "clean" ONLY once the first status
        // snapshot has landed — before that it means "still loading", and
        // the two must not be conflated.
        let text = if loading {
            "Loading working copy…"
        } else {
            "Working tree clean"
        };
        list = list.child(div().p_4().text_size(px(13.)).text_color(DIM).child(text));
    } else if total > FILE_ROW_RENDER_CAP {
        let hidden = total - FILE_ROW_RENDER_CAP;
        list = list.child(
            div()
                .px_3()
                .py_2()
                .text_size(px(12.))
                .text_color(DIM)
                .child(format!(
                    "… {hidden} more files — stage them from the terminal to narrow the list"
                )),
        );
    }
    list
}

/// The right pane: unified diff rows for the selected file, preview text for
/// untracked/conflicted rows, placeholders for binary/missing/failed. Must
/// keep tracking the diff focus handle so tab routing and `detail_keydown`
/// keep dispatching.
fn render_diff_pane(this: &mut RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let diff_focus = this.detail_diff_focus.clone();
    let mut pane = div()
        .id("wc-diff")
        .track_focus(&diff_focus)
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, _cx| {
                window.focus(&this.detail_diff_focus);
            }),
        );
    let Some(wc) = this.detail.clone() else {
        return pane;
    };
    let store = wc.read(cx);
    let Some(detail) = &store.detail else {
        return pane.child(
            div()
                .p_4()
                .text_size(px(13.))
                .text_color(DIM)
                .child("No selection"),
        );
    };
    if matches!(store.selected_row(), Some((Group::Conflicts, _))) {
        pane = pane.child(placeholder(
            "Resolve in your editor, then press s to mark resolved",
        ));
    }
    let transparent = rgba(0x00000000);
    match detail {
        FileDetail::Diff(ud) if ud.binary => pane.child(placeholder("Binary file — not shown")),
        FileDetail::Diff(ud) => {
            let total: usize = ud.hunks.iter().map(|h| h.lines.len()).sum();
            let mut rendered = 0usize;
            pane = pane.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(DIM)
                    .child(
                        ud.header
                            .lines()
                            .filter(|l| !l.is_empty())
                            .map(String::from)
                            .collect::<Vec<_>>()
                            .join("  ·  "),
                    ),
            );
            for hunk in &ud.hunks {
                if rendered >= DIFF_RENDER_CAP {
                    break;
                }
                pane = pane.child(
                    div()
                        .px_3()
                        .py_0p5()
                        .text_size(px(11.))
                        .text_color(DIM)
                        .child(hunk.header.clone()),
                );
                for line in &hunk.lines {
                    if rendered >= DIFF_RENDER_CAP {
                        break;
                    }
                    rendered += 1;
                    let (marker, color, bg) = match line.kind {
                        DiffLineKind::Add => ("+", GREEN, rgba(0xa6e3a120)),
                        DiffLineKind::Del => ("−", RED, rgba(0xf38ba820)),
                        DiffLineKind::Context => (" ", TEXT, transparent),
                    };
                    let row = div()
                        .flex()
                        .px_3()
                        .text_size(px(12.))
                        .when(bg != transparent, |r| r.bg(bg));
                    pane = pane.child(
                        row.child(
                            div()
                                .w(px(14.))
                                .flex_shrink_0()
                                .text_color(color)
                                .child(marker),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .child(if line.no_newline {
                                    format!("{}\\ (no newline)", line.content)
                                } else {
                                    line.content.clone()
                                }),
                        ),
                    );
                }
            }
            if total > DIFF_RENDER_CAP {
                pane = pane.child(placeholder(&format!(
                    "… {} more lines — open the file in your editor",
                    total - DIFF_RENDER_CAP
                )));
            }
            pane
        }
        FileDetail::Preview(p) => match p {
            diff::Preview::Text { content, truncated } => {
                let lines: Vec<&str> = content.lines().collect();
                let shown = lines.len().min(DIFF_RENDER_CAP);
                for l in lines.iter().take(shown) {
                    pane = pane.child(
                        div()
                            .px_3()
                            .text_size(px(12.))
                            .text_color(TEXT)
                            .child(l.to_string()),
                    );
                }
                if *truncated || lines.len() > DIFF_RENDER_CAP {
                    pane = pane.child(placeholder("… truncated — open the file in your editor"));
                }
                pane
            }
            diff::Preview::Binary => pane.child(placeholder("Binary file — not shown")),
            diff::Preview::Directory => pane.child(placeholder(
                "Untracked directory — press S to stage its contents",
            )),
            diff::Preview::Missing => pane.child(placeholder("File missing on disk")),
        },
        FileDetail::Failed(msg) => pane.child(
            div()
                .p_4()
                .text_size(px(12.))
                .text_color(RED)
                .child(msg.clone()),
        ),
    }
}

fn placeholder(text: &str) -> impl IntoElement {
    div()
        .p_4()
        .text_size(px(13.))
        .text_color(DIM)
        .child(text.to_string())
}
