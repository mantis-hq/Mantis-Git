use gitclient::theme;
use gitclient::view::{self, Workspace};
use gitclient_core::cache;
use gitclient_core::cache::keys::Caches;
use gitclient_core::settings::state::{self, AppState};
use gpui_kit::*;
use std::path::PathBuf;

fn main() {
    // Each path on the command line gets a tab. With none, the repositories left open
    // last time come back, and failing that the current folder opens.
    let given: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let state_file = state::default_file();
    let saved = state_file
        .as_deref()
        .map(AppState::load)
        .unwrap_or_default();
    let startup = saved.startup(&given, std::env::current_dir().ok());
    let dock = saved.dock;
    // Without a cache folder the caches still work, from memory.
    let caches = match cache::default_dir() {
        Some(root) => Caches::on_disk(root, saved.disk_settings()),
        None => Caches::memory_only(),
    };

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        view::bind_keys(cx);
        view::install_caches(caches, cx);

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
                for repo in startup.tabs {
                    workspace.open(repo, window, cx);
                }
                workspace.activate(startup.active, window, cx);
                workspace.restore_dock(dock, cx);
                // Only now, so that opening the tabs one by one doesn't overwrite the rest.
                workspace.remember_in(state_file, cx);
                workspace
            })
        })
        .expect("Failed to open window");
    });
}
