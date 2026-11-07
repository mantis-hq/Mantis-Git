use gitclient::theme;
use gitclient::view::{self, Workspace};
use gpui_kit::*;
use std::path::PathBuf;

fn main() {
    // Each path on the command line gets a tab; with none, the current folder does.
    let mut repos: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if repos.is_empty() {
        repos.extend(std::env::current_dir().ok());
    }

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        view::bind_keys(cx);

        // Settings is an add-on to the main window, so closing it alone keeps the app open.
        cx.on_window_closed(|cx, _| {
            if view::should_quit(cx) {
                cx.quit()
            }
        })
        .detach();

        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("gitclient".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::centered(size(px(1200.), px(800.)), cx)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            theme::follow_system(window, cx);
            cx.new(|cx| {
                let mut workspace = Workspace::new(window, cx);
                for repo in repos {
                    workspace.open(repo, window, cx);
                }
                workspace
            })
        })
        .expect("Failed to open window");
    });
}
