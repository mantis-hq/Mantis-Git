//! Tests that drive the whole graph view: loading, selecting, menus, staging and the
//! popups, as someone using the window would.

// Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
use super::{Entry, GraphView, LoadEvent, Status, bind_keys};
use gitclient_core::history::Commit;
use gitclient_core::history::lanes::GraphBuilder;
use gitclient_core::history::refs::RefKind;
use gitclient_core::repo::branch::Dragged;
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext, point, px};
use std::sync::Arc;

fn entries(names: &[&str]) -> Vec<Entry> {
    let mut builder = GraphBuilder::new();
    names
        .iter()
        .enumerate()
        .map(|(index, &name)| {
            let parents: Box<[Arc<str>]> = names
                .get(index + 1)
                .map(|&parent| parent.into())
                .into_iter()
                .collect();
            Entry {
                working: false,
                row: builder.push(name, &parents),
                commit: Commit {
                    oid: name.into(),
                    parents,
                    author: "Author".into(),
                    time: 0,
                    subject: name.into(),
                },
            }
        })
        .collect()
}

fn open_view(cx: &mut TestAppContext) -> (Entity<GraphView>, VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        bind_keys(cx);
    });
    let window = cx.add_window(GraphView::new);
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    (view, cx)
}

#[gpui_kit::test]
fn the_way_files_are_listed_is_saved_for_every_repository(cx: &mut TestAppContext) {
    use gitclient_core::settings::state::AppState;
    use gitclient_core::tree::FileView;
    let (view, mut cx) = open_view(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state");
    AppState {
        reopen_tabs: false,
        ..Default::default()
    }
    .save(&file)
    .unwrap();
    view.update(&mut cx, |view, cx| {
        view.set_state_file(Some(file.clone()));
        view.set_files_view(FileView::Tree, cx);
    });
    let saved = AppState::load(&file);
    assert_eq!(saved.files_view, FileView::Tree);
    assert!(!saved.reopen_tabs, "other settings are kept");
}

#[gpui_kit::test]
fn the_order_of_the_graph_columns_is_saved_and_comes_back(cx: &mut TestAppContext) {
    use gitclient_core::history::columns::Column;
    use gitclient_core::settings::state::AppState;
    let (view, mut cx) = open_view(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state");
    AppState {
        reopen_tabs: false,
        ..Default::default()
    }
    .save(&file)
    .unwrap();
    view.update(&mut cx, |view, cx| {
        view.set_state_file(Some(file.clone()));
        view.move_column(Column::Date, Column::Refs, cx);
    });
    let saved = AppState::load(&file);
    assert_eq!(saved.graph_columns.columns()[0], Column::Date);
    assert!(!saved.reopen_tabs, "other settings are kept");

    let (again, mut cx2) = open_view(&mut cx.cx.clone());
    again.update(&mut cx2, |view, _| {
        view.set_state_file(Some(file.clone()));
        assert_eq!(view.columns, saved.graph_columns);
    });
}

#[gpui_kit::test]
fn wide_histories_stop_growing_the_graph_at_eight_lanes(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a"], &mut cx);
    view.update(&mut cx, |view, _| {
        view.max_columns = 259;
        let eight = view.graph_width();
        view.max_columns = 9;
        assert_eq!(view.graph_width(), eight);
        view.max_columns = 7;
        assert!(view.graph_width() < eight);
    });
}

fn load(view: &Entity<GraphView>, names: &[&str], cx: &mut VisualTestContext) {
    load_with(view, names, Arc::default(), cx);
}

fn load_with(
    view: &Entity<GraphView>,
    names: &[&str],
    refs: Arc<gitclient_core::history::refs::Refs>,
    cx: &mut VisualTestContext,
) {
    view.update(cx, |view, cx| {
        view.status = Status::Loading;
        view.apply(LoadEvent::Refs(refs), cx);
        view.apply(LoadEvent::Batch(entries(names)), cx);
        view.finish_load(cx);
    });
}

#[gpui_kit::test]
fn searching_for_a_file_lists_its_commits_and_picking_one_selects_it(cx: &mut TestAppContext) {
    use gitclient_core::history::search::Mode;
    use gitclient_core::testing::Repo;
    let repo = Repo::new();
    repo.write("a.txt", "1");
    let first = repo.commit("first");
    repo.write("b.txt", "1");
    let second = repo.commit("second");
    repo.write("a.txt", "2");
    let third = repo.commit("third");
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, _| {
        view.repo = Some(repo.path().to_path_buf())
    });
    load(&view, &[&third, &second, &first], &mut cx);

    cx.simulate_keystrokes("ctrl-f");
    assert!(view.read_with(&cx, |view, _| view.search_is_open()));

    view.update_in(&mut cx, |view, window, cx| {
        view.search_for("a.txt", Mode::File, window, cx)
    });
    cx.executor().allow_parking();
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(&cx, |view, _| view.search_subjects().is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let found = view.read_with(&cx, |view, _| view.search_subjects());
    assert_eq!(found, Some(vec!["third".to_owned(), "first".to_owned()]));

    view.update_in(&mut cx, |view, window, cx| {
        view.pick_search_result(1, window, cx)
    });
    assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(2));

    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(&cx, |view, _| view.search_is_open()));
    // The keys are back with the graph.
    cx.simulate_keystrokes("up");
    assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(1));
}

#[gpui_kit::test]
fn keyboard_moves_the_selection_and_stops_at_the_ends(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b", "c"], &mut cx);
    let selected = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.selected());

    assert_eq!(selected(&mut cx), None);
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&mut cx), Some(0));
    cx.simulate_keystrokes("down j");
    assert_eq!(selected(&mut cx), Some(2));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&mut cx), Some(2));
    cx.simulate_keystrokes("home");
    assert_eq!(selected(&mut cx), Some(0));
    cx.simulate_keystrokes("up");
    assert_eq!(selected(&mut cx), Some(0));
    cx.simulate_keystrokes("end");
    assert_eq!(selected(&mut cx), Some(2));
    cx.simulate_keystrokes("pageup");
    assert_eq!(selected(&mut cx), Some(0));
}

#[gpui_kit::test]
fn selection_follows_its_commit_across_a_reload(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["b", "a"], &mut cx);
    cx.simulate_keystrokes("down");
    assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(0));

    load(&view, &["new", "b", "a"], &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(1));

    load(&view, &["other"], &mut cx);
    assert_eq!(view.read_with(&cx, |view, _| view.selected()), None);
}

#[gpui_kit::test]
fn a_refresh_keeps_the_old_history_until_new_commits_arrive(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.status = Status::Loading;
        view.apply(LoadEvent::Refs(Arc::default()), cx);
        assert_eq!(view.commit_count(), 1);
        view.apply(LoadEvent::Batch(entries(&["x", "y"])), cx);
        assert_eq!(view.commit_count(), 2);
    });
}

#[gpui_kit::test]
fn a_repository_emptied_by_a_reload_shows_no_stale_commits(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.status = Status::Loading;
        view.apply(LoadEvent::Refs(Arc::default()), cx);
        view.finish_load(cx);
        assert_eq!(view.commit_count(), 0);
        assert_eq!(view.status(), &Status::Loaded);
    });
}

#[gpui_kit::test]
fn a_failed_load_stays_failed(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| {
        view.status = Status::Loading;
        view.apply(LoadEvent::Failed("nope".into()), cx);
        view.finish_load(cx);
        assert_eq!(view.status(), &Status::Failed("nope".into()));
    });
}

#[gpui_kit::test]
fn a_failed_operation_says_why_and_a_quiet_one_says_nothing(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| {
        view.finish_operation(Err("push was rejected".into()), cx);
        assert_eq!(view.notice_text().as_deref(), Some("push was rejected"));
        view.finish_operation(Ok(None), cx);
        assert_eq!(view.notice_text(), None);
    });
}

#[gpui_kit::test]
fn a_force_push_waits_for_confirmation_and_others_do_not(cx: &mut TestAppContext) {
    use gitclient_core::repo::status::NextAction;
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| {
        let force = NextAction::ForcePush {
            commits: 1,
            replaced: 2,
        };
        view.start_action(force, cx);
        assert!(view.is_confirming());
        let Some(super::dialog::Dialog::Confirm(confirm)) = &view.dialog else {
            panic!("a force push asks in a popup");
        };
        let band = confirm.message.clone();
        assert!(band.contains("replaces 2 commits"), "{band}");
        assert!(band.contains("git reflog"), "says how to undo it: {band}");

        view.dialog = None;
        view.start_action(NextAction::Push { commits: 1 }, cx);
        assert!(!view.is_confirming());
    });
}

#[gpui_kit::test]
fn a_deletion_popup_takes_enter_and_escape_and_the_risky_one_needs_two_presses(
    cx: &mut TestAppContext,
) {
    use super::dialog::Confirm;
    use gitclient_core::menu::command::Weight;
    use std::cell::Cell;
    use std::rc::Rc;
    let (view, mut cx) = open_view(cx);
    let ran = Rc::new(Cell::new(0));
    let ask = |view: &mut GraphView, weight, cx: &mut gpui_kit::Context<GraphView>| {
        let ran = ran.clone();
        let confirm = Confirm::new("This deletes it.", "Delete", weight, move |_, _| {
            ran.set(ran.get() + 1)
        });
        view.ask(confirm, cx);
    };
    view.update(&mut cx, |view, cx| {
        ask(view, Weight::Popup, cx);
        view.press_escape(cx);
        assert!(!view.is_confirming());
        assert_eq!(ran.get(), 0, "Escape cancels");

        ask(view, Weight::Popup, cx);
        view.key_held = true;
        view.press_enter(cx);
        assert!(view.is_confirming(), "a held key doesn't answer");
        view.key_held = false;
        view.press_enter(cx);
        assert!(!view.is_confirming());
        assert_eq!(ran.get(), 1, "Enter confirms");

        ask(view, Weight::DoublePress, cx);
        view.press_enter(cx);
        assert!(view.is_confirming());
        assert_eq!(ran.get(), 1, "one press only arms it");
        view.press_enter(cx);
        assert!(!view.is_confirming());
        assert_eq!(ran.get(), 2, "the second press confirms");

        ask(view, Weight::DoublePress, cx);
        view.press_enter(cx);
        view.press_escape(cx);
        assert!(!view.is_confirming());
        assert_eq!(ran.get(), 2);
    });
}

#[gpui_kit::test]
fn the_keys_reach_a_popup_even_when_focus_was_elsewhere(cx: &mut TestAppContext) {
    use super::dialog::Confirm;
    use gitclient_core::menu::command::Weight;
    use std::cell::Cell;
    use std::rc::Rc;
    let (view, mut cx) = open_view(cx);
    let ran = Rc::new(Cell::new(0));
    let ask = |cx: &mut VisualTestContext| {
        let ran = ran.clone();
        view.update(cx, |view, cx| {
            let confirm = Confirm::new("This deletes it.", "Delete", Weight::Popup, move |_, _| {
                ran.set(ran.get() + 1)
            });
            view.ask(confirm, cx);
        });
        cx.run_until_parked();
    };
    // Nothing has focused the view, as after a click on a menu line.
    cx.update(|window, cx| window.blur(cx));
    ask(&mut cx);
    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(&cx, |view, _| view.is_confirming()));
    assert_eq!(ran.get(), 0);
    ask(&mut cx);
    cx.simulate_keystrokes("enter");
    assert!(!view.read_with(&cx, |view, _| view.is_confirming()));
    assert_eq!(ran.get(), 1);
}

#[gpui_kit::test]
fn the_sidebar_stays_out_of_narrow_windows_until_asked_for(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| {
        view.sidebar.window_is_wide = false;
        assert!(!view.sidebar_is_open());
        view.toggle_sidebar(cx);
        assert!(view.sidebar_is_open());

        view.sidebar.window_is_wide = true;
        view.toggle_sidebar(cx);
        assert!(!view.sidebar_is_open(), "a choice holds at any width");
    });
}

/// Refs for branches named like the commits they point at.
fn branches(names: &[&str]) -> Arc<gitclient_core::history::refs::Refs> {
    let mut refs = gitclient_core::history::refs::Refs::default();
    refs.branches = names
        .iter()
        .map(|name| gitclient_core::history::refs::Branch {
            name: (*name).into(),
            tip: (*name).into(),
            color: Default::default(),
            is_head: false,
            ahead_behind: None,
            upstream: None,
            upstream_name: None,
        })
        .collect();
    Arc::new(refs)
}

fn branch(name: &str) -> Dragged {
    Dragged {
        kind: RefKind::Branch,
        name: name.into(),
    }
}

#[gpui_kit::test]
fn picking_twice_turns_it_off_and_a_reload_keeps_it(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load_with(&view, &["b", "a"], branches(&["a", "b"]), &mut cx);
    let rows = [branch("a"), branch("b")];
    view.update(&mut cx, |view, cx| {
        view.pick(&rows[0], Modifiers::none(), &rows, cx);
        let spot = view.sidebar.spotlight.clone().unwrap();
        assert!(!spot.commit_is_lit(0) && spot.commit_is_lit(1));

        view.pick(&rows[0], Modifiers::none(), &rows, cx);
        assert!(view.sidebar.spotlight.is_none());
        view.pick(&rows[0], Modifiers::none(), &rows, cx);
    });
    load_with(&view, &["c", "b", "a"], branches(&["a", "b"]), &mut cx);
    view.read_with(&cx, |view, _| {
        let spot = view.sidebar.spotlight.as_ref().unwrap();
        assert!(spot.commit_is_lit(2) && !spot.commit_is_lit(0));
    });
}

#[gpui_kit::test]
fn shift_and_ctrl_pick_several_branches(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load_with(&view, &["c", "b", "a"], branches(&["a", "b", "c"]), &mut cx);
    let rows = [branch("a"), branch("b"), branch("c")];
    let picked = |view: &Entity<GraphView>, cx: &VisualTestContext| {
        view.read_with(cx, |view, _| {
            view.sidebar
                .picks
                .items()
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
        })
    };
    view.update(&mut cx, |view, cx| {
        view.pick(&rows[0], Modifiers::none(), &rows, cx);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::none()
        };
        view.pick(&rows[2], shift, &rows, cx);
    });
    assert_eq!(picked(&view, &cx), ["a", "b", "c"]);

    view.update(&mut cx, |view, cx| {
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::none()
        };
        view.pick(&rows[1], ctrl, &rows, cx);
    });
    assert_eq!(picked(&view, &cx), ["a", "c"]);
}

#[gpui_kit::test]
fn right_clicking_a_picked_branch_offers_to_delete_all_picked(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load_with(&view, &["c", "b", "a"], branches(&["a", "b", "c"]), &mut cx);
    let rows = [branch("a"), branch("b"), branch("c")];
    view.update(&mut cx, |view, cx| {
        view.pick(&rows[0], Modifiers::none(), &rows, cx);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::none()
        };
        view.pick(&rows[1], shift, &rows, cx);
        view.open_ref_menu(rows[0].clone(), point(px(10.), px(10.)), cx);
        let lines = view.menu.as_ref().unwrap().lines();
        assert!(lines.iter().any(|line| matches!(
            line.command(),
            Some(gitclient_core::menu::command::Command::DeleteMany { refs }) if refs.len() == 2
        )));

        // A branch outside the picks keeps its own menu.
        view.open_ref_menu(rows[2].clone(), point(px(10.), px(10.)), cx);
        let lines = view.menu.as_ref().unwrap().lines();
        assert!(!lines.iter().any(|line| matches!(
            line.command(),
            Some(gitclient_core::menu::command::Command::DeleteMany { .. })
        )));
    });
}

#[gpui_kit::test]
fn clicking_a_row_selects_it(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b", "c"], &mut cx);
    cx.run_until_parked();

    // The second row, below the 52px header and the 28px column headings.
    cx.simulate_click(
        point(px(300.), px(52. + 28. + 32. + 16.)),
        Modifiers::none(),
    );
    assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(1));
}

#[gpui_kit::test]
fn a_read_of_tracked_files_alone_keeps_the_untracked_ones_it_had(cx: &mut TestAppContext) {
    use super::inspector::Scan;
    use gitclient_core::testing::Repo;
    let repo = Repo::new();
    repo.write("a", "1\n");
    repo.commit("base");
    repo.write("a", "2\n");
    repo.write("new-1", "x\n");
    let (view, mut cx) = open_view(cx);
    load(&view, &["x"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.repo = Some(repo.path().to_owned());
        view.entries[0].working = true;
        view.select(0, cx);
    });
    cx.run_until_parked();
    let listed = |cx: &VisualTestContext| {
        view.read_with(cx, |view, _| {
            let unstaged = &view.working_changes().unwrap().unstaged;
            let names: Vec<String> = unstaged.iter().map(|f| f.path.clone()).collect();
            (names, view.inspector.scanning_untracked)
        })
    };
    assert_eq!(listed(&cx), (vec!["a".into(), "new-1".into()], false));

    // A file appears, and a read of tracked files alone doesn't go looking for it, but
    // changes to the files git knows are seen, and the untracked one stays listed.
    repo.write("new-2", "y\n");
    repo.write("a", "3\n4\n");
    view.update(&mut cx, |view, cx| {
        view.reload_working_as(Scan::Tracked, cx)
    });
    cx.run_until_parked();
    assert_eq!(listed(&cx), (vec!["a".into(), "new-1".into()], false));
    let lines = view.read_with(&cx, |view, _| {
        view.working_changes().unwrap().unstaged[0].change.additions
    });
    assert_eq!(lines, 2, "the tracked file is read again");

    // A full read finds it.
    view.update(&mut cx, |view, cx| view.reload_working(cx));
    cx.run_until_parked();
    let (names, scanning) = listed(&cx);
    assert_eq!(names, ["a", "new-1", "new-2"]);
    assert!(!scanning, "the note goes once the scan is done");
}

#[gpui_kit::test]
fn ticking_a_file_moves_it_between_the_lists_and_its_menu_follows(cx: &mut TestAppContext) {
    use gitclient_core::menu::command::Command;
    use gitclient_core::testing::Repo;
    use gitclient_core::working::Side;
    let repo = Repo::new();
    repo.write("a", "1\n");
    repo.commit("base");
    repo.write("a", "2\n");
    let (view, mut cx) = open_view(cx);
    load(&view, &["x"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.repo = Some(repo.path().to_owned());
        view.entries[0].working = true;
        view.select(0, cx);
    });
    cx.run_until_parked();
    let counts = |cx: &VisualTestContext| {
        view.read_with(cx, |view, _| {
            let changes = view.working_changes().expect("changes were read");
            (changes.unstaged.len(), changes.staged.len())
        })
    };
    assert_eq!(counts(&cx), (1, 0));

    view.update(&mut cx, |view, cx| {
        view.stage_paths(vec!["a".to_owned()], false, cx)
    });
    cx.run_until_parked();
    assert_eq!(counts(&cx), (0, 1));
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a");

    // The file is the first of the lists end to end, now in the staged one.
    view.update(&mut cx, |view, cx| {
        assert_eq!(
            view.working_file(0).map(|(side, _)| side),
            Some(Side::Staged)
        );
        view.open_file_menu(0, point(px(10.), px(10.)), cx);
        let lines = view.menu.as_ref().unwrap().lines();
        let unstage = Command::StageFiles {
            paths: vec!["a".to_owned()],
            unstage: true,
        };
        assert!(lines.iter().any(|line| line.command() == Some(&unstage)));
        view.close_menu(cx);
        view.show_file(0, cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(&cx, |view, _| view.diff_is_open()));
    // Drawing the lists, the form and the diff with a file open must not trip.
    cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());

    // Ticking the file in the diff unstages it, and the diff follows it to the other list.
    view.update(&mut cx, |view, cx| {
        view.stage_paths(vec!["a".to_owned()], true, cx)
    });
    cx.run_until_parked();
    assert_eq!(counts(&cx), (1, 0));
    view.read_with(&cx, |view, _| {
        assert_eq!(view.shown_file(), Some(0), "the diff follows the file");
    });
}

#[gpui_kit::test]
fn ticking_a_hunk_in_the_diff_stages_only_that_hunk(cx: &mut TestAppContext) {
    use gitclient_core::testing::Repo;
    let long = |top: &str, bottom: &str| {
        let middle: String = (0..18).map(|n| format!("line {n}\n")).collect();
        format!("{top}\n{middle}{bottom}\n")
    };
    let repo = Repo::new();
    repo.write("f", &long("top", "bottom"));
    repo.commit("base");
    repo.write("f", &long("new top", "new bottom"));
    let (view, mut cx) = open_view(cx);
    load(&view, &["x"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.repo = Some(repo.path().to_owned());
        view.entries[0].working = true;
        view.select(0, cx);
    });
    cx.run_until_parked();
    view.update(&mut cx, |view, cx| view.show_file(0, cx));
    cx.run_until_parked();
    cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());

    view.update(&mut cx, |view, cx| view.move_hunk(1, cx));
    cx.run_until_parked();
    assert_eq!(repo.git(&["show", ":f"]), long("top", "new bottom").trim());
    view.read_with(&cx, |view, _| {
        let changes = view.working_changes().unwrap();
        assert_eq!(changes.staged.len(), 1);
        assert_eq!(changes.unstaged.len(), 1);
        // The file is in both lists now, and the diff stays on the unstaged one.
        assert_eq!(view.shown_file(), Some(0));
    });
}

#[gpui_kit::test]
fn a_conflict_is_listed_in_the_inspector_resolved_from_its_menu_and_continued(
    cx: &mut TestAppContext,
) {
    use gitclient_core::menu::command::Command;
    use gitclient_core::testing::Repo;
    let repo = Repo::new();
    repo.git(&["config", "user.name", "Ada"]);
    repo.git(&["config", "user.email", "ada@example.com"]);
    repo.write("a", "base\n");
    repo.commit("base");
    let main = repo.git(&["branch", "--show-current"]);
    repo.git(&["checkout", "-q", "-b", "feat"]);
    repo.write("a", "feat\n");
    repo.commit("feat change");
    repo.git(&["checkout", "-q", &main]);
    repo.write("a", "main\n");
    repo.commit("main change");
    // The merge stops on the conflict, which git reports as a failure.
    let merged = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["merge", "feat"])
        .output()
        .unwrap();
    assert!(!merged.status.success());

    let (view, mut cx) = open_view(cx);
    load(&view, &["x"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.repo = Some(repo.path().to_owned());
        view.entries[0].working = true;
        view.check_paused_operation(cx);
    });
    cx.run_until_parked();
    view.read_with(&cx, |view, _| {
        let progress = view.paused_operation().expect("the merge is paused");
        assert_eq!(progress.files().len(), 1);
        assert!(view.working_row_is_selected(), "the conflicts are in sight");
        assert!(!view.resolver_is_open());
    });

    // Both sides open next to each other, over the graph.
    view.update(&mut cx, |view, cx| view.show_conflict("a", cx));
    assert!(view.read_with(&cx, |view, _| view.resolver_is_open()));
    cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());

    // The menu offers each branch by name and never "mine" or "theirs".
    view.update(&mut cx, |view, cx| {
        view.open_conflict_menu("a", point(px(10.), px(10.)), cx);
        let lines = view.menu.as_ref().unwrap().lines();
        let taking_other = lines.iter().any(|line| {
            matches!(
                line.command(),
                Some(Command::UseSide {
                    side: gitclient_core::repo::conflict::Side::Other,
                    ..
                })
            )
        });
        assert!(taking_other);
        view.close_menu(cx);
        view.use_side_for_file("a", gitclient_core::repo::conflict::Side::Mine, cx);
    });
    cx.run_until_parked();
    view.update(&mut cx, |view, cx| view.check_paused_operation(cx));
    cx.run_until_parked();
    // Taking a side saved the file, so nothing is left to decide and the merge can go on.
    view.read_with(&cx, |view, _| {
        let progress = view
            .paused_operation()
            .expect("still paused until continued");
        assert!(progress.decisions.is_empty());
        assert!(!view.resolver_is_open(), "the last file was saved");
    });
    view.update(&mut cx, |view, cx| view.continue_operation(cx));
    cx.run_until_parked();
    view.update(&mut cx, |view, cx| view.check_paused_operation(cx));
    cx.run_until_parked();
    view.read_with(&cx, |view, _| {
        assert!(view.paused_operation().is_none(), "the merge finished");
    });
    assert_eq!(repo.read("a"), "main\n");
}

/// A merge paused on one conflict in `a`, open in the resolver with its result in the editor.
fn resolving_a_conflict(
    cx: &mut TestAppContext,
) -> (
    gitclient_core::testing::Repo,
    Entity<GraphView>,
    VisualTestContext,
) {
    resolving_a_conflict_between(cx, "top\n", "bottom\n")
}

/// The same, with `above` and `below` around the conflict.
fn resolving_a_conflict_between(
    cx: &mut TestAppContext,
    above: &str,
    below: &str,
) -> (
    gitclient_core::testing::Repo,
    Entity<GraphView>,
    VisualTestContext,
) {
    use gitclient_core::testing::Repo;
    let repo = Repo::new();
    repo.git(&["config", "user.name", "Ada"]);
    repo.git(&["config", "user.email", "ada@example.com"]);
    repo.write("a", &format!("{above}base\n{below}"));
    repo.commit("base");
    let main = repo.git(&["branch", "--show-current"]);
    repo.git(&["checkout", "-q", "-b", "feat"]);
    repo.write("a", &format!("{above}feat one\nfeat two\n{below}"));
    repo.commit("feat change");
    repo.git(&["checkout", "-q", &main]);
    repo.write("a", &format!("{above}main\n{below}"));
    repo.commit("main change");
    let merged = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["merge", "feat"])
        .output()
        .unwrap();
    assert!(!merged.status.success());

    let (view, mut cx) = open_view(cx);
    load(&view, &["x"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.repo = Some(repo.path().to_owned());
        view.entries[0].working = true;
        view.check_paused_operation(cx);
    });
    cx.run_until_parked();
    view.update(&mut cx, |view, cx| view.show_conflict("a", cx));
    cx.run_until_parked();
    // The first frame draws an empty editor, and the next puts the file's result in it.
    cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());
    cx.run_until_parked();
    assert!(
        view.read_with(&cx, |view, _| view.conflict_editor().is_some()),
        "the result is in the editor"
    );
    (repo, view, cx)
}

#[gpui_kit::test]
fn a_result_is_built_from_ticked_lines_and_saved_with_one_button(cx: &mut TestAppContext) {
    use gitclient_core::repo::conflict::Side;
    let (repo, view, mut cx) = resolving_a_conflict(cx);
    let editor = view.read_with(&cx, |view, _| view.conflict_editor().unwrap());
    let in_editor =
        |cx: &mut VisualTestContext| editor.read_with(cx, |editor, _| editor.text().to_string());
    let markers = "top\n<<<<<<< HEAD\nmain\n=======\nfeat one\nfeat two\n>>>>>>> feat\nbottom\n";
    assert_eq!(in_editor(&mut cx), markers);

    // Saving waits for the conflict to be settled in the result.
    view.update(&mut cx, |view, cx| view.save_conflict(cx));
    cx.run_until_parked();
    assert!(view.read_with(&cx, |view, _| view.resolver_is_open()));
    assert!(repo.read("a").contains("<<<<<<<"), "nothing was saved");

    // Each tick shows in the result at once: the first side's ticked lines, then the
    // second's, in place of the markers.
    view.update_in(&mut cx, |view, window, cx| {
        view.toggle_line((0, Side::Other, 1), window, cx);
    });
    assert_eq!(in_editor(&mut cx), "top\nfeat two\nbottom\n");
    view.update_in(&mut cx, |view, window, cx| {
        view.toggle_line((0, Side::Mine, 0), window, cx);
        assert_eq!(view.conflicts_left("a"), 0, "known at once");
    });
    assert_eq!(in_editor(&mut cx), "top\nmain\nfeat two\nbottom\n");

    // Clearing every tick puts the markers back; "Use" ticks a whole side.
    view.update_in(&mut cx, |view, window, cx| {
        view.toggle_line((0, Side::Other, 1), window, cx);
        view.toggle_line((0, Side::Mine, 0), window, cx);
        assert_eq!(view.conflicts_left("a"), 1);
    });
    assert_eq!(in_editor(&mut cx), markers);
    view.update_in(&mut cx, |view, window, cx| {
        view.use_chunk(0, Side::Other, window, cx)
    });
    assert_eq!(in_editor(&mut cx), "top\nfeat one\nfeat two\nbottom\n");
    view.update_in(&mut cx, |view, window, cx| {
        view.toggle_line((0, Side::Other, 0), window, cx);
        view.toggle_line((0, Side::Mine, 0), window, cx);
    });
    assert_eq!(in_editor(&mut cx), "top\nmain\nfeat two\nbottom\n");
    cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());
    view.update(&mut cx, |view, cx| view.save_conflict(cx));
    cx.run_until_parked();
    view.update(&mut cx, |view, cx| view.check_paused_operation(cx));
    cx.run_until_parked();

    assert_eq!(repo.read("a"), "top\nmain\nfeat two\nbottom\n");
    assert_eq!(repo.git(&["diff", "--name-only", "--diff-filter=U"]), "");
    view.read_with(&cx, |view, _| {
        assert!(!view.resolver_is_open(), "the last file was saved");
        assert!(view.paused_operation().unwrap().decisions.is_empty());
    });
}

#[gpui_kit::test]
fn the_two_versions_and_the_result_scroll_together(cx: &mut TestAppContext) {
    let lines = |name: &str| -> String { (0..300).map(|n| format!("{name} {n}\n")).collect() };
    let (_repo, view, mut cx) = resolving_a_conflict_between(cx, &lines("above"), &lines("below"));
    let draw = |cx: &mut VisualTestContext| {
        for _ in 0..3 {
            cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());
            cx.run_until_parked();
        }
    };
    let at = |cx: &mut VisualTestContext| view.update(cx, |view, cx| view.resolver_scroll(cx));
    let near = |(list, result): (f32, f32), (want_list, want_result): (f32, f32)| {
        assert!(
            (list - want_list).abs() < 0.5 && (result - want_result).abs() < 0.5,
            "list at row {list}, result at line {result}; wanted {want_list} and {want_result}"
        );
    };
    // The list is the top half of the resolver, and the result the bottom half; the
    // inspector takes the right.
    let size = cx.update(|window, _| window.viewport_size());
    let x = (size.width - px(380.)) / 2.;
    let (over_list, over_result) = (point(x, px(260.)), point(x, size.height - px(160.)));
    let wheel = |cx: &mut VisualTestContext, position, down: f32| {
        cx.simulate_event(gpui_kit::ScrollEvent {
            position,
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-down))),
            ..Default::default()
        });
        draw(cx);
    };
    // A file opens at its first conflict, and the result is there too.
    draw(&mut cx);
    near(at(&mut cx), (300., 300.));

    // Above the conflict, a row of the list is the same line of the result.
    wheel(&mut cx, over_list, -200. * 24.);
    near(at(&mut cx), (100., 100.));

    // The list has a header and two rows for the conflict; the result has its six lines
    // of markers. Below it, each keeps to the same line of the file.
    wheel(&mut cx, over_list, 250. * 24.);
    near(at(&mut cx), (350., 353.));

    // Scrolling the result back above the conflict moves the list with it.
    wheel(&mut cx, over_result, -3000.);
    let (list, result) = at(&mut cx);
    assert!(result < 300., "the result moved up, to line {result}");
    near((list, result), (result, result));
}

#[gpui_kit::test]
fn typing_over_picked_lines_keeps_what_was_typed(cx: &mut TestAppContext) {
    use gitclient_core::repo::conflict::Side;
    let (_repo, view, mut cx) = resolving_a_conflict(cx);
    let editor = view.read_with(&cx, |view, _| view.conflict_editor().unwrap());
    view.update_in(&mut cx, |view, window, cx| {
        view.use_chunk(0, Side::Mine, window, cx)
    });

    // Typing over the picked line, and a check once typing pauses.
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(4..8, cx);
            editor.replace("typed", window, cx);
        })
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();

    // The ticks no longer make it up, so ticking leaves the typing alone.
    view.update_in(&mut cx, |view, window, cx| {
        assert!(!view.ticks_apply("a", 0));
        view.toggle_line((0, Side::Other, 0), window, cx);
    });
    let text = editor.read_with(&cx, |editor, _| editor.text().to_string());
    assert_eq!(text, "top\ntyped\nbottom\n");
}

#[gpui_kit::test]
fn typing_in_the_result_is_checked_once_it_pauses(cx: &mut TestAppContext) {
    let (repo, view, mut cx) = resolving_a_conflict(cx);
    let markers = repo.read("a");
    let editor = view.read_with(&cx, |view, _| view.conflict_editor().unwrap());
    let type_in = |text: &str, cx: &mut VisualTestContext| {
        let text = text.to_owned();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let end = editor.text().len();
                editor.set_selected_range(0..end, cx);
                editor.replace(text, window, cx);
            })
        });
    };

    // A key only marks the result as changed; reading it waits for typing to pause.
    type_in("top\nby hand\nbottom\n", &mut cx);
    cx.run_until_parked();
    assert_eq!(view.read_with(&cx, |view, _| view.conflicts_left("a")), 1);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(view.read_with(&cx, |view, _| view.conflicts_left("a")), 0);

    // Saving straight after typing reads the newest text, even before a check has run.
    type_in(&markers, &mut cx);
    view.update(&mut cx, |view, cx| view.save_conflict(cx));
    cx.run_until_parked();
    assert_eq!(
        repo.read("a"),
        markers,
        "a file with its markers is not saved"
    );
    type_in("top\nfinal\nbottom\n", &mut cx);
    view.update(&mut cx, |view, cx| view.save_conflict(cx));
    cx.run_until_parked();
    assert_eq!(repo.read("a"), "top\nfinal\nbottom\n");
}

#[gpui_kit::test]
fn a_saved_conflict_counts_as_resolved_before_git_is_done(cx: &mut TestAppContext) {
    use gitclient_core::repo::conflict::Side;
    let (_repo, view, mut cx) = resolving_a_conflict(cx);
    view.update_in(&mut cx, |view, window, cx| {
        view.use_chunk(0, Side::Mine, window, cx)
    });
    view.update(&mut cx, |view, cx| view.save_conflict(cx));

    // Nothing has run yet, and the screen already has the file resolved and has moved on,
    // with a spinner on it. The graph isn't read again for a file that moves no branch.
    view.read_with(&cx, |view, _| {
        assert_eq!(view.saved_files(), [("a".to_owned(), true)]);
        assert!(!view.resolver_is_open(), "the last file is done");
        assert_eq!(*view.status(), Status::Loaded);
    });

    // Once git has it and has been read again, the spinner goes and the file stays listed.
    cx.run_until_parked();
    view.read_with(&cx, |view, _| {
        assert_eq!(view.saved_files(), [("a".to_owned(), false)]);
        assert_eq!(*view.status(), Status::Loaded, "no reload of the graph");
        assert!(view.notice_text().is_some_and(|s| s.contains("resolved")));
    });
}

#[gpui_kit::test]
fn a_file_resolved_before_the_window_knew_of_the_stop_is_still_listed(cx: &mut TestAppContext) {
    let (repo, view, mut cx) = resolving_a_conflict(cx);
    assert!(view.read_with(&cx, |view, _| view.saved_files().is_empty()));

    // Resolved with git itself, as in an earlier session of the app.
    repo.write("a", "top\nby hand\nbottom\n");
    repo.git(&["add", "a"]);
    view.update(&mut cx, |view, cx| {
        view.check_paused_operation(cx);
        view.reload_working(cx);
    });
    cx.run_until_parked();

    view.read_with(&cx, |view, _| {
        assert_eq!(view.saved_files(), [("a".to_owned(), false)]);
        let staged = &view.working_changes().unwrap().staged;
        assert_eq!(staged.len(), 1, "the staged list has it too");
    });
}

#[gpui_kit::test]
fn a_conflict_git_refuses_to_save_comes_back_with_the_reason(cx: &mut TestAppContext) {
    use gitclient_core::repo::conflict::Side;
    let (repo, view, mut cx) = resolving_a_conflict(cx);
    view.update_in(&mut cx, |view, window, cx| {
        view.use_chunk(0, Side::Mine, window, cx)
    });
    // With the index locked, `git add` fails.
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    view.update(&mut cx, |view, cx| view.save_conflict(cx));
    assert_eq!(view.read_with(&cx, |view, _| view.saved_files().len()), 1);
    cx.run_until_parked();

    view.read_with(&cx, |view, _| {
        assert!(
            view.saved_files().is_empty(),
            "it is not resolved after all"
        );
        assert!(view.resolver_is_open(), "the file is open again");
        assert!(matches!(view.notice, Some(super::Notice::Error(_))));
    });

    // Once the lock is gone, the same file saves.
    std::fs::remove_file(&lock).unwrap();
    view.update(&mut cx, |view, cx| view.save_conflict(cx));
    cx.run_until_parked();
    view.read_with(&cx, |view, _| {
        assert_eq!(view.saved_files(), [("a".to_owned(), false)]);
    });
}

#[gpui_kit::test]
fn dragging_the_inspector_edge_resizes_it_within_limits(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b", "c"], &mut cx);
    cx.run_until_parked();
    let width = cx.update(|window, _| f32::from(window.viewport_size().width));
    let width_of = |cx: &VisualTestContext| view.read_with(cx, |view, _| view.inspector.width);
    assert_eq!(width_of(&cx), None);

    let edge = point(px(width - 380.), px(300.));
    cx.simulate_mouse_down(edge, gpui_kit::MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(edge.x - px(60.), edge.y),
        gpui_kit::MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(edge.x - px(100.), edge.y),
        gpui_kit::MouseButton::Left,
        Modifiers::none(),
    );
    let dragged = width_of(&cx).expect("the drag set a width");
    assert!((dragged - 480.).abs() < 2., "got {dragged}");

    // Far to the left it stops, so the graph keeps room.
    cx.simulate_mouse_move(
        point(px(0.), edge.y),
        gpui_kit::MouseButton::Left,
        Modifiers::none(),
    );
    let widest = width_of(&cx).unwrap();
    assert!(widest < width - 300., "got {widest}");
    cx.simulate_mouse_up(
        point(px(0.), edge.y),
        gpui_kit::MouseButton::Left,
        Modifiers::none(),
    );
}

#[gpui_kit::test]
fn ctrl_and_shift_clicks_select_several_commits(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b", "c", "d", "e"], &mut cx);
    let subjects = |view: &GraphView| -> Vec<String> {
        view.selection
            .commits(&view.entries, view.selected)
            .iter()
            .map(|c| c.subject.to_string())
            .collect()
    };
    view.update(&mut cx, |view, cx| {
        view.click_row(1, Modifiers::none(), cx);
        view.click_row(3, Modifiers::secondary_key(cx), cx);
        assert_eq!(subjects(view), ["b", "d"]);
        assert_eq!(view.selected, Some(1), "the inspector stays on the first");

        view.click_row(3, Modifiers::secondary_key(cx), cx);
        assert_eq!(subjects(view), ["b"]);

        view.click_row(4, Modifiers::shift(), cx);
        assert_eq!(subjects(view), ["b", "c", "d", "e"]);

        // Unticking the main row passes it to another marked one.
        view.click_row(1, Modifiers::secondary_key(cx), cx);
        assert_eq!(subjects(view), ["c", "d", "e"]);
        assert_eq!(view.selected, Some(2));

        view.click_row(0, Modifiers::none(), cx);
        assert_eq!(subjects(view), ["a"], "a plain click starts over");
    });
}

#[gpui_kit::test]
fn right_clicking_opens_a_menu_for_the_whole_selection_and_escape_closes_it(
    cx: &mut TestAppContext,
) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b", "c"], &mut cx);
    let labels = |view: &GraphView| -> Vec<String> {
        view.menu
            .as_ref()
            .map(|menu| {
                menu.lines()
                    .iter()
                    .filter_map(|line| match line {
                        gitclient_core::menu::Line::Item { label, .. } => Some(label.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    view.update(&mut cx, |view, cx| {
        view.click_row(0, Modifiers::none(), cx);
        view.click_row(1, Modifiers::secondary_key(cx), cx);
        // Inside the selection: the menu is for both commits.
        view.open_commit_menu(1, point(px(10.), px(10.)), cx);
        assert!(labels(view)[0].starts_with("Cherry-pick 2 commits"));

        // Outside it: the click selects that commit alone.
        view.open_commit_menu(2, point(px(10.), px(10.)), cx);
        assert_eq!(view.selected, Some(2));
        assert_eq!(labels(view)[0], "Check out c without a branch");

        view.close_menu(cx);
        assert!(view.menu.is_none());
    });
}

#[gpui_kit::test]
fn editing_a_remote_opens_a_form_that_starts_from_it_and_escape_closes_it(cx: &mut TestAppContext) {
    use gitclient_core::menu::command::Command;
    use gitclient_core::menu::form::Form;
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| {
        let mut refs = gitclient_core::history::refs::Refs::default();
        refs.remote_configs = vec![gitclient_core::repo::remote::Remote {
            name: "origin".into(),
            url: "git@host:a.git".into(),
            push_url: None,
        }];
        view.refs = Arc::new(refs);
        assert!(view.is_remote("origin") && !view.is_remote("feat"));

        view.open_remote_menu("origin", point(px(10.), px(10.)), cx);
        let edit = view
            .menu
            .as_ref()
            .unwrap()
            .lines()
            .iter()
            .find_map(|line| match line {
                gitclient_core::menu::Line::Item {
                    command: Some(Command::Form(form)),
                    ..
                } => Some(form.clone()),
                _ => None,
            });
        view.open_form(edit.expect("the menu edits the remote"), cx);
        assert!(view.menu.is_none(), "the form takes the menu's place");
        assert_eq!(
            view.form_values(cx),
            Some(vec!["origin".to_owned(), "git@host:a.git".to_owned()])
        );
        assert!(!view.is_confirming(), "a form is not a question");

        view.press_escape(cx);
        assert_eq!(view.form_values(cx), None);
        view.open_form(Form::Remote { original: None }, cx);
        assert_eq!(view.form_values(cx), Some(vec![String::new(); 2]));
    });
}

#[gpui_kit::test]
fn editing_a_submodule_opens_a_form_that_starts_from_it(cx: &mut TestAppContext) {
    use gitclient_core::menu::command::Command;
    use gitclient_core::menu::form::Form;
    use gitclient_core::repo::submodule::{State, Submodule};
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| {
        let mut refs = gitclient_core::history::refs::Refs::default();
        refs.submodules = vec![Submodule {
            name: "vendor/lib".into(),
            path: "vendor/lib".into(),
            folder: "/code/app/vendor/lib".into(),
            url: Some("https://example.com/lib.git".into()),
            recorded: "a".repeat(40),
            head: Some("a".repeat(40)),
            state: State::Current,
        }];
        view.refs = Arc::new(refs);

        view.open_submodule_menu("vendor/lib", point(px(10.), px(10.)), cx);
        let edit = view
            .menu
            .as_ref()
            .unwrap()
            .lines()
            .iter()
            .find_map(|line| match line {
                gitclient_core::menu::Line::Item {
                    command: Some(Command::Form(form)),
                    ..
                } => Some(form.clone()),
                _ => None,
            });
        view.open_form(edit.expect("the menu edits the submodule"), cx);
        assert_eq!(
            view.form_values(cx),
            Some(vec![
                "https://example.com/lib.git".to_owned(),
                "vendor/lib".to_owned()
            ])
        );

        view.press_escape(cx);
        view.open_form(Form::Submodule { original: None }, cx);
        assert_eq!(view.form_values(cx), Some(vec![String::new(); 2]));
    });
}

/// A view on a repository with one commit on `main`, loaded.
fn open_repo(
    cx: &mut TestAppContext,
) -> (
    gitclient_core::testing::Repo,
    Entity<GraphView>,
    VisualTestContext,
) {
    let repo = gitclient_core::testing::Repo::new();
    repo.write("a", "1\n");
    repo.commit("base");
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| view.open(repo.path().to_owned(), cx));
    wait_until_loaded(&view, &mut cx);
    (repo, view, cx)
}

#[gpui_kit::test]
fn a_new_branch_is_named_in_a_popup_and_made_on_enter(cx: &mut TestAppContext) {
    use gitclient_core::menu::form::Form;
    let (repo, view, mut cx) = open_repo(cx);
    view.update(&mut cx, |view, cx| {
        view.open_form(Form::new_branch_here(&view.refs), cx)
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(&cx, |view, _| {
        assert_eq!(
            view.form_problem().as_deref(),
            Some("Name the new branch first."),
            "an empty name is turned down before git runs"
        );
    });

    cx.simulate_input("topic");
    view.read_with(&cx, |view, _| {
        assert_eq!(view.form_problem(), None, "typing clears the problem");
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "topic");
    view.update(&mut cx, |view, cx| {
        assert_eq!(
            view.form_values(cx),
            None,
            "the form closes once git is done"
        );
        let said = view.notice_text().unwrap();
        assert!(
            said.contains("git branch -D topic"),
            "says how to undo it: {said}"
        );
    });
}

#[gpui_kit::test]
fn a_name_git_refuses_keeps_the_form_open_and_says_why(cx: &mut TestAppContext) {
    use gitclient_core::menu::form::Form;
    let (repo, view, mut cx) = open_repo(cx);
    repo.git(&["branch", "taken"]);
    view.update(&mut cx, |view, cx| {
        view.open_form(Form::new_branch_here(&view.refs), cx)
    });
    cx.run_until_parked();
    cx.simulate_input("taken");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.update(&mut cx, |view, cx| {
        assert_eq!(
            view.form_problem().as_deref(),
            Some("A branch called taken already exists.")
        );
        assert_eq!(view.form_values(cx), Some(vec!["taken".to_owned()]));
        assert_eq!(view.notice_text(), None, "the band stays quiet");
        assert!(view.busy.is_none());
    });
}

#[gpui_kit::test]
fn tab_moves_between_a_forms_fields_and_a_held_enter_does_nothing(cx: &mut TestAppContext) {
    use gitclient_core::menu::form::Form;
    let (repo, view, mut cx) = open_repo(cx);
    view.update(&mut cx, |view, cx| {
        view.open_form(Form::Remote { original: None }, cx)
    });
    cx.run_until_parked();
    cx.simulate_input("up");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("https://example.com/a.git");
    cx.simulate_keystrokes("shift-tab");
    cx.simulate_input("stream");
    view.update(&mut cx, |view, cx| {
        assert_eq!(
            view.form_values(cx),
            Some(vec![
                "upstream".to_owned(),
                "https://example.com/a.git".to_owned()
            ])
        );
        view.key_held = true;
        view.press_enter(cx);
        assert!(view.busy.is_none(), "a held key doesn't send the form");
        view.key_held = false;
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(repo.git(&["remote"]), "upstream");
    view.update(&mut cx, |view, cx| assert_eq!(view.form_values(cx), None));
}

#[gpui_kit::test]
fn a_question_from_git_is_answered_in_a_popup_and_cancelled_by_escape(cx: &mut TestAppContext) {
    use gitclient_core::askpass::Request;
    let (_repo, view, mut cx) = open_repo(cx);

    let (request, answer) = Request::new("Enter passphrase for key '/home/me/.ssh/id': ");
    view.update(&mut cx, |view, cx| view.open_ask(request, cx));
    cx.run_until_parked();
    cx.simulate_input("hunter2");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(answer.try_recv().unwrap().as_deref(), Some("hunter2"));
    view.update(&mut cx, |view, _| assert!(view.dialog.is_none()));

    let (request, answer) = Request::new("Username for 'https://example.com': ");
    view.update(&mut cx, |view, cx| view.open_ask(request, cx));
    cx.run_until_parked();
    cx.simulate_input("me");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        answer.try_recv().is_err(),
        "a cancelled question sends no answer"
    );
    assert!(answer.is_closed(), "and git is told it was cancelled");
}

#[gpui_kit::test]
fn the_keyboard_walks_a_menu_wraps_and_skips_lines_that_do_nothing(cx: &mut TestAppContext) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b"], &mut cx);
    view.update(&mut cx, |view, cx| {
        assert!(
            !view.step_menu(1, cx),
            "with no menu the keys move the selection"
        );
        let at = |view: &GraphView| view.menu.as_ref().unwrap().highlighted_label();

        view.open_commit_menu(0, point(px(10.), px(10.)), cx);
        view.step_menu(1, cx);
        assert_eq!(at(view).as_deref(), Some("Check out a without a branch"));
        view.step_menu(-1, cx);
        assert_eq!(
            at(view).as_deref(),
            Some("Copy subject"),
            "wraps to the end"
        );

        // On the branch that is checked out, the first line only explains.
        std::sync::Arc::make_mut(&mut view.refs).head_branch = Some("main".into());
        let main = Dragged {
            kind: RefKind::Branch,
            name: "main".into(),
        };
        view.open_ref_menu(main, point(px(10.), px(10.)), cx);
        view.step_menu(1, cx);
        assert_eq!(at(view).as_deref(), Some("New branch from main"));
    });
}

#[gpui_kit::test]
fn the_refs_card_stays_while_the_pointer_is_on_it_and_closes_once_it_leaves(
    cx: &mut TestAppContext,
) {
    let (view, mut cx) = open_view(cx);
    load(&view, &["a", "b"], &mut cx);
    view.update(&mut cx, |view, cx| {
        view.hovered_refs = Some(1);
        cx.notify();
    });
    cx.run_until_parked();

    view.update(&mut cx, |view, cx| {
        view.popover_hovered = true;
        view.leave_refs(1, cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(view.read_with(&cx, |v, _| v.hovered_refs), Some(1));

    view.update(&mut cx, |view, cx| {
        view.popover_hovered = false;
        view.leave_refs(1, cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(view.read_with(&cx, |v, _| v.hovered_refs), None);
}

/// A repository with `origin` at `url`, whose `origin/main` is behind `origin/feat/x`, and
/// a view on it. Nothing is fetched: the remote branches are made by hand.
fn open_repo_with_remote(
    cx: &mut TestAppContext,
    url: &str,
) -> (
    gitclient_core::testing::Repo,
    Entity<GraphView>,
    VisualTestContext,
) {
    let repo = gitclient_core::testing::Repo::new();
    repo.write("a", "1\n");
    repo.commit("base");
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.write("b", "2\n");
    let tip = repo.commit("feat: x");
    repo.git(&["update-ref", "refs/remotes/origin/feat/x", &tip]);
    repo.git(&["remote", "add", "origin", url]);
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| view.open(repo.path().to_owned(), cx));
    wait_until_loaded(&view, &mut cx);
    (repo, view, cx)
}

/// Git runs on another thread, so give it a moment rather than parking on it.
fn wait_until_loaded(view: &Entity<GraphView>, cx: &mut VisualTestContext) {
    cx.executor().allow_parking();
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(cx, |view, _| *view.status() == Status::Loaded) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(view.read_with(cx, |view, _| *view.status() == Status::Loaded));
}

#[gpui_kit::test]
fn branch_counts_arrive_after_the_load_without_holding_it_up(cx: &mut TestAppContext) {
    let repo = gitclient_core::testing::Repo::new();
    repo.write("a", "1\n");
    repo.commit("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("b", "2\n");
    repo.commit("add b");
    repo.git(&["checkout", "-q", "main"]);
    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| view.open(repo.path().to_owned(), cx));
    wait_until_loaded(&view, &mut cx);

    let counts = |view: &GraphView, name: &str| {
        view.refs
            .branches
            .iter()
            .find(|b| b.name == name)
            .unwrap()
            .ahead_behind
    };
    // A branch level with the default is known at once; the other fills in when counted.
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(&cx, |view, _| !view.refs.needs_counts()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    view.read_with(&cx, |view, _| {
        assert!(!view.refs.needs_counts());
        assert_eq!(counts(view, "side"), Some((1, 0)));
    });
}

#[gpui_kit::test]
fn the_commit_graph_hint_writes_one_and_goes_away(cx: &mut TestAppContext) {
    let (repo, view, mut cx) = open_repo(cx);
    view.update(&mut cx, |view, cx| {
        view.graph_hint = Some("Counting the branches took 7 s.".into());
        assert!(view.render_graph_hint(cx).is_some());
        view.write_commit_graph(cx);
        assert!(view.graph_hint.is_none(), "it is said once it is acted on");
    });
    cx.executor().allow_parking();
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(&cx, |view, _| view.busy.is_none()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    view.read_with(&cx, |view, _| {
        let said = view.notice_text().unwrap_or_default();
        assert!(said.starts_with("Wrote the commit graph."), "{said}");
    });
    assert!(!gitclient_core::repo::commit_graph::is_missing(repo.path()));
}

#[gpui_kit::test]
fn a_slow_step_is_explained_only_when_there_is_no_commit_graph(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (repo, view, mut cx) = open_repo(cx);
    let settle = |view: &Entity<GraphView>, cx: &mut VisualTestContext| {
        cx.executor().allow_parking();
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        view.read_with(cx, |view, _| view.graph_hint.clone())
    };

    view.update(&mut cx, |view, cx| {
        view.explain_slowness("Loading the history", Duration::from_millis(900), cx)
    });
    assert_eq!(settle(&view, &mut cx), None, "a quick step needs no word");

    view.update(&mut cx, |view, cx| {
        view.explain_slowness("Loading the history", Duration::from_secs(8), cx)
    });
    let said = settle(&view, &mut cx).expect("slow with no commit graph says why");
    assert!(
        said.starts_with("Loading the history took 8 s because"),
        "{said}"
    );

    view.update(&mut cx, |view, _| view.graph_hint = None);
    gitclient_core::repo::commit_graph::write(repo.path()).unwrap();
    view.update(&mut cx, |view, cx| {
        view.explain_slowness("Loading the history", Duration::from_secs(8), cx)
    });
    assert_eq!(
        settle(&view, &mut cx),
        None,
        "with a commit graph it is not why"
    );
}

#[gpui_kit::test]
fn a_dismissed_commit_graph_hint_stays_dismissed_for_that_repository(cx: &mut TestAppContext) {
    let (repo, view, mut cx) = open_repo(cx);
    view.update(&mut cx, |view, cx| {
        view.graph_hint = Some("slow".into());
        view.graph_hint_dismissed.insert(repo.path().to_owned());
        view.graph_hint = None;
        assert!(view.render_graph_hint(cx).is_none());
        assert!(view.graph_hint_dismissed.contains(repo.path()));
    });
}

fn remote_branch(name: &str) -> Dragged {
    Dragged {
        kind: RefKind::Remote,
        name: name.into(),
    }
}

fn menu_labels(view: &GraphView) -> Vec<String> {
    use gitclient_core::menu::Line;
    view.menu
        .as_ref()
        .unwrap()
        .lines()
        .iter()
        .filter_map(|line| match line {
            Line::Item { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn dropping_a_remote_branch_on_another_offers_a_request_on_the_forge(cx: &mut TestAppContext) {
    let (_repo, view, mut cx) = open_repo_with_remote(cx, "git@github.com:octo/cat.git");
    view.update(&mut cx, |view, cx| {
        view.drop_ref(
            remote_branch("origin/feat/x"),
            remote_branch("origin/main"),
            point(px(10.), px(10.)),
            cx,
        );
    });
    cx.run_until_parked();
    view.update(&mut cx, |view, _| {
        assert_eq!(
            menu_labels(view),
            ["Open a pull request for feat/x into main", "Copy link"]
        );
    });

    // The other way round, main has nothing feat/x lacks to ask for.
    view.update(&mut cx, |view, cx| {
        view.drop_ref(
            remote_branch("origin/main"),
            remote_branch("origin/feat/x"),
            point(px(10.), px(10.)),
            cx,
        );
    });
    cx.run_until_parked();
    view.update(&mut cx, |view, _| {
        assert_eq!(
            menu_labels(view),
            ["Nothing to propose: feat/x has all of main."]
        );
    });
}

#[gpui_kit::test]
fn a_forge_that_is_not_known_is_still_offered_with_a_question_and_explained_in_a_popup(
    cx: &mut TestAppContext,
) {
    use gitclient_core::menu::Glyph;
    // Nothing answers at this address, so no probe can say what is there.
    let (_repo, view, mut cx) = open_repo_with_remote(cx, "http://127.0.0.1:1/octo/cat.git");
    view.update(&mut cx, |view, cx| {
        view.drop_ref(
            remote_branch("origin/feat/x"),
            remote_branch("origin/main"),
            point(px(10.), px(10.)),
            cx,
        );
    });
    cx.run_until_parked();
    let ask = view.update(&mut cx, |view, _| {
        assert_eq!(menu_labels(view), ["Open a request for feat/x into main"]);
        let lines = view.menu.as_ref().unwrap().lines();
        assert_eq!(lines[1].glyph(), Some(Glyph::Unknown));
        lines[1].command().cloned().unwrap()
    });

    // Choosing it asks, in a popup, instead of doing something it can't.
    view.update(&mut cx, |view, cx| view.choose(ask, cx));
    view.update(&mut cx, |view, _| {
        assert!(view.menu.is_none());
        assert!(view.is_confirming());
    });
    cx.simulate_keystrokes("escape");
    view.update(&mut cx, |view, _| assert!(!view.is_confirming()));
}

#[gpui_kit::test]
fn the_requests_of_a_forge_are_listed_under_its_own_word_for_them(cx: &mut TestAppContext) {
    use super::sidebar::Load;
    use gitclient_core::repo::forge::{Request, Requests};
    let (_repo, view, mut cx) = open_repo_with_remote(cx, "git@gitlab.com:team/cat.git");
    view.update(&mut cx, |view, _| {
        assert_eq!(
            view.request_rows_for_test(),
            ["Merge requests"],
            "closed until opened, so nothing has been asked yet"
        );
        view.set_requests_for_test(Load::Done(Requests {
            newest: vec![
                Request {
                    number: 8,
                    tip: "1a2b3c4d5e".into(),
                    subject: Some("fix: the cat".into()),
                },
                Request {
                    number: 5,
                    tip: "9f8e7d6c5b".into(),
                    subject: None,
                },
            ],
            total: 80,
        }));
        assert_eq!(
            view.request_rows_for_test(),
            ["Merge requests", "!8 fix: the cat", "!5 9f8e7d6"]
        );
        view.set_requests_for_test(Load::Failed("Couldn't ask origin for its requests.".into()));
        assert_eq!(
            view.request_rows_for_test(),
            ["Merge requests", "Couldn't ask origin for its requests."]
        );
        view.set_requests_for_test(Load::Loading);
        assert_eq!(
            view.request_rows_for_test(),
            ["Merge requests", "Asking origin…"]
        );
    });
}

#[gpui_kit::test]
fn a_branch_made_as_an_upstream_shows_in_the_sidebar_at_once(cx: &mut TestAppContext) {
    use gitclient_core::menu::form::Form;
    let remote = gitclient_core::testing::Repo::new();
    remote.git(&["config", "receive.denyCurrentBranch", "ignore"]);
    let (repo, view, mut cx) = open_repo(cx);
    repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    view.update(&mut cx, |view, cx| view.reload(cx));
    cx.executor().allow_parking();
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(&cx, |view, _| !view.refs.remote_configs.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    view.update(&mut cx, |view, cx| {
        let form = Form::Upstream {
            branch: "main".into(),
            existing: Vec::new(),
        };
        view.open_form(form, cx)
    });
    cx.run_until_parked();
    // The cursor starts in the branch, not the remote dropdown, so Enter answers.
    cx.simulate_keystrokes("enter");
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(&cx, |view, _| !view.refs.remotes.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    view.read_with(&cx, |view, _| {
        let names: Vec<_> = view.refs.remotes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["origin/main"], "notice: {:?}", view.notice_text());
    });
}

#[gpui_kit::test]
fn a_branch_pushed_by_someone_else_shows_in_the_sidebar_after_a_fetch(cx: &mut TestAppContext) {
    let remote = gitclient_core::testing::Repo::new();
    remote.git(&["config", "receive.denyCurrentBranch", "ignore"]);
    let repo = gitclient_core::testing::Repo::new();
    repo.write("a", "1\n");
    repo.commit("base");
    repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    repo.git(&["push", "-q", "-u", "origin", "main"]);

    let (view, mut cx) = open_view(cx);
    view.update(&mut cx, |view, cx| view.open(repo.path().to_owned(), cx));
    cx.executor().allow_parking();
    let wait = |cx: &mut VisualTestContext, done: &dyn Fn(&GraphView) -> bool| {
        for _ in 0..500 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| done(view)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    wait(&mut cx, &|view| *view.status() == Status::Loaded);

    // Someone else pushes a new branch with a new commit.
    let other = gitclient_core::testing::Repo::new();
    other.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    other.git(&["fetch", "-q", "origin", "main"]);
    other.git(&["checkout", "-q", "-b", "feat/z", "FETCH_HEAD"]);
    other.write("z", "z\n");
    other.commit("add z");
    other.git(&["push", "-q", "origin", "feat/z"]);

    view.update(&mut cx, |view, cx| {
        view.run_operation("Fetching", gitclient_core::repo::status::fetch, cx)
    });
    wait(&mut cx, &|view| view.refs.remotes.len() == 2);
    view.read_with(&cx, |view, _| {
        let names: Vec<_> = view.refs.remotes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["origin/feat/z", "origin/main"]);
    });
}

#[gpui_kit::test]
fn no_section_is_drawn_for_a_remote_on_no_known_forge(cx: &mut TestAppContext) {
    let (_repo, view, cx) = open_repo_with_remote(cx, "http://127.0.0.1:1/octo/cat.git");
    view.read_with(&cx, |view, _| {
        assert!(view.request_rows_for_test().is_empty());
    });
}

#[gpui_kit::test]
fn a_repository_is_cloned_from_an_address_in_a_tab_with_nothing_open(cx: &mut TestAppContext) {
    use gitclient_core::menu::form::Form;
    let source = gitclient_core::testing::Repo::new();
    source.write("a", "1\n");
    source.commit("base");
    let into = gitclient_core::testing::Repo::new();
    let address = source.path().to_str().unwrap().to_owned();
    let (view, mut cx) = open_view(cx);
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = asked.clone();
    cx.update(|_, cx| {
        cx.subscribe(
            &view,
            move |_, event: &crate::view::OpenWorktreeInTab, _| {
                seen.borrow_mut().push(event.0.clone())
            },
        )
        .detach()
    });
    view.update(&mut cx, |view, cx| {
        let form = Form::Clone {
            address,
            into: into.path().join("code").display().to_string(),
        };
        view.open_form(form, cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.executor().allow_parking();
    for _ in 0..500 {
        cx.run_until_parked();
        if !asked.borrow().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let opened = asked.borrow().clone();
    assert_eq!(opened.len(), 1, "the clone asks for its folder to be shown");
    assert!(opened[0].join("a").is_file());
    assert_eq!(view.read_with(&cx, |view, cx| view.form_values(cx)), None);
}
