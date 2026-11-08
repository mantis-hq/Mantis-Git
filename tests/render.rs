//! Renders a real repository through the GPU pipeline and checks the pixels.
//!
//! Needs a working renderer, so it isn't part of `cargo test`. Run it with
//! `cargo test --test render`. Set `RENDER_OUTPUT=dir` to keep the frames as PNGs.

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    render::run();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    println!("render: skipped; GPUI has no headless renderer on this platform");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod render {
    use gitclient::theme;
    use gitclient::view::{self, GraphView, Status};
    use gitclient_core::testing::{Repo, git};
    use gpui_kit::assets::Assets;
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Entity, HeadlessAppContext, WindowBounds,
        WindowOptions, px, size,
    };
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Where the graph is drawn, inside the row area: below the toolbar and header.
    const GRAPH_LEFT: u32 = 0;
    const GRAPH_RIGHT: u32 = 60;
    const LIST_TOP: u32 = 72;

    fn commit(repo: &Path, subject: &str) {
        let name: String = subject
            .chars()
            .filter(char::is_ascii_alphabetic)
            .take(12)
            .collect();
        let code: String = (0..40)
            .map(|line| format!("pub fn step_{line}(input: u32) -> u32 {{ input + {line} }}\n"))
            .collect();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join(format!("src/{name}.rs")), code).unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", subject]);
    }

    fn fixture() -> Repo {
        let fixture = Repo::new();
        let repo = fixture.path();
        commit(repo, "Initial commit");
        commit(repo, "Add the storage layer");
        git(repo, &["tag", "-a", "v0.1.0", "-m", "first release"]);
        git(repo, &["branch", "feature/search"]);
        git(repo, &["branch", "fix/crash"]);
        commit(repo, "Document the storage format");

        git(repo, &["checkout", "-q", "feature/search"]);
        commit(repo, "Index commits for search");
        commit(repo, "Search by author");

        git(repo, &["checkout", "-q", "fix/crash"]);
        commit(repo, "Fix a crash on empty repositories");

        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "Merge fix/crash",
                "fix/crash",
            ],
        );
        commit(repo, "Tidy up error handling");
        git(
            repo,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "Merge feature/search",
                "feature/search",
            ],
        );
        git(repo, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);

        git(repo, &["checkout", "-q", "-b", "wip"]);
        commit(
            repo,
            "Work in progress with a rather long subject line that goes on and on, far past where any sensible commit subject would stop, to show how it gets truncated in the window",
        );
        git(repo, &["checkout", "-q", "main"]);
        commit(repo, "Bump the version");
        git(repo, &["checkout", "-q", "wip"]);
        fixture
    }

    /// An app on the real renderer, in the given appearance.
    fn app(mode: ThemeMode) -> HeadlessAppContext {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            view::bind_keys(cx);
            Theme::change(mode, None, cx);
        });
        cx
    }

    fn open(cx: &mut HeadlessAppContext, repo: &Path) -> (AnyWindowHandle, Entity<GraphView>) {
        let repo = repo.to_owned();
        let (handle, view) = cx
            .update(|cx| {
                let bounds = Bounds {
                    origin: Default::default(),
                    size: size(px(1200.), px(800.)),
                };
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: false,
                    show: false,
                    ..Default::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| {
                        let mut view = GraphView::new(window, cx);
                        view.open(repo, cx);
                        view
                    })
                })
            })
            .expect("window opens");

        // The loader runs on a real thread, which the test scheduler can't see.
        cx.allow_parking();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            let status = cx.update(|cx| view.read(cx).status().clone());
            match status {
                Status::Loaded => break,
                Status::Failed(message) => panic!("load failed: {message}"),
                _ if Instant::now() > deadline => panic!("load timed out"),
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        (handle, view)
    }

    fn capture(
        cx: &mut HeadlessAppContext,
        handle: AnyWindowHandle,
        name: &str,
    ) -> image::RgbaImage {
        cx.update_window(handle, |_, window, _| window.refresh())
            .unwrap();
        cx.run_until_parked();
        let frame = cx
            .capture_screenshot(handle)
            .expect("a renderer is required: this test does not fake one");
        if let Some(dir) = std::env::var_os("RENDER_OUTPUT") {
            let path = Path::new(&dir).join(format!("{name}.png"));
            frame.save(&path).expect("frame saves");
            println!("wrote {}", path.display());
        }
        frame
    }

    /// Distinct saturated colours in the graph column, which only the lanes can produce:
    /// the surfaces behind them are barely tinted.
    fn lane_hues(frame: &image::RgbaImage) -> usize {
        let scale = frame.width() as f32 / 1200.;
        let mut hues = std::collections::BTreeSet::new();
        for y in (LIST_TOP as f32 * scale) as u32..frame.height() {
            for x in (GRAPH_LEFT as f32 * scale) as u32..(GRAPH_RIGHT as f32 * scale) as u32 {
                let [r, g, b, _] = frame.get_pixel(x, y).0;
                let (max, min) = (r.max(g).max(b) as i32, r.min(g).min(b) as i32);
                if max - min > 45 {
                    hues.insert((r / 48, g / 48, b / 48));
                }
            }
        }
        hues.len()
    }

    fn assert_draws_lanes(mode: ThemeMode, name: &str) {
        let repo = fixture();
        let mut cx = app(mode);
        let (handle, view) = open(&mut cx, repo.path());
        assert_eq!(cx.update(|cx| view.read(cx).commit_count()), 11);

        let frame = capture(&mut cx, handle, name);
        let hues = lane_hues(&frame);
        assert!(hues >= 3, "expected several lane colours, found {hues}");
    }

    fn assert_working_changes_show_in_the_inspector() {
        let repo = fixture();
        let lines: String = (0..30).map(|line| format!("line {line}\n")).collect();
        repo.write("notes.txt", &lines);
        repo.commit("docs: add notes");
        repo.git(&["tag", "v0.1.1"]);
        let edited = lines
            .replace("line 3\n", "changed 3\n")
            .replace("line 26\n", "changed 26\n");
        repo.write("notes.txt", &edited);
        repo.write("draft.txt", "a new file\n");

        let mut cx = app(ThemeMode::Dark);
        let (handle, view) = open(&mut cx, repo.path());
        assert_eq!(cx.update(|cx| view.read(cx).commit_count()), 12);
        capture(&mut cx, handle, "working-row");

        // The working changes, with one file staged: two lists and the commit form.
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| {
                view.select_working_changes(cx);
                view.stage_paths(vec!["draft.txt".to_owned()], false, cx);
            });
        })
        .unwrap();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        capture(&mut cx, handle, "inspector");

        // A file's diff over the graph, in cards.
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| view.show_file(0, cx));
        })
        .unwrap();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        capture(&mut cx, handle, "diff");

        // The whole file, with the changes marked beside the scrollbar.
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| {
                let whole = gitclient_core::patch::DiffView {
                    whole_file: true,
                    ..Default::default()
                };
                view.set_diff_view(whole, cx);
            });
        })
        .unwrap();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        capture(&mut cx, handle, "diff-whole-file");
    }

    fn wait_for(cx: &mut HeadlessAppContext, done: impl Fn(&mut HeadlessAppContext) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(cx) {
            assert!(Instant::now() < deadline, "timed out");
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_conflict_screen_opens() {
        let repo = Repo::new();
        let path = repo.path();
        // Unchanged lines around the conflict, so the context shows and the whole file is long.
        let above: String = (1..=30).map(|n| format!("// helper {n}\n")).collect();
        let below: String = (1..=30).map(|n| format!("// rule {n}\n")).collect();
        let write = |text: &str| repo.write("parser.rs", &format!("{above}{text}{below}"));
        write("fn parse(input: &str) {\n    input.trim();\n}\n");
        commit(path, "Add the parser");
        git(path, &["checkout", "-q", "-b", "feat/parser"]);
        commit(path, "Name the parser");
        write("fn parse(input: &str) {\n    let line = input.trim();\n    line.len();\n}\n");
        commit(path, "Measure the input");
        commit(path, "Document the parser");
        git(path, &["checkout", "-q", "main"]);
        write("fn parse(input: &str) {\n    input.trim_end();\n}\n");
        commit(path, "Keep leading spaces");
        git(path, &["checkout", "-q", "feat/parser"]);
        assert!(gitclient_core::repo::conflict::rebase_onto(path, "main").is_ok());
        assert!(
            gitclient_core::repo::conflict::detect(path).is_some(),
            "fixture must conflict"
        );

        let mut cx = app(ThemeMode::Dark);
        let (handle, view) = open(&mut cx, path);
        wait_for(&mut cx, |cx| {
            cx.update(|cx| view.read(cx).paused_operation().is_some())
        });
        capture(&mut cx, handle, "conflict");

        // Both sides next to each other, over the graph.
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| view.show_conflict("parser.rs", cx));
        })
        .unwrap();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        capture(&mut cx, handle, "resolver");

        // The same setting as a diff's: the whole file instead of only the changes.
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| {
                let whole = gitclient_core::patch::DiffView {
                    whole_file: true,
                    ..Default::default()
                };
                view.set_diff_view(whole, cx);
            });
        })
        .unwrap();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        capture(&mut cx, handle, "resolver-whole");
    }

    /// The popups that ask for a name: the new-branch form with a name that git turns down,
    /// and the remote form. Both are cards in the middle of the window, in Mantle.
    fn assert_forms_draw() {
        use gitclient_core::menu::form::Form;
        let repo = fixture();
        let mut cx = app(ThemeMode::Dark);
        let (handle, view) = open(&mut cx, repo.path());
        let mantle = image::Rgba([0x18, 0x18, 0x25, 0xff]);
        let forms = [
            (
                "form-new-branch",
                Form::NewBranch {
                    start: Some("main".into()),
                    label: "main".into(),
                },
            ),
            ("form-remote", Form::Remote { original: None }),
            ("form-submodule", Form::Submodule { original: None }),
        ];
        for (name, form) in forms {
            cx.update_window(handle, |_, _, cx| {
                view.update(cx, |view, cx| view.open_form(form, cx));
            })
            .unwrap();
            let frame = capture(&mut cx, handle, name);
            // The 420 px card is centred; 205 px left of the middle is its padding, in
            // Mantle, beside the inputs. Opening the next form replaces this one.
            let scale = frame.width() as f32 / 1200.;
            let x = frame.width() / 2 - (205. * scale) as u32;
            let y = frame.height() / 2;
            assert_eq!(*frame.get_pixel(x, y), mantle, "{name}: the card is drawn");
        }
    }

    /// The sidebar, open, with the submodules pinned to its foot under a border, though
    /// the fixture has none.
    fn assert_sidebar_pins_submodules() {
        let repo = fixture();
        let mut cx = app(ThemeMode::Dark);
        let (handle, view) = open(&mut cx, repo.path());
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| {
                if !view.sidebar_is_open() {
                    view.toggle_sidebar(cx);
                }
            });
        })
        .unwrap();
        let frame = capture(&mut cx, handle, "sidebar");
        // Down the sidebar's middle: the pinned section's top border is a Surface 0 line
        // that starts under the panel's Mantle. It is more than one pixel on a scaled
        // display, so only its top edge is looked for.
        let scale = frame.width() as f32 / 1200.;
        let x = (150. * scale) as u32;
        let surface0 = image::Rgba([0x31, 0x32, 0x44, 0xff]);
        let mantle = image::Rgba([0x18, 0x18, 0x25, 0xff]);
        let lines: Vec<u32> = (1..frame.height())
            .filter(|&y| *frame.get_pixel(x, y) == surface0 && *frame.get_pixel(x, y - 1) == mantle)
            .collect();
        assert!(
            lines
                .iter()
                .any(|&y| y as f32 > frame.height() as f32 * 0.6),
            "a border low in the sidebar sets off the submodules: {lines:?}"
        );
    }

    /// The dock over a loaded repository: the log with the commands that loaded it, then a
    /// terminal that has run something.
    fn assert_dock_shows_the_log_and_a_terminal() {
        use gitclient::view::Workspace;
        use image::GenericImageView as _;
        let repo = fixture();
        let mut cx = app(ThemeMode::Dark);
        let path = repo.path().to_owned();
        let (handle, workspace) = cx
            .update(|cx| {
                let bounds = Bounds {
                    origin: Default::default(),
                    size: size(px(1200.), px(800.)),
                };
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: false,
                    show: false,
                    ..Default::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| {
                        let mut workspace = Workspace::new(window, cx);
                        workspace.open(path, window, cx);
                        workspace
                    })
                })
            })
            .expect("window opens");
        cx.allow_parking();
        wait_for(&mut cx, |cx| {
            cx.update(|cx| {
                let view = workspace.read(cx).active_view().clone();
                *view.read(cx).status() == Status::Loaded
            })
        });

        // Loading the repository logged pages of diff. A short log shows how it reads.
        gitclient_core::log::clear();
        gitclient_core::ops::run(repo.path(), &["status", "--short", "--branch"]).unwrap();
        gitclient_core::ops::run(repo.path(), &["log", "--oneline", "-3"]).unwrap();
        assert!(gitclient_core::ops::run(repo.path(), &["rev-parse", "no-such-ref"]).is_err());
        gitclient_core::ops::run(repo.path(), &["branch", "--list"]).unwrap();
        cx.update_window(handle, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace
                    .dock()
                    .update(cx, |dock, cx| dock.show(0, window, cx))
            })
        })
        .unwrap();
        let log = capture(&mut cx, handle, "dock-log");

        cx.update_window(handle, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace
                    .dock()
                    .update(cx, |dock, cx| dock.new_terminal(window, cx))
            })
        })
        .unwrap();
        let terminal = cx
            .update(|cx| {
                let dock = workspace.read(cx).dock().read(cx);
                dock.active_terminal().cloned()
            })
            .expect("a terminal opened");
        cx.update(|cx| {
            terminal
                .read(cx)
                .terminal()
                .expect("the shell started")
                .write(b"git log --oneline -3; echo ready\r")
        });
        wait_for(&mut cx, |cx| {
            cx.update(|cx| {
                let snapshot = terminal.read(cx).terminal().unwrap().snapshot();
                snapshot
                    .rows
                    .iter()
                    .flatten()
                    .any(|run| run.text.contains("ready"))
            })
        });
        let shell = capture(&mut cx, handle, "dock-terminal");

        // The dock is the bottom of the window: the terminal must replace the log there.
        let bottom = |frame: &image::RgbaImage| {
            let top = frame.height() * 3 / 4;
            frame
                .view(0, top, frame.width(), frame.height() - top)
                .to_image()
        };
        assert_ne!(
            bottom(&log),
            bottom(&shell),
            "the terminal replaces the log"
        );
    }

    /// Every page of the settings window, against a config file with something in it.
    fn assert_settings_pages_draw(mode: ThemeMode, suffix: &str) {
        use gitclient::view::Settings;
        use gitclient_core::settings::config::{self, Scope};
        use gitclient_core::settings::identity::{self, Identity, Match, Profile};

        let repo = fixture();
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope::File(dir.path().join("gitconfig"));
        for (key, value) in [
            ("user.name", "Pretext"),
            ("user.email", "pretext@example.com"),
            ("pull.rebase", "true"),
            ("fetch.prune", "true"),
            ("core.editor", "code --wait"),
        ] {
            config::set(&scope, key, value).unwrap();
        }
        config::set_alias(&scope, "co", "checkout").unwrap();
        config::set_alias(&scope, "st", "status -sb").unwrap();
        let profile = |name: &str, email: &str| Profile {
            name: name.into(),
            email: email.into(),
            signing_key: String::new(),
            ssh_key: String::new(),
        };
        config::set(&scope, "user.name", "Pretext").unwrap();
        let mut identity_named = |label: &str, profile: Profile| {
            let identity = Identity {
                id: String::new(),
                label: label.into(),
                profile,
            };
            identity::save_identity(&scope, &identity).unwrap()
        };
        let work = identity_named(
            "Work",
            Profile {
                ssh_key: "~/.ssh/id_work".into(),
                ..profile("Ada Lovelace", "ada@work.example")
            },
        );
        let home = identity_named("Home", profile("Ada", "ada@home.example"));
        identity::add_rule(&scope, &work, &Match::Host("github-work".into())).unwrap();
        identity::add_rule(&scope, &work, &Match::Folder("~/work/".into())).unwrap();
        identity::add_rule(&scope, &home, &Match::Host("github-home".into())).unwrap();
        let path = repo.path().to_owned();
        git(
            &path,
            &["remote", "add", "origin", "git@github-work:team/app.git"],
        );

        let mut cx = app(mode);
        let (handle, view) = cx
            .update(|cx| {
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1000.), px(900.)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| Settings::new(Some(path), scope, window, cx))
                })
            })
            .expect("window opens");
        cx.allow_parking();
        wait_for(&mut cx, |cx| cx.update(|cx| view.read(cx).is_loaded()));
        for _ in 0..30 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        for (index, name) in [(0, "git-config"), (1, "identities"), (3, "repository")] {
            cx.update_window(handle, |_, _, cx| {
                view.update(cx, |view, cx| view.show_page(index, cx));
            })
            .unwrap();
            // The repository page reads git when it opens.
            for _ in 0..30 {
                cx.run_until_parked();
                std::thread::sleep(Duration::from_millis(10));
            }
            capture(&mut cx, handle, &format!("settings-{name}-{suffix}"));
        }
    }

    pub fn run() {
        assert_settings_pages_draw(ThemeMode::Dark, "dark");
        assert_settings_pages_draw(ThemeMode::Light, "light");
        assert_conflict_screen_opens();
        assert_working_changes_show_in_the_inspector();
        assert_draws_lanes(ThemeMode::Dark, "graph-dark");
        assert_draws_lanes(ThemeMode::Light, "graph-light");
        assert_dock_shows_the_log_and_a_terminal();
        assert_forms_draw();
        assert_sidebar_pins_submodules();
        println!("render: passed (real offscreen renderer)");
    }
}
