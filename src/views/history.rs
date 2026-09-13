//! History section of the worktree detail view: commit list with text
//! graph, selected commit's changed files, and the selected file's diff.
//! Read-only — staging lives in the Working Copy section.

use crate::app::{RootView, ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT, YELLOW};
use crate::engine::diff::{DiffLineKind, UnifiedDiff};
use crate::engine::history::GraphCell;
use crate::views::working_copy::tab_label;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, rgba, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window,
};

/// Diff lines rendered per commit-detail pane.
const DIFF_RENDER_CAP: usize = 5000;

pub fn render(
    this: &mut RootView,
    window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let Some(hs) = this.history.clone() else {
        return div().id("history-view").into_any_element();
    };
    let (branch_label, arrows) = {
        let branch = this
            .store
            .read(cx)
            .selected_entry()
            .and_then(|e| e.branch.clone())
            .unwrap_or_else(|| "…".into());
        let (ahead, behind) = match this.store.read(cx).selected_entry() {
            Some(entry) => match &entry.status {
                crate::model::WorktreeStatus::Clean { ahead, behind }
                | crate::model::WorktreeStatus::Dirty { ahead, behind, .. } => (*ahead, *behind),
                _ => (0, 0),
            },
            None => (0, 0),
        };
        let arrows = format!(
            "{}{}",
            if ahead > 0 {
                format!("↑{ahead} ")
            } else {
                String::new()
            },
            if behind > 0 {
                format!("↓{behind}")
            } else {
                String::new()
            }
        );
        (branch, arrows)
    };
    let path = hs.read(cx).worktree.display().to_string();

    let list_focused = this.history_list_focus.is_focused(window);
    let files_focused = this.history_files_focus.is_focused(window);

    let body = div()
        .id("history-body")
        .flex()
        .flex_1()
        .min_h_0()
        .child(render_commit_list(this, cx))
        .child(render_detail(this, cx));

    div()
        .id("history-view")
        .track_focus(&this.detail_focus)
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
                .child(tab_label("1 Working Copy", false))
                .child(tab_label("2 History", true))
                .child(tab_label("3 Branches — v0.4", false))
                .child(div().text_size(px(11.)).text_color(DIM).child("esc back")),
        )
        // ---- body: commits | files+diff ----
        .child(body)
        // ---- footer: hints + message ----
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
                        if files_focused {
                            "↑↓ file · tab back to commits · S stage all · 1/2 section · esc back"
                                .to_string()
                        } else if list_focused {
                            "↑↓ commit · y copy hash · x checkout · w new worktree · L load more · r refresh · t terminal · 1/2 section · esc back".to_string()
                        } else {
                            String::new()
                        },
                    ),
                )
                .child(div().flex_1())
                .when_some(
                    hs.read(cx).message.clone(),
                    |f, msg| f.child(div().text_size(px(11.)).text_color(YELLOW).child(msg)),
                ),
        )
        .into_any_element()
}

/// The commit list: graph cells, short hash, subject, author + age.
fn render_commit_list(this: &mut RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let list_focus = this.history_list_focus.clone();
    let Some(hs) = this.history.clone() else {
        return div().into_any_element();
    };
    let loading = hs.read(cx).commits.is_empty() && !hs.read(cx).load_failed;
    let mut list = div()
        .id("history-commits")
        .track_focus(&list_focus)
        .w(px(430.))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .border_r_1()
        .border_color(BORDER)
        .overflow_y_scroll();
    if hs.read(cx).load_failed {
        let text = hs.read(cx).message.clone().unwrap_or_default();
        return list
            .child(div().p_4().text_size(px(13.)).text_color(DIM).child(text))
            .into_any_element();
    }
    if loading {
        return list
            .child(
                div()
                    .p_4()
                    .text_size(px(13.))
                    .text_color(DIM)
                    .child("Loading history…"),
            )
            .into_any_element();
    }
    let now = now_secs();
    let count = hs.read(cx).commits.len();
    let selected = hs.read(cx).selected;
    for pos in 0..count {
        let (cells, lane, short, subject, author, timestamp, refs) = {
            let s = hs.read(cx);
            let c = &s.commits[pos];
            let cells = s.rows[pos].cells.clone();
            (
                cells,
                s.rows[pos].lane,
                c.short.clone(),
                c.subject.clone(),
                c.author.clone(),
                c.timestamp,
                c.refs.clone(),
            )
        };
        let is_selected = selected == Some(pos);
        let mut row = div()
            .id(SharedString::from(format!("h-row-{pos}")))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .when(is_selected, |r| r.bg(ROW_SELECTED));
        // Graph: one fixed-width cell per lane, so alignment never
        // depends on the font.
        for (i, cell) in cells.iter().enumerate() {
            let (ch, color) = match cell {
                GraphCell::Commit => ("*", if i == lane { ACCENT } else { DIM }),
                GraphCell::Wire => ("│", DIM),
                GraphCell::Empty => (" ", rgba(0x00000000)),
            };
            row = row.child(
                div()
                    .w(px(14.))
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(color)
                    .child(ch),
            );
        }
        let _ = lane;
        row = row
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(DIM)
                    .child(short.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(if is_selected { TEXT } else { DIM })
                    .child(subject),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(DIM)
                    .child(format!("{} · {}", author, age_label(timestamp, now))),
            );
        if !refs.is_empty() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(ACCENT)
                    .child(refs.clone()),
            );
        }
        let hs_clone = hs.clone();
        list = list.child(row.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                window.focus(&this.history_list_focus);
                hs_clone.update(cx, |store, cx| store.select(Some(pos), cx));
            }),
        ));
    }
    let more = hs.read(cx).max_count;
    if count >= more {
        list = list.child(
            div()
                .px_3()
                .py_2()
                .text_size(px(11.))
                .text_color(DIM)
                .child(format!("… older commits — L loads {more} more")),
        );
    }
    list.into_any_element()
}

/// The right column: the selected commit's changed files above the
/// selected file's diff.
fn render_detail(this: &mut RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let files_focus = this.history_files_focus.clone();
    let mut pane = div()
        .id("history-detail")
        .track_focus(&files_focus)
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, _cx| {
                window.focus(&this.history_files_focus);
            }),
        );
    let Some(hs) = this.history.clone() else {
        return pane.into_any_element();
    };
    let store = hs.read(cx);
    let Some(files) = &store.files else {
        return pane
            .child(placeholder("Loading commit…"))
            .into_any_element();
    };
    if files.is_empty() {
        return pane
            .child(placeholder("Empty commit — no files changed"))
            .into_any_element();
    }
    // Files strip: horizontal chips keep the diff beside the list.
    let mut chips = div().flex().flex_wrap().gap_1().px_3().py_2();
    for (i, file) in files.iter().enumerate() {
        let is_selected = store.selected_file == Some(i);
        chips = chips.child(
            div()
                .id(SharedString::from(format!("h-file-{i}")))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .when(is_selected, |c| c.bg(ROW_SELECTED))
                .when(!is_selected, |c| c.border_1())
                .when(!is_selected, |c| c.border_color(BORDER))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        window.focus(&this.history_files_focus);
                        if let Some(hs) = &this.history {
                            hs.update(cx, |store, cx| store.select_file(Some(i), cx));
                        }
                    }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(letter_color(file.letter))
                        .child(file.letter.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .min_w_0()
                        .child(match &file.orig_path {
                            Some(orig) => format!("{orig} → {}", file.path),
                            None => file.path.clone(),
                        }),
                ),
        );
    }
    pane = pane.child(chips);
    match &store.file_diff {
        Err(text) if text.is_empty() => {}
        Err(text) => {
            pane = pane.child(placeholder(text));
        }
        Ok(ud) => {
            pane = pane.child(render_diff(ud));
        }
    }
    pane.into_any_element()
}

fn render_diff(ud: &UnifiedDiff) -> impl IntoElement {
    let transparent = rgba(0x00000000);
    let mut pane = div().flex().flex_col().pb_2();
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
    if ud.hunks.is_empty() && !ud.binary {
        pane = pane.child(placeholder("No content changes (mode/rename only)"));
        return pane.into_any_element();
    }
    if ud.binary {
        pane = pane.child(placeholder("Binary file — not shown"));
        return pane.into_any_element();
    }
    let mut rendered = 0usize;
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
            pane = pane.child(
                div()
                    .flex()
                    .px_3()
                    .text_size(px(12.))
                    .when(bg != transparent, |r| r.bg(bg))
                    .child(
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
    let total: usize = ud.hunks.iter().map(|h| h.lines.len()).sum();
    if total > DIFF_RENDER_CAP {
        pane = pane.child(placeholder(&format!(
            "… {} more lines — open the file in your editor",
            total - DIFF_RENDER_CAP
        )));
    }
    pane.into_any_element()
}

fn placeholder(text: &str) -> impl IntoElement {
    div()
        .p_4()
        .text_size(px(13.))
        .text_color(DIM)
        .child(text.to_string())
}

fn letter_color(letter: char) -> gpui::Rgba {
    match letter {
        'A' => GREEN,
        'D' => RED,
        _ => YELLOW,
    }
}

/// Rough relative age for the commit list: "now", "5m", "3h", "2d",
/// "6w", else the date.
fn age_label(timestamp: i64, now: i64) -> String {
    let secs = (now - timestamp).max(0);
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    match secs {
        s if s < MIN => "now".into(),
        s if s < HOUR => format!("{}m", s / MIN),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < 14 * DAY => format!("{}d", s / DAY),
        s if s < 365 * DAY => format!("{}w", s / (7 * DAY)),
        _ => format!("{}y", secs / (365 * DAY)),
    }
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::age_label;

    #[test]
    fn age_label_buckets() {
        let now = 1_700_000_000;
        assert_eq!(age_label(now, now), "now");
        assert_eq!(age_label(now - 300, now), "5m");
        assert_eq!(age_label(now - 7_200, now), "2h");
        assert_eq!(age_label(now - 3 * 86_400, now), "3d");
        assert_eq!(age_label(now - 14 * 86_400, now), "2w");
        assert_eq!(age_label(now - 400 * 86_400, now), "1y");
    }
}
