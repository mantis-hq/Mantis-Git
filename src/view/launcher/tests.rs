use super::{AppState, GraphView, Launcher, Notice, Status, exe_path, offer};
use crate::view::bind_keys;
use crate::view::dialog::Dialog;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::path::PathBuf;

struct Setup {
    view: Entity<GraphView>,
    cx: VisualTestContext,
    /// Where the state is remembered, and the data folder the launcher is written under.
    state: PathBuf,
    launcher: Launcher,
    _dir: tempfile::TempDir,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
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
        _dir: dir,
    }
}

impl Setup {
    fn offer(&mut self) {
        let (view, launcher, state) =
            (self.view.clone(), self.launcher.clone(), self.state.clone());
        self.cx.update(|_, cx| offer(&view, &launcher, &state, cx));
    }

    fn asking(&self) -> bool {
        self.view.read_with(&self.cx, |view, _| {
            matches!(view.dialog, Some(Dialog::Confirm(_)))
        })
    }

    fn asked(&self) -> bool {
        AppState::load(&self.state).menu_entry_asked
    }

    fn status(&self) -> Status {
        self.launcher.status(&exe_path())
    }

    fn notice(&self) -> Option<Notice> {
        self.view.read_with(&self.cx, |view, _| view.notice.clone())
    }
}

#[gpui_kit::test]
fn the_first_run_asks_in_the_front_tab_and_changes_nothing_until_answered(cx: &mut TestAppContext) {
    let mut app = setup(cx);

    app.offer();

    assert!(app.asking());
    assert!(app.asked(), "remembered as soon as it is asked");
    assert_eq!(app.status(), Status::Absent);
    // The popup says what will be written, and how to take it away.
    let message = app.view.read_with(&app.cx, |view, _| match &view.dialog {
        Some(Dialog::Confirm(confirm)) => confirm.message.to_string(),
        _ => String::new(),
    });
    assert!(message.starts_with("Add gitclient to your"), "{message}");
    assert!(message.contains("Settings → App"), "{message}");
}

#[gpui_kit::test]
fn enter_adds_the_launcher_and_the_band_says_how_to_take_it_away(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.offer();

    app.view
        .update(&mut app.cx, |view, cx| view.press_enter(cx));
    app.cx.run_until_parked();

    assert!(matches!(app.status(), Status::Present { up_to_date: true }));
    assert!(!app.asking());
    let notice = app.notice();
    assert!(
        matches!(&notice, Some(Notice::Info(text)) if text.contains("Settings → App")),
        "{notice:?}"
    );
}

#[gpui_kit::test]
fn escape_declines_and_the_question_never_comes_back(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.offer();

    app.view
        .update(&mut app.cx, |view, cx| view.press_escape(cx));
    app.cx.run_until_parked();

    assert!(!app.asking());
    assert_eq!(app.status(), Status::Absent, "nothing was written");
    assert_eq!(app.notice(), None);

    // The next start doesn't ask.
    app.offer();
    assert!(!app.asking());
}

#[gpui_kit::test]
fn nothing_is_asked_without_a_place_to_remember_the_answer(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    // A state file inside a folder that can't be made: asking would come back every start.
    app.state = PathBuf::from("/nonexistent-gitclient/never/state");

    app.offer();

    assert!(!app.asking());
}

#[gpui_kit::test]
fn a_launcher_that_is_already_there_is_not_asked_about(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    app.launcher.add(&exe_path()).unwrap();

    app.offer();

    assert!(!app.asking());
    assert!(app.asked(), "and it is settled for good");
}

#[gpui_kit::test]
fn a_launcher_somebody_else_wrote_is_neither_asked_about_nor_touched(cx: &mut TestAppContext) {
    let mut app = setup(cx);
    // A launcher without our marker where ours would go.
    let folder = app._dir.path().join("data").join("applications");
    std::fs::create_dir_all(&folder).unwrap();
    let theirs = folder.join("gitclient.desktop");
    std::fs::write(&theirs, "[Desktop Entry]\nName=Mine\n").unwrap();

    app.offer();

    assert!(!app.asking());
    assert_eq!(app.status(), Status::Foreign);
    assert!(std::fs::read_to_string(theirs).unwrap().contains("Mine"));
}

#[gpui_kit::test]
fn a_launcher_for_a_copy_that_has_moved_is_mended_at_launch_without_asking(
    cx: &mut TestAppContext,
) {
    let mut app = setup(cx);
    app.launcher
        .add(std::path::Path::new("/old/place/gitclient"))
        .unwrap();
    assert_eq!(app.status(), Status::Present { up_to_date: false });

    app.offer();

    assert_eq!(app.status(), Status::Present { up_to_date: true });
    assert!(!app.asking());
}
