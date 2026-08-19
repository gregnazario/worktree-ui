use gpui::{
    App, AppContext, Application, Bounds, Context, IntoElement, ParentElement, Point, Render,
    Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(960.), px(640.)),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|_| Hello)).unwrap();
    });
}

struct Hello;

impl Render for Hello {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .justify_center()
            .items_center()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child("worktree-tool")
    }
}
