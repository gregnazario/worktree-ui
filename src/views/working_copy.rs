//! Working Copy detail view rendering. Listeners attach against `RootView`
//! (same pattern as dialogs.rs).

use crate::app::RootView;
use gpui::{Context, InteractiveElement, IntoElement, Styled, Window};

pub fn render(
    _this: &mut RootView,
    _window: &mut Window,
    _cx: &mut Context<RootView>,
) -> impl IntoElement {
    gpui::div().id("detail-view").size_full()
}
