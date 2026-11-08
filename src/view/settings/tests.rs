use super::{Feedback, Match, Page, Scope, Settings, Target, config, identity};
use gitclient_core::settings::state::AppState;
use gitclient_core::testing::Repo;
use gpui_kit::component::input::InputEvent;
use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualTestContext};
use std::path::PathBuf;

fn open(
    repo: Option<PathBuf>,
    scope: Scope,
    cx: &mut TestAppContext,
) -> (Entity<Settings>, VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let window = cx.add_window(|window, cx| Settings::new(repo, scope, window, cx));
    let view = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    settle(&mut cx);
    (view, cx)
}

/// The tools run on real threads, so give them time to answer.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cx.run_until_parked();
}

fn file_scope() -> (tempfile::TempDir, Scope) {
    let dir = tempfile::tempdir().unwrap();
    let scope = Scope::File(dir.path().join("gitconfig"));
    (dir, scope)
}

#[gpui_kit::test]
fn changes_are_written_to_the_file_and_shown_back(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    config::set(&scope, "user.name", "Ada").unwrap();
    let (view, mut cx) = open(None, scope.clone(), cx);

    let shown = view.read_with(&cx, |view, cx| {
        view.fields
            .iter()
            .find(|field| field.key == "user.name")
            .map(|field| field.input.read(cx).value().to_string())
    });
    assert_eq!(shown.as_deref(), Some("Ada"), "existing values are loaded");
    view.read_with(&cx, |view, _| {
        assert!(matches!(view.git, Some(Ok(_))), "the git version is known");
    });

    view.update_in(&mut cx, |view, window, cx| {
        view.write("fetch.prune", "true", window, cx);
    });
    settle(&mut cx);
    assert_eq!(
        config::read(&scope).unwrap().get("fetch.prune"),
        Some("true")
    );
    view.read_with(&cx, |view, _| {
        assert!(
            matches!(&view.feedback, Feedback::Saved(message) if message.contains("fetch.prune"))
        );
    });

    view.update_in(&mut cx, |view, window, cx| {
        view.write("fetch.prune", "", window, cx);
    });
    settle(&mut cx);
    assert!(!config::read(&scope).unwrap().is_set("fetch.prune"));
}

#[gpui_kit::test]
fn a_bad_alias_says_why_and_writes_nothing(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    let (view, mut cx) = open(None, scope.clone(), cx);

    view.update_in(&mut cx, |view, window, cx| {
        view.new_name
            .update(cx, |input, cx| input.set_value("a b", window, cx));
        view.add_alias(window, cx);
    });
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        assert!(matches!(view.feedback, Feedback::Failed(_)));
    });
    assert!(config::read(&scope).unwrap().aliases().is_empty());
}

#[gpui_kit::test]
fn the_repository_scope_writes_to_the_repository_and_leaves_the_rest_alone(
    cx: &mut TestAppContext,
) {
    let repo = Repo::new();
    let (_dir, scope) = file_scope();
    config::set(&scope, "fetch.prune", "true").unwrap();
    let (view, mut cx) = open(Some(repo.path().to_owned()), scope.clone(), cx);

    view.update_in(&mut cx, |view, window, cx| {
        view.set_target(Target::Repository, window, cx);
    });
    settle(&mut cx);
    view.update_in(&mut cx, |view, window, cx| {
        view.write("fetch.prune", "false", window, cx);
    });
    settle(&mut cx);

    let local = Scope::Repository(repo.path().to_owned());
    assert_eq!(
        config::read(&local).unwrap().get("fetch.prune"),
        Some("false")
    );
    assert_eq!(
        config::read(&scope).unwrap().get("fetch.prune"),
        Some("true"),
        "the global config is untouched"
    );

    // Back to inheriting: the repository's own value goes away.
    view.update_in(&mut cx, |view, window, cx| {
        view.write("fetch.prune", "", window, cx);
    });
    settle(&mut cx);
    assert!(!config::read(&local).unwrap().is_set("fetch.prune"));
}

#[gpui_kit::test]
fn leaving_an_input_that_was_never_typed_in_does_not_erase_the_value(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    config::set(&scope, "user.name", "Ada").unwrap();
    config::set(&scope, "user.email", "ada@example.com").unwrap();

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let window = {
        let scope = scope.clone();
        cx.add_window(move |window, cx| Settings::new(None, scope, window, cx))
    };
    let view = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);

    // The first field has focus before the config has been read, as when the window
    // opens, so it is still empty when focus moves on.
    view.update_in(&mut cx, |view, window, cx| {
        let input = view.fields[0].input.clone();
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    settle(&mut cx);

    let shown = view.read_with(&cx, |view, cx| {
        view.fields[0].input.read(cx).value().to_string()
    });
    assert_eq!(shown, "Ada", "a focused input is still filled in");

    // Even an empty box, left alone, saves nothing.
    view.update_in(&mut cx, |view, window, cx| {
        let input = view.fields[0].input.clone();
        input.update(cx, |input, cx| input.set_value("", window, cx));
    });
    for field in ["user.name", "user.email"] {
        let input = view.read_with(&cx, |view, _| {
            view.fields
                .iter()
                .find(|f| f.key == field)
                .unwrap()
                .input
                .clone()
        });
        input.update(&mut cx, |_, cx| cx.emit(InputEvent::Blur));
    }
    settle(&mut cx);
    let values = config::read(&scope).unwrap();
    assert_eq!(values.get("user.name"), Some("Ada"));
    assert_eq!(values.get("user.email"), Some("ada@example.com"));
}

#[gpui_kit::test]
fn text_that_was_typed_is_saved_when_the_input_is_left(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    config::set(&scope, "user.name", "Ada").unwrap();
    let (view, mut cx) = open(None, scope.clone(), cx);

    let input = view.read_with(&cx, |view, _| view.fields[0].input.clone());
    view.update_in(&mut cx, |view, window, cx| {
        assert_eq!(view.fields[0].key, "user.name");
        let input = view.fields[0].input.clone();
        input.update(cx, |input, cx| input.set_value("Grace", window, cx));
    });
    // `set_value` is silent, as it is for the app; typing is what says "Change".
    input.update(&mut cx, |_, cx| cx.emit(InputEvent::Change));
    input.update(&mut cx, |_, cx| cx.emit(InputEvent::Blur));
    settle(&mut cx);
    assert_eq!(
        config::read(&scope).unwrap().get("user.name"),
        Some("Grace")
    );

    // Clearing it on purpose does remove it.
    view.update_in(&mut cx, |view, window, cx| {
        let input = view.fields[0].input.clone();
        input.update(cx, |input, cx| input.set_value("", window, cx));
    });
    input.update(&mut cx, |_, cx| cx.emit(InputEvent::Change));
    input.update(&mut cx, |_, cx| cx.emit(InputEvent::Blur));
    settle(&mut cx);
    assert!(!config::read(&scope).unwrap().is_set("user.name"));
}

/// Fills in the add-identity form.
fn type_identity(
    view: &Entity<Settings>,
    cx: &mut VisualTestContext,
    label: &str,
    name: &str,
    email: &str,
) {
    view.update_in(cx, |view, window, cx| {
        for (input, text) in [
            (&view.new_label, label),
            (&view.new_identity_name, name),
            (&view.new_identity_email, email),
        ] {
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
        view.add_identity(window, cx);
    });
    settle(cx);
}

#[gpui_kit::test]
fn the_default_is_listed_first_as_an_identity_but_cannot_be_changed_here(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    config::set(&scope, "user.name", "Pretext").unwrap();
    config::set(&scope, "user.email", "p@example.com").unwrap();
    let (view, mut cx) = open(None, scope.clone(), cx);
    view.read_with(&cx, |view, _| {
        assert_eq!(view.identity_rows.len(), 1);
        assert_eq!(view.identity_rows[0].id, None, "the default is first");
        assert_eq!(view.book.default.email, "p@example.com");
    });

    // Even if its inputs were somehow edited and left, nothing is saved.
    let email = view.read_with(&cx, |view, _| view.identity_rows[0].email.clone());
    view.update_in(&mut cx, |_, window, cx| {
        email.update(cx, |input, cx| {
            input.set_value("new@example.com", window, cx)
        });
    });
    email.update(&mut cx, |_, cx| cx.emit(InputEvent::Change));
    email.update(&mut cx, |_, cx| cx.emit(InputEvent::Blur));
    settle(&mut cx);
    let values = config::read(&scope).unwrap();
    assert_eq!(values.get("user.email"), Some("p@example.com"));
    assert_eq!(values.get("user.name"), Some("Pretext"));
}

#[gpui_kit::test]
fn an_identity_gets_an_ssh_key_by_typing_or_picking_and_loses_it_by_picking_again(
    cx: &mut TestAppContext,
) {
    let (dir, scope) = file_scope();
    let (view, mut cx) = open(None, scope.clone(), cx);

    // The add form carries a key.
    view.update_in(&mut cx, |view, window, cx| {
        view.new_identity_ssh.update(cx, |input, cx| {
            input.set_value("~/.ssh/id_work", window, cx)
        });
    });
    type_identity(&view, &mut cx, "Work", "Ada", "ada@work.example");
    let key = |scope: &Scope| {
        identity::book(scope)
            .unwrap()
            .identity("work")
            .map(|identity| identity.profile.ssh_key.clone())
    };
    assert_eq!(key(&scope).as_deref(), Some("~/.ssh/id_work"));
    let file = dir.path().join("identities/work.gitconfig");
    let written = config::read(&Scope::File(file)).unwrap();
    assert_eq!(
        written.get("core.sshCommand"),
        Some("ssh -i '~/.ssh/id_work' -o IdentitiesOnly=yes")
    );
    view.read_with(&cx, |view, cx| {
        assert_eq!(
            view.new_identity_ssh.read(cx).value().as_ref(),
            "",
            "form empties"
        );
        assert_eq!(
            view.identity_rows[1].ssh_key.read(cx).value().as_ref(),
            "~/.ssh/id_work"
        );
    });

    // Picking a key from the list saves it at once.
    let target = super::KeyTarget::Identity("work".into());
    view.update_in(&mut cx, |view, window, cx| {
        view.pick_key(&target, "~/.ssh/id_other", window, cx);
    });
    settle(&mut cx);
    assert_eq!(key(&scope).as_deref(), Some("~/.ssh/id_other"));

    // Picking the same one again clears it.
    view.update_in(&mut cx, |view, window, cx| {
        view.pick_key(&target, "~/.ssh/id_other", window, cx);
    });
    settle(&mut cx);
    assert_eq!(key(&scope).as_deref(), Some(""));
}

#[gpui_kit::test]
fn identities_are_added_given_rules_and_removed_with_them(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    let (view, mut cx) = open(None, scope.clone(), cx);
    type_identity(&view, &mut cx, "Work", "Ada Lovelace", "ada@work.example");

    let book = identity::book(&scope).unwrap();
    assert_eq!(book.identities.len(), 1);
    assert_eq!(book.identities[0].label, "Work");
    view.read_with(&cx, |view, cx| {
        assert_eq!(view.identity_rows.len(), 2, "the default and Work");
        assert_eq!(view.identity_rows[1].id.as_deref(), Some("work"));
        assert_eq!(
            view.new_label.read(cx).value().as_ref(),
            "",
            "the form empties"
        );
    });

    // A rule for a host, typed in the identity's own card.
    view.update_in(&mut cx, |view, window, cx| {
        view.show_rule_form("work", true, cx);
        let rule = view.identity_rows[1].rule.clone();
        rule.update(cx, |input, cx| input.set_value("github-work", window, cx));
        view.add_rule("work", window, cx);
    });
    settle(&mut cx);
    let rules = identity::book(&scope).unwrap().rules;
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].matcher, Match::Host("github-work".into()));
    view.read_with(&cx, |view, _| {
        assert!(!view.identity_rows[1].adding_rule, "the form closes");
    });

    view.update_in(&mut cx, |view, window, cx| {
        view.remove_rule(&Match::Host("github-work".into()), window, cx);
    });
    settle(&mut cx);
    assert!(identity::book(&scope).unwrap().rules.is_empty());

    view.update_in(&mut cx, |view, window, cx| {
        view.delete_identity("work", window, cx);
    });
    settle(&mut cx);
    assert!(identity::book(&scope).unwrap().identities.is_empty());
    view.read_with(&cx, |view, _| assert_eq!(view.identity_rows.len(), 1));
}

#[gpui_kit::test]
fn a_repository_is_switched_between_identities_in_one_step(cx: &mut TestAppContext) {
    let repo = Repo::new();
    let (_dir, scope) = file_scope();
    let (view, mut cx) = open(Some(repo.path().to_owned()), scope.clone(), cx);
    type_identity(&view, &mut cx, "Work", "Ada Lovelace", "ada@work.example");
    type_identity(&view, &mut cx, "Home", "Ada", "ada@home.example");

    let pinned = |cx: &VisualTestContext| {
        view.read_with(cx, |view, _| match &view.repo_state {
            Some(Ok(state)) => state.pinned.clone(),
            other => panic!("no state: {other:?}"),
        })
    };
    assert_eq!(pinned(&cx), None, "automatic at first");

    view.update_in(&mut cx, |view, window, cx| {
        view.pick_identity(
            gitclient_core::settings::identity::Choice::Identity("work".into()),
            window,
            cx,
        );
    });
    settle(&mut cx);
    assert_eq!(pinned(&cx).as_deref(), Some("work"));
    view.read_with(&cx, |view, _| {
        assert!(matches!(&view.feedback, Feedback::Saved(m) if m.contains("Work")));
    });

    view.update_in(&mut cx, |view, window, cx| {
        view.pick_identity(
            gitclient_core::settings::identity::Choice::Identity("home".into()),
            window,
            cx,
        );
    });
    settle(&mut cx);
    assert_eq!(
        pinned(&cx).as_deref(),
        Some("home"),
        "replaced, not added to"
    );

    view.update_in(&mut cx, |view, window, cx| {
        view.pick_identity(
            gitclient_core::settings::identity::Choice::Automatic,
            window,
            cx,
        );
    });
    settle(&mut cx);
    assert_eq!(pinned(&cx), None);
}

#[gpui_kit::test]
fn a_failed_identity_keeps_what_was_typed(cx: &mut TestAppContext) {
    let (_dir, scope) = file_scope();
    let (view, mut cx) = open(None, scope.clone(), cx);

    // No label, which an identity needs.
    type_identity(&view, &mut cx, "", "Ada", "ada@x.example");
    view.read_with(&cx, |view, cx| {
        assert!(matches!(view.feedback, Feedback::Failed(_)));
        assert_eq!(
            view.new_identity_email.read(cx).value().as_ref(),
            "ada@x.example"
        );
    });
    assert!(identity::book(&scope).unwrap().identities.is_empty());
}

#[gpui_kit::test]
fn the_reopen_switch_saves_at_once_and_keeps_the_remembered_tabs(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state");
    let remembered = AppState {
        tabs: vec!["/work/a".into()],
        ..Default::default()
    };
    remembered.save(&file).unwrap();
    let scope = Scope::File(dir.path().join("gitconfig"));
    let (view, mut cx) = open(None, scope, cx);

    // A window on a test config has nowhere to save, so the switch does nothing.
    view.update(&mut cx, |settings, cx| settings.set_reopen_tabs(false, cx));
    assert!(AppState::load(&file).reopen_tabs);

    view.update(&mut cx, |settings, cx| {
        settings.state_file = Some(file.clone());
        settings.set_reopen_tabs(false, cx);
    });
    let saved = AppState::load(&file);
    assert!(!saved.reopen_tabs);
    assert_eq!(saved.tabs, remembered.tabs, "the tabs are left alone");
    view.read_with(&cx, |settings, _| {
        assert!(!settings.app.reopen_tabs);
        assert!(matches!(
            &settings.feedback,
            Feedback::Saved(message) if message.contains("without")
        ));
    });

    view.update(&mut cx, |settings, cx| settings.set_reopen_tabs(true, cx));
    assert!(AppState::load(&file).reopen_tabs);
}

#[gpui_kit::test]
fn the_cache_settings_save_at_once_and_keep_the_tabs(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state");
    let remembered = AppState {
        tabs: vec!["/work/a".into()],
        ..Default::default()
    };
    remembered.save(&file).unwrap();
    let (view, mut cx) = open(None, Scope::File(dir.path().join("gitconfig")), cx);
    assert!(AppState::load(&file).disk_cache, "on unless turned off");
    assert_eq!(AppState::load(&file).cache_mb, 256);

    view.update(&mut cx, |settings, cx| {
        settings.state_file = Some(file.clone());
        settings.set_disk_cache(false, cx);
    });
    assert!(!AppState::load(&file).disk_cache);
    view.read_with(&cx, |settings, _| {
        assert!(matches!(
            &settings.feedback,
            Feedback::Saved(message) if message.contains("no longer")
        ));
    });

    view.update(&mut cx, |settings, cx| settings.set_cache_mb(1024, cx));
    let saved = AppState::load(&file);
    assert_eq!(saved.cache_mb, 1024);
    assert_eq!(saved.tabs, remembered.tabs, "the tabs are left alone");
    view.read_with(&cx, |settings, _| {
        assert!(matches!(
            &settings.feedback,
            Feedback::Saved(message) if message.contains("1 GB")
        ));
    });

    view.update(&mut cx, |settings, cx| settings.clear_cache(cx));
    view.read_with(&cx, |settings, _| {
        assert!(matches!(
            &settings.feedback,
            Feedback::Saved(message) if message.contains("Cleared")
        ));
    });
}

#[gpui_kit::test]
fn a_state_file_that_cannot_be_written_is_reported_and_changes_nothing(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    // A folder where the file should be makes the write fail.
    let file = dir.path().join("state");
    std::fs::create_dir(&file).unwrap();
    let scope = Scope::File(dir.path().join("gitconfig"));
    let (view, mut cx) = open(None, scope, cx);

    view.update(&mut cx, |settings, cx| {
        settings.state_file = Some(file);
        settings.set_reopen_tabs(false, cx);
    });
    view.read_with(&cx, |settings, _| {
        assert!(settings.app.reopen_tabs, "still on, as saved");
        assert!(matches!(settings.feedback, Feedback::Failed(_)));
    });
}

#[gpui_kit::test]
fn the_repository_page_writes_a_missing_commit_graph(cx: &mut TestAppContext) {
    use super::repository::GraphState;
    use gitclient_core::repo::commit_graph::Health;

    let repo = Repo::new();
    repo.commit("a");
    let (_dir, scope) = file_scope();
    let (view, mut cx) = open(Some(repo.path().to_owned()), scope, cx);

    view.update(&mut cx, |settings, cx| settings.show_page(3, cx));
    settle(&mut cx);
    view.read_with(&cx, |settings, _| {
        assert_eq!(settings.page, Page::Repository);
        let GraphState::Read(Ok(report)) = &settings.commit_graph else {
            panic!("the commit graph was read");
        };
        assert_eq!(report.health, Health::Missing);
    });

    view.update(&mut cx, |settings, cx| settings.write_commit_graph(cx));
    view.read_with(&cx, |settings, _| assert!(settings.writing_graph));
    settle(&mut cx);
    view.read_with(&cx, |settings, _| {
        assert!(!settings.writing_graph);
        let GraphState::Read(Ok(report)) = &settings.commit_graph else {
            panic!("the commit graph was read again");
        };
        assert_eq!(report.health, Health::UpToDate);
        assert!(matches!(
            &settings.feedback,
            Feedback::Saved(message) if message == "Wrote the commit graph. It lists all 1 commit."
        ));
    });

    // Without a repository there is no such page, and the window leaves it.
    view.update_in(&mut cx, |settings, window, cx| {
        settings.set_repository(None, window, cx)
    });
    view.read_with(&cx, |settings, _| {
        assert_eq!(settings.page, Page::GitConfig)
    });
    view.update(&mut cx, |settings, cx| settings.show_page(3, cx));
    view.read_with(&cx, |settings, _| {
        assert_eq!(settings.page, Page::GitConfig)
    });
}
