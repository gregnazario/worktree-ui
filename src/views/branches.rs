//! Branches section of the worktree detail view: branch list with
//! current/ahead/behind markers, and branch management actions.

use crate::app::{RootView, ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT, YELLOW};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

pub fn render(
    this: &mut RootView,
    window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let Some(bs) = this.branch_store.clone() else {
        return div().id("branches-view").into_any_element();
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
    let path = bs.read(cx).worktree.display().to_string();

    let list_focused = this.history_list_focus.is_focused(window);

    let body = div()
        .id("branches-body")
        .flex()
        .flex_1()
        .min_h_0()
        .child(render_branch_list(this, cx));

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
                        if list_focused {
                            "↑↓ branch · enter switch · d delete · m merge · R rebase · y copy · 1/2/3 section · esc back".to_string()
                        } else {
                            "1/2/3 section · esc back".to_string()
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

fn render_branch_list(this: &mut RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let list_focus = this.history_list_focus.clone();
    let Some(bs) = this.branch_store.clone() else {
        return div().into_any_element();
    };
    let (branches, selected, load_failed) = {
        let s = bs.read(cx);
        (s.branches.clone(), s.selected, s.load_failed)
    };
    let mut list = div()
        .id("branches-list")
        .track_focus(&list_focus)
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_y_scroll();
    if load_failed {
        let text = bs.read(cx).message.clone().unwrap_or_default();
        return list
            .child(div().p_4().text_size(px(13.)).text_color(RED).child(text))
            .into_any_element();
    }
    if branches.is_empty() {
        return list
            .child(
                div()
                    .p_4()
                    .text_size(px(13.))
                    .text_color(DIM)
                    .child("No branches"),
            )
            .into_any_element();
    }
    for (pos, branch) in branches.iter().enumerate() {
        let is_selected = selected == Some(pos);
        let mut row = div()
            .id(SharedString::from(format!("b-row-{pos}")))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .when(is_selected, |r| r.bg(ROW_SELECTED))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.history_list_focus);
                    if let Some(bs) = &this.branch_store {
                        bs.update(cx, |store, cx| store.select(Some(pos), cx));
                    }
                }),
            );
        // Current branch marker
        row = row.child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .text_size(px(12.))
                .text_color(if branch.is_current { ACCENT } else { DIM })
                .child(if branch.is_current { "●" } else { " " }),
        );
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(if branch.is_current { TEXT } else { DIM })
                .child(branch.short.clone()),
        );
        // Ahead/behind
        if branch.ahead > 0 || branch.behind > 0 {
            let arrows = format!(
                "{}{}",
                if branch.ahead > 0 {
                    format!("↑{}", branch.ahead)
                } else {
                    String::new()
                },
                if branch.behind > 0 {
                    format!(" ↓{}", branch.behind)
                } else {
                    String::new()
                }
            );
            if !arrows.is_empty() {
                row = row.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(YELLOW)
                        .child(arrows),
                );
            }
        }
        list = list.child(row);
    }
    list.into_any_element()
}

fn tab_label(text: &str, active: bool) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .text_color(if active { ACCENT } else { DIM })
        .child(text.to_string())
}
