use super::{AppState, Dialog, GraphView, Launcher, Notice, Status, exe_path, offer};
use crate::view::bind_keys;
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext, point, px};
use std::path::PathBuf;

struct Setup {
    view: Entity<GraphView>,
    cx: VisualTestContext,
    /// Where "shown" is remembered, and the data folder the launcher is written under.
    state: PathBuf,
    launcher: Launcher,
    dir: tempfile::TempDir,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
        bind_keys(cx);
    });
    let dir = tempfile::tempdir().unwrap();
    let window = cx.add_window(GraphView::new);
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    Setup {
        view,
        cx,
        state: dir.path().join("state"),
        launcher: Launcher::in_data_home(&dir.path().join("data")),
        dir,
    }
}

impl Setup {
    /// The launch step, with this setup's launcher.
    fn welcome(&mut self) {
        let launcher = self.launcher.clone();
        self.welcome_with(Some(launcher));
    }

    fn welcome_with(&mut self, launcher: Option<Launcher>) {
        let (view, state) = (self.view.clone(), self.state.clone());
        self.cx
            .update(|_, cx| offer(&view, launcher.as_ref(), &state, cx));
    }

    fn showing(&self) -> bool {
        self.view.read_with(&self.cx, |view, _| {
            matches!(view.dialog, Some(Dialog::Welcome(_)))
        })
    }

    /// Whether the panel offers the launcher, and if so whether its box is ticked.
    fn ticked(&self) -> Option<bool> {
        self.view.read_with(&self.cx, |view, _| match &view.dialog {
            Some(Dialog::Welcome(welcome)) => welcome.launcher.as_ref().map(|choice| choice.add),
            _ => None,
        })
    }

    fn seen(&self) -> bool {
        AppState::load(&self.state).welcome_seen
    }

    fn status(&self) -> Status {
        self.launcher.status(&exe_path())
    }

    fn notice(&self) -> Option<Notice> {
        self.view.read_with(&self.cx, |view, _| view.notice.clone())
    }

    fn enter(&mut self) {
        self.view
            .update(&mut self.cx, |view, cx| view.press_enter(cx));
        self.cx.run_until_parked();
    }

    fn escape(&mut self) {
        self.view
            .update(&mut self.cx, |view, cx| view.press_escape(cx));
        self.cx.run_until_parked();
    }

    fn space(&mut self) -> bool {
        self.view
            .update(&mut self.cx, |view, cx| view.toggle_welcome_launcher(cx))
    }

    /// Draws the window, so that a panel that panics while rendering fails its test.
    fn draw(&mut self) {
        self.view.update(&mut self.cx, |_, cx| cx.notify());
        // A pointer event draws the window before it is handled.
        self.cx
            .simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::none());
    }
}

#[gpui_kit::test]
fn the_first_run_shows_the_welcome_with_the_launcher_ticked_and_writes_nothing(
    cx: &mut TestAppContext,
) {
    let mut app = setup(cx);

    app.welcome();

    assert!(app.showing());
    assert!(app.seen(), "remembered as soon as it is shown");
    assert_eq!(app.ticked(), Some(true));
    assert_eq!(
        app.status(),
        Status::Absent,
        "nothing is written until Enter"
    );
    app.draw();
}

#[gpui_kit::test]
fn enter_adds_the_launcher_and_the_band_says_how_to_take_it_away(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.welcome();

    app.enter();

    assert!(matches!(app.status(), Status::Present { up_to_date: true }));
    assert!(!app.showing());
    let notice = app.notice();
    assert!(
        matches!(&notice, Some(Notice::Info(text)) if text.contains("Settings → App")),
        "{notice:?}"
    );
}

#[gpui_kit::test]
fn space_leaves_the_launcher_out_and_enter_then_only_starts(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.welcome();

    assert!(app.space());
    assert_eq!(app.ticked(), Some(false));
    app.draw();
    app.enter();

    assert!(!app.showing());
    assert_eq!(app.status(), Status::Absent, "the box was cleared");
    assert_eq!(app.notice(), None);
}

#[gpui_kit::test]
fn space_ticks_the_box_again_and_does_nothing_without_one(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.welcome();
    assert!(app.space() && app.space());
    assert_eq!(app.ticked(), Some(true));

    app.escape();
    assert!(
        !app.space(),
        "no panel, so Space is left for whatever has focus"
    );
}

#[gpui_kit::test]
fn escape_skips_without_writing_and_the_welcome_never_comes_back(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.welcome();

    app.escape();

    assert!(!app.showing());
    assert_eq!(app.status(), Status::Absent);
    assert_eq!(app.notice(), None);

    // The next start doesn't show it.
    app.welcome();
    assert!(!app.showing());
}

#[gpui_kit::test]
fn a_held_down_enter_never_starts_it(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.welcome();

    app.view.update(&mut app.cx, |view, cx| {
        view.key_held = true;
        view.press_enter(cx);
        view.key_held = false;
    });
    app.cx.run_until_parked();

    assert!(app.showing());
    assert_eq!(app.status(), Status::Absent);
}

#[gpui_kit::test]
fn nothing_is_shown_without_a_place_to_remember_it(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    // A state file inside a folder that can't be made: it would come back every start.
    app.state = PathBuf::from("/nonexistent-gitclient/never/state");

    app.welcome();

    assert!(!app.showing());
}

#[gpui_kit::test]
fn a_copy_a_package_manager_installed_gets_the_welcome_without_the_launcher(
    cx: &mut TestAppContext,
) {
    let mut app = setup(cx);

    app.welcome_with(None);

    assert!(app.showing());
    assert_eq!(app.ticked(), None, "no box to tick");
    assert!(!app.space());
    app.draw();
    app.enter();
    assert!(!app.showing());
    assert_eq!(app.status(), Status::Absent);
}

#[gpui_kit::test]
fn a_launcher_that_is_already_there_needs_no_offer_but_still_gets_the_welcome(
    cx: &mut TestAppContext,
) {
    let mut app = setup(cx);
    app.launcher.add(&exe_path()).unwrap();

    app.welcome();

    assert!(app.showing());
    assert_eq!(app.ticked(), None);
    assert!(app.seen());
}

#[gpui_kit::test]
fn a_launcher_somebody_else_wrote_is_neither_offered_nor_touched(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    // A launcher without our marker where ours would go.
    let folder = app.dir.path().join("data").join("applications");
    std::fs::create_dir_all(&folder).unwrap();
    let theirs = folder.join("gitclient.desktop");
    std::fs::write(&theirs, "[Desktop Entry]\nName=Mine\n").unwrap();

    app.welcome();
    app.enter();

    assert_eq!(app.status(), Status::Foreign);
    assert!(std::fs::read_to_string(theirs).unwrap().contains("Mine"));
}

#[gpui_kit::test]
fn a_launcher_for_a_copy_that_has_moved_is_mended_at_launch_without_the_welcome(
    cx: &mut TestAppContext,
) {
    let mut app = setup(cx);
    AppState::change(&app.state, |state| state.welcome_seen = true).unwrap();
    app.launcher
        .add(std::path::Path::new("/old/place/gitclient"))
        .unwrap();
    assert_eq!(app.status(), Status::Present { up_to_date: false });

    app.welcome();

    assert_eq!(app.status(), Status::Present { up_to_date: true });
    assert!(!app.showing());
}

#[gpui_kit::test]
fn clicking_outside_skips_like_escape(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.welcome();
    app.draw();

    // The backdrop fills the window, and the panel is in its middle.
    app.cx
        .simulate_click(point(px(2.), px(2.)), Modifiers::none());

    assert!(!app.showing());
    assert_eq!(app.status(), Status::Absent);
}
