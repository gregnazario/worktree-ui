//! Working Copy detail view rendering. Listeners attach against `RootView`
//! (same pattern as dialogs.rs).

use crate::app::RootView;
use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Styled, Window};

/// Placeholder shell for the detail view (real file list and diff panes
/// arrive in Tasks 10–11). Tracks the three detail focus handles so they
/// resolve inside the window's dispatch tree: gpui only dispatches keystrokes
/// along the ancestry of the focused element's node, and an untracked focus
/// handle would fall back to the window root — bypassing `RootView`'s
/// `on_key_down` and silently killing `detail_keydown` routing.
pub fn render(
    this: &mut RootView,
    _window: &mut Window,
    _cx: &mut Context<RootView>,
) -> impl IntoElement {
    gpui::div()
        .id("detail-view")
        .size_full()
        .track_focus(&this.detail_focus)
        .child(
            gpui::div()
                .id("detail-list-placeholder")
                .flex_1()
                .track_focus(&this.detail_list_focus),
        )
        .child(
            gpui::div()
                .id("detail-diff-placeholder")
                .flex_1()
                .track_focus(&this.detail_diff_focus),
        )
}
