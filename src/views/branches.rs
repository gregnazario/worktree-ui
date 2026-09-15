//! Branches section of the worktree detail view: branch list with
//! current/ahead/behind markers, and a stash pane behind `tab`. The
//! section's header reads the BranchStore, not the home list — after a
//! switch the list (and this header) refresh from the store itself.

use crate::app::{RootView, ACCENT, BORDER, DIM, PANEL, RED, ROW_SELECTED, TEXT, YELLOW};
use crate::branch_store::Pane;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, uniform_list, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, Styled, Window,
};

pub fn render(
    this: &mut RootView,
    window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let Some(bs) = this.branch_store.clone() else {
        return div().id("branches-view").into_any_element();
    };
    // Header follows the section's own store: `is_current` comes from the
    // refreshed branch list, so it moves the moment a switch lands.
    let (branch_label, arrows) = {
        let s = bs.read(cx);
        match s.branches.iter().find(|b| b.is_current) {
            Some(b) => {
                let arrows = format!(
                    "{}{}",
                    if b.ahead > 0 {
                        format!("↑{} ", b.ahead)
                    } else {
                        String::new()
                    },
                    if b.behind > 0 {
                        format!("↓{}", b.behind)
                    } else {
                        String::new()
                    }
                );
                (b.short.clone(), arrows)
            }
            None => ("(no branch)".to_string(), String::new()),
        }
    };
    let path = bs.read(cx).worktree.display().to_string();
    let list_focused = this.history_list_focus.is_focused(window);
    let pane = bs.read(cx).pane;
    let branch_count = bs.read(cx).branches.len();
    let stash_count = bs.read(cx).stashes.len();

    let body = div()
        .id("branches-body")
        .flex()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .min_h_0()
        // Subsection header: which list the cursor is in, and how to
        // reach the other one.
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(ACCENT)
                        .child(match pane {
                            Pane::Branches => format!("BRANCHES ({branch_count})"),
                            Pane::Stashes => format!("STASHES ({stash_count})"),
                        }),
                )
                .child(div().text_size(px(10.)).text_color(DIM).child(match pane {
                    Pane::Branches => format!("tab → stashes ({stash_count})"),
                    Pane::Stashes => format!("tab → branches ({branch_count})"),
                })),
        )
        .child(render_active_list(this, cx));

    div()
        .id("branches-view")
        .track_focus(&this.detail_focus)
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
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
                .child(tab_label("2 History", false))
                .child(tab_label("3 Branches", true))
                .child(div().text_size(px(11.)).text_color(DIM).child("esc back")),
        )
        .child(body)
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
                        if !list_focused {
                            "1/2/3 section · esc back".to_string()
                        } else {
                            match pane {
                                Pane::Branches => "↑↓ branch · enter switch · m merge · R rebase · d delete · M rename · n new · y copy · z stash · tab stashes · esc back".to_string(),
                                Pane::Stashes => "↑↓ stash · p pop · a apply · D drop · tab branches · esc back".to_string(),
                            }
                        },
                    ),
                )
                .child(div().flex_1())
                .when_some(
                    bs.read(cx).message.clone(),
                    |f, msg| f.child(div().text_size(px(11.)).text_color(YELLOW).child(msg)),
                ),
        )
        .into_any_element()
}

/// The active pane's list, virtualized so a many-branch repo costs the
/// same per frame as a small one. `up`/`down` reveal the cursor through
/// the shared scroll handle (see `branches_keydown`).
fn render_active_list(this: &mut RootView, cx: &mut Context<RootView>) -> gpui::AnyElement {
    let list_focus = this.history_list_focus.clone();
    let scroll = this.branch_list_scroll.clone();
    let Some(bs) = this.branch_store.clone() else {
        return div().into_any_element();
    };
    let (pane, selected, load_failed, count) = {
        let s = bs.read(cx);
        (
            s.pane,
            s.active_selected(),
            s.load_failed,
            match s.pane {
                Pane::Branches => s.branches.len(),
                Pane::Stashes => s.stashes.len(),
            },
        )
    };
    if load_failed {
        let text = bs.read(cx).message.clone().unwrap_or_default();
        return div()
            .id("branches-list")
            .track_focus(&list_focus)
            .flex_1()
            .min_h_0()
            .p_4()
            .text_size(px(13.))
            .text_color(RED)
            .child(text)
            .into_any_element();
    }
    if count == 0 {
        let text = match pane {
            Pane::Branches => "No branches".to_string(),
            Pane::Stashes => "No stashes — z stashes the working copy".to_string(),
        };
        return div()
            .id("branches-list")
            .track_focus(&list_focus)
            .flex_1()
            .min_h_0()
            .p_4()
            .text_size(px(13.))
            .text_color(DIM)
            .child(text)
            .into_any_element();
    }
    uniform_list("branches-list", count, move |range, _window, cx| {
        range
            .filter(|pos| *pos < count)
            .filter_map(|pos| {
                let (is_selected, label, hint, is_remote) = {
                    let s = bs.read(cx);
                    match pane {
                        Pane::Branches => {
                            let b = s.branches.get(pos)?;
                            (
                                selected == Some(pos),
                                b.short.clone(),
                                String::new(),
                                b.is_remote,
                            )
                        }
                        Pane::Stashes => {
                            let e = s.stashes.get(pos)?;
                            (
                                selected == Some(pos),
                                format!("stash@{{{}}}", e.index),
                                e.message.clone(),
                                false,
                            )
                        }
                    }
                };
                let mut row = div()
                    .id(SharedString::from(format!("b-row-{pos}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .when(is_selected, |r| r.bg(ROW_SELECTED))
                    .on_mouse_down(MouseButton::Left, {
                        let bs = bs.clone();
                        let focus = list_focus.clone();
                        move |_, window, cx| {
                            window.focus(&focus);
                            bs.update(cx, |store, cx| store.select(Some(pos), cx));
                        }
                    });
                // Current-branch marker (branches pane only).
                let (marker, marker_color) = match pane {
                    Pane::Branches => {
                        let current = bs
                            .read(cx)
                            .branches
                            .get(pos)
                            .map(|b| b.is_current)
                            .unwrap_or(false);
                        if current {
                            ("●", ACCENT)
                        } else {
                            (" ", DIM)
                        }
                    }
                    Pane::Stashes => ("⋯", DIM),
                };
                row = row.child(
                    div()
                        .w(px(14.))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(marker_color)
                        .child(marker),
                );
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(if is_selected { TEXT } else { DIM })
                        .child(label),
                );
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(if is_remote { DIM } else { TEXT })
                        .truncate()
                        .child(hint),
                );
                // Remote-tracking rows are labeled as such: switch and
                // delete refuse them, so the label explains why.
                if is_remote {
                    row = row.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(DIM)
                            .child("remote"),
                    );
                }
                Some(row)
            })
            .collect()
    })
    .track_scroll(scroll)
    .flex_1()
    .min_h_0()
    .into_any_element()
}

fn tab_label(text: &str, active: bool) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .text_color(if active { ACCENT } else { DIM })
        .child(text.to_string())
}
