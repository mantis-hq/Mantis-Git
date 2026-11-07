use gitclient::view::{self, GraphView};
use gpui_kit::*;
use std::path::PathBuf;

fn main() {
    let repo = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        view::bind_keys(cx);

        cx.on_window_closed(|cx, _| cx.quit()).detach();

        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("gitclient".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::centered(size(px(1200.), px(800.)), cx)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                let mut view = GraphView::new(window, cx);
                if let Some(repo) = repo {
                    view.open(repo, cx);
                }
                view
            })
        })
        .expect("Failed to open window");
    });
}
