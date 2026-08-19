use gpui::{
    App, Application, Bounds, KeyBinding, Point, TitlebarOptions, WindowBounds, WindowOptions,
    px, size,
};
use worktree_tool::store::WorktreeStore;
use worktree_tool::ui::{FocusSearch, NewWorktree, Quit, Refresh, RootView};

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewWorktree, Some("Root")),
            KeyBinding::new("cmd-r", Refresh, Some("Root")),
            KeyBinding::new("cmd-f", FocusSearch, Some("Root")),
        ]);

        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(960.), px(640.)),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Worktree Tool".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            let store = WorktreeStore::new(cx);
            RootView::new(store, window, cx)
        })
        .unwrap();

        // Keep the app-lifetime subscriptions alive for the whole process.
        std::mem::forget(cx.on_window_closed(|cx| cx.quit()));
        cx.on_action(|_: &Quit, cx| cx.quit());
    });
}
