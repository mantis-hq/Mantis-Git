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
    use gpui_kit::assets::Assets;
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Entity, HeadlessAppContext, WindowBounds,
        WindowOptions, px, size,
    };
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Where the graph is drawn, inside the row area: below the toolbar and header.
    const GRAPH_LEFT: u32 = 0;
    const GRAPH_RIGHT: u32 = 60;
    const LIST_TOP: u32 = 72;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=Ada", "-c", "user.email=ada@example.com"])
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", repo.join(".no-global-config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", "2026-03-04T05:06:07Z")
            .env("GIT_COMMITTER_DATE", "2026-03-04T05:06:07Z")
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

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

    fn fixture(repo: &Path) {
        git(repo, &["init", "-q", "-b", "main"]);
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
        let repo = tempfile::tempdir().unwrap();
        fixture(repo.path());

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

        let (handle, view) = open(&mut cx, repo.path());
        assert_eq!(cx.update(|cx| view.read(cx).commit_count()), 11);

        let frame = capture(&mut cx, handle, name);
        let hues = lane_hues(&frame);
        assert!(hues >= 3, "expected several lane colours, found {hues}");
    }

    fn assert_working_changes_lead_to_the_commit_screen() {
        let repo = tempfile::tempdir().unwrap();
        fixture(repo.path());
        let lines: String = (0..30).map(|line| format!("line {line}\n")).collect();
        std::fs::write(repo.path().join("notes.txt"), &lines).unwrap();
        git(repo.path(), &["add", "notes.txt"]);
        git(repo.path(), &["commit", "-q", "-m", "docs: add notes"]);
        git(repo.path(), &["tag", "v0.1.1"]);
        let edited = lines
            .replace("line 3\n", "changed 3\n")
            .replace("line 26\n", "changed 26\n");
        std::fs::write(repo.path().join("notes.txt"), edited).unwrap();
        std::fs::write(repo.path().join("draft.txt"), "a new file\n").unwrap();

        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            view::bind_keys(cx);
            Theme::change(ThemeMode::Dark, None, cx);
        });
        let (handle, view) = open(&mut cx, repo.path());
        assert_eq!(cx.update(|cx| view.read(cx).commit_count()), 12);
        capture(&mut cx, handle, "working-row");

        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |view, cx| view.open_commit_screen(cx));
        })
        .unwrap();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        capture(&mut cx, handle, "commit");
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
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path();
        git(path, &["init", "-q", "-b", "main"]);
        git(path, &["config", "user.name", "Ada"]);
        git(path, &["config", "user.email", "ada@example.com"]);
        git(path, &["config", "commit.gpgsign", "false"]);
        let write = |text: &str| std::fs::write(path.join("parser.rs"), text).unwrap();
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
        assert!(gitclient::conflict::rebase_onto(path, "main").is_ok());
        assert!(
            gitclient::conflict::detect(path).is_some(),
            "fixture must conflict"
        );

        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            view::bind_keys(cx);
            Theme::change(ThemeMode::Dark, None, cx);
        });
        let (handle, view) = open(&mut cx, path);
        wait_for(&mut cx, |cx| {
            cx.update(|cx| view.read(cx).screen()) == view::Screen::Conflict
        });
        capture(&mut cx, handle, "conflict");
    }

    pub fn run() {
        assert_conflict_screen_opens();
        assert_working_changes_lead_to_the_commit_screen();
        assert_draws_lanes(ThemeMode::Dark, "graph-dark");
        assert_draws_lanes(ThemeMode::Light, "graph-light");
        println!("render: passed (real offscreen renderer)");
    }
}
