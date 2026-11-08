use gitclient::theme;
use gitclient::view::{self, Workspace};
use gitclient_core::cache;
use gitclient_core::cache::keys::Caches;
use gitclient_core::install::menu_entry::Launcher;
use gitclient_core::install::{Channel, exe_path, update};
use gitclient_core::settings::state::{self, AppState};
use gpui_kit::*;
use std::path::PathBuf;

fn main() {
    // git and ssh run this program to ask a question, with the question as its argument.
    // Pass it on to the window that is open, and quit with the answer.
    if let Some(status) = gitclient_core::askpass::run_helper() {
        std::process::exit(status);
    }
    let questions = gitclient_core::askpass::serve();
    // Taken now, before an update can replace the program and change what it is called.
    let exe = exe_path();
    let channel = Channel::detect();
    if channel.is_portable() {
        update::clean_up(&exe);
    }
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
    let check_updates = saved.check_updates;
    // Only a portable copy manages its own launcher; a package puts one in place itself.
    let launcher = channel.is_portable().then(Launcher::this_system).flatten();
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
        let updates = view::install_updates(channel, cx);
        if check_updates {
            updates.update(cx, |updates, cx| updates.check(cx));
        }

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
            // Wayland matches the window to gitclient.desktop by this, for its icon.
            app_id: Some("gitclient".into()),
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
                if let Some(questions) = questions {
                    workspace.answer_questions(questions, cx);
                }
                workspace.welcome(launcher, cx);
                workspace
            })
        })
        .expect("Failed to open window");
    });
}
