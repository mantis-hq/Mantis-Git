//! The settings window: a side panel of pages. Git config edits the user's config without
//! making them know its keys, identities give a host or folder its own name and email,
//! and SSH hosts shows which key each remote will use.
//!
//! There is no Save button. Toggles and choices apply when clicked, text applies when you
//! press Enter or leave the field, and the header says what happened.

use super::parts::{checkbox, header_bar, panel_row, section_label, title};
use super::repository_name;
use crate::theme::{selected_row, tokens};
use gitclient_core::config::{self, Kind, Scope, Setting, Sources, Values, Version};
use gitclient_core::identity::{self, Book, Choice, Identity, Match, Profile, RepoState};
use gitclient_core::ssh::{self, Host, RemoteHost};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

const SIDEBAR_WIDTH: Pixels = px(220.);
const PAGE_WIDTH: Pixels = px(720.);
const INPUT_WIDTH: Pixels = px(260.);
const NAME_WIDTH: Pixels = px(140.);

actions!(settings, [OpenSettings]);

pub(super) fn bind_keys(cx: &mut App) {
    let key = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    // The graph view handles the action, since only it knows which repository is open.
    cx.bind_keys([KeyBinding::new(key, OpenSettings, Some(super::KEY_CONTEXT))]);
}

struct SettingsWindow {
    handle: AnyWindowHandle,
    view: Entity<Settings>,
}

impl Global for SettingsWindow {}

/// Shows the settings window for `repo`, bringing the one that is already open to the
/// front.
pub fn open(repo: Option<PathBuf>, cx: &mut App) {
    if let Some(SettingsWindow { handle, view }) = cx.try_global::<SettingsWindow>() {
        let (handle, view) = (*handle, view.clone());
        let shown = handle.update(cx, |_, window, cx| {
            window.activate_window();
            view.update(cx, |settings, cx| {
                settings.set_repository(repo.clone(), window, cx)
            });
        });
        if shown.is_ok() {
            return;
        }
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(1000.), px(720.)), cx)),
        window_min_size: Some(size(px(680.), px(420.))),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        crate::theme::follow_system(window, cx);
        cx.new(|cx| Settings::new(repo, Scope::Global, window, cx))
    });
    if let Ok((handle, view)) = opened {
        cx.set_global(SettingsWindow { handle, view });
    }
}

/// Whether the app should quit now that a window closed: it should unless a window
/// other than settings is still open, since settings is only ever an add-on.
pub fn should_quit(cx: &App) -> bool {
    let settings = cx
        .try_global::<SettingsWindow>()
        .map(|settings| settings.handle.window_id());
    cx.windows()
        .iter()
        .all(|window| Some(window.window_id()) == settings)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    GitConfig,
    Identities,
    Ssh,
}

impl Page {
    const ALL: [Page; 3] = [Page::GitConfig, Page::Identities, Page::Ssh];

    fn name(self) -> &'static str {
        match self {
            Page::GitConfig => "Git config",
            Page::Identities => "Identities",
            Page::Ssh => "SSH hosts",
        }
    }
}

/// Whose settings the Git config page edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Everywhere,
    Repository,
}

/// What a new identity is matched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NewKind {
    Host,
    Folder,
    Remote,
}

impl NewKind {
    const ALL: [(NewKind, &'static str); 3] = [
        (NewKind::Host, "SSH host"),
        (NewKind::Folder, "Folder"),
        (NewKind::Remote, "Remote pattern"),
    ];

    fn placeholder(self) -> &'static str {
        match self {
            NewKind::Host => "github-work",
            NewKind::Folder => "~/work/",
            NewKind::Remote => "https://example.com/team/**",
        }
    }

    fn matcher(self, text: String) -> Match {
        match self {
            NewKind::Host => Match::Host(text),
            NewKind::Folder => Match::Folder(text),
            NewKind::Remote => Match::Remote(text),
        }
    }
}

enum Feedback {
    None,
    Saved(String),
    Failed(String),
}

/// Everything the config pages show, read off the UI thread in one go.
struct Snapshot {
    git: Result<Version, String>,
    values: Result<Values, String>,
    sources: Sources,
    book: Book,
    repo_state: Option<Result<RepoState, String>>,
}

fn snapshot(base: Scope, active: Scope, repo: Option<PathBuf>) -> Snapshot {
    let book = identity::book(&base).unwrap_or_default();
    let repo_state = repo
        .as_deref()
        .map(|repo| identity::repo_state(repo, &book));
    Snapshot {
        git: config::git_version(),
        values: config::read(&active),
        sources: config::read_sources(&active).unwrap_or_default(),
        book,
        repo_state,
    }
}

/// What `ssh` says about the hosts that matter here.
#[derive(Default)]
struct SshInfo {
    hosts: Vec<Host>,
    remotes: Vec<RemoteHost>,
    /// Why nothing could be looked up, such as OpenSSH being missing.
    problem: Option<String>,
}

fn read_ssh(repo: Option<PathBuf>) -> SshInfo {
    let mut info = SshInfo::default();
    match ssh::resolve_all(&ssh::host_names(repo.as_deref())) {
        Ok(hosts) => info.hosts = hosts,
        Err(message) => info.problem = Some(message),
    }
    info.remotes = repo
        .as_deref()
        .and_then(|repo| ssh::remotes(repo).ok())
        .unwrap_or_default();
    info
}

struct TextField {
    key: &'static str,
    input: Entity<InputState>,
    _events: Subscription,
}

struct AliasRow {
    name: String,
    input: Entity<InputState>,
    _events: Subscription,
}

/// The default, or one identity, with the inputs that edit it.
struct IdentityRow {
    /// `None` for the default.
    id: Option<String>,
    label: Entity<InputState>,
    name: Entity<InputState>,
    email: Entity<InputState>,
    key: Entity<InputState>,
    /// What a new rule for this identity matches on.
    rule: Entity<InputState>,
    rule_kind: NewKind,
    adding_rule: bool,
    _events: Vec<Subscription>,
}

/// Names a row's inputs in `dirty`, which says whether the user typed in them.
fn identity_key(id: Option<&str>) -> String {
    format!("identity:{}", id.unwrap_or("default"))
}

pub struct Settings {
    /// Where "everywhere" lives: the global config, or a file in tests.
    base: Scope,
    repo: Option<PathBuf>,
    page: Page,
    target: Target,
    git: Option<Result<Version, String>>,
    /// What the file being edited holds. `None` until it has been read.
    values: Option<Values>,
    sources: Sources,
    load_error: Option<String>,
    book: Book,
    repo_state: Option<Result<RepoState, String>>,
    ssh: Option<SshInfo>,
    fields: Vec<TextField>,
    aliases: Vec<AliasRow>,
    identity_rows: Vec<IdentityRow>,
    new_name: Entity<InputState>,
    new_command: Entity<InputState>,
    new_label: Entity<InputState>,
    new_identity_name: Entity<InputState>,
    new_identity_email: Entity<InputState>,
    new_identity_key: Entity<InputState>,
    feedback: Feedback,
    /// Inputs the user has typed in since they were last saved or filled in.
    dirty: std::collections::HashSet<String>,
    _new_inputs: Vec<Subscription>,
}

fn field(
    placeholder: &str,
    value: &str,
    window: &mut Window,
    cx: &mut Context<Settings>,
) -> Entity<InputState> {
    let (placeholder, value) = (placeholder.to_owned(), value.to_owned());
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

impl Settings {
    pub fn new(
        repo: Option<PathBuf>,
        base: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut fields = Vec::new();
        for setting in config::sections().iter().flat_map(|s| s.settings) {
            if let Kind::Text { placeholder } = setting.kind {
                let input = field(placeholder, "", window, cx);
                let key = setting.key;
                let events = cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        if this.edited(&format!("text:{key}"), event) {
                            this.commit_text(key, window, cx);
                        }
                    },
                );
                fields.push(TextField {
                    key,
                    input,
                    _events: events,
                });
            }
        }

        let new_name = field("co", "", window, cx);
        let new_command = field("checkout", "", window, cx);
        let new_label = field("Work", "", window, cx);
        let new_identity_name = field("Name", "", window, cx);
        let new_identity_email = field("Email", "", window, cx);
        let new_identity_key = field("Signing key (optional)", "", window, cx);
        let mut new_inputs = Vec::new();
        for input in [&new_name, &new_command] {
            new_inputs.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_alias(window, cx);
                    }
                    cx.notify();
                },
            ));
        }
        for input in [
            &new_label,
            &new_identity_name,
            &new_identity_email,
            &new_identity_key,
        ] {
            new_inputs.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_identity(window, cx);
                    }
                    cx.notify();
                },
            ));
        }

        let mut this = Self {
            base,
            repo,
            page: Page::GitConfig,
            target: Target::Everywhere,
            git: None,
            values: None,
            sources: Sources::default(),
            load_error: None,
            book: Book::default(),
            repo_state: None,
            ssh: None,
            fields,
            aliases: Vec::new(),
            identity_rows: Vec::new(),
            new_name,
            new_command,
            new_label,
            new_identity_name,
            new_identity_email,
            new_identity_key,
            feedback: Feedback::None,
            dirty: Default::default(),
            _new_inputs: new_inputs,
        };
        this.load(window, cx);
        this.load_ssh(window, cx);
        this
    }

    /// Shows the page at `index` of the side panel.
    pub fn show_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(page) = Page::ALL.get(index) {
            self.page = *page;
            cx.notify();
        }
    }

    /// Whether everything the pages show has been read.
    pub fn is_loaded(&self) -> bool {
        self.values.is_some() && self.ssh.is_some()
    }

    /// The config the Git config page edits.
    fn scope(&self) -> Scope {
        match (&self.target, &self.repo) {
            (Target::Repository, Some(repo)) => Scope::Repository(repo.clone()),
            _ => self.base.clone(),
        }
    }

    /// Points the window at another repository, as when it is opened from another tab.
    pub fn set_repository(
        &mut self,
        repo: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repo == repo {
            return;
        }
        self.repo = repo;
        if self.repo.is_none() {
            self.target = Target::Everywhere;
        }
        self.feedback = Feedback::None;
        self.load(window, cx);
        self.load_ssh(window, cx);
    }

    fn set_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.target != target {
            self.target = target;
            self.feedback = Feedback::None;
            self.values = None;
            self.load(window, cx);
        }
    }

    /// Runs `work` off the UI thread, then gives its result to `done`.
    fn blocking<T: Send + 'static>(
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            this.update_in(cx, |this, window, cx| done(this, result, window, cx))
                .ok();
        })
        .detach();
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (base, active, repo) = (self.base.clone(), self.scope(), self.repo.clone());
        Self::blocking(
            move || snapshot(base, active, repo),
            |this, snapshot, window, cx| this.apply(snapshot, window, cx),
            window,
            cx,
        );
    }

    fn load_ssh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        Self::blocking(
            move || read_ssh(repo),
            |this, info, _, cx| {
                this.ssh = Some(info);
                cx.notify();
            },
            window,
            cx,
        );
    }

    /// Makes `change` to the configs, then reads them back so the pages show what is
    /// true. `change` gets the config being edited and the one holding identities.
    fn change(
        &mut self,
        saved: String,
        change: impl FnOnce(&Scope, &Scope) -> Result<(), String> + Send + 'static,
        then: Option<fn(&mut Self, &mut Window, &mut Context<Self>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (base, active, repo) = (self.base.clone(), self.scope(), self.repo.clone());
        Self::blocking(
            move || {
                let done = change(&active, &base);
                (done.clone(), snapshot(base, active, repo))
            },
            move |this, (result, snapshot), window, cx| match result {
                Ok(()) => {
                    this.feedback = Feedback::Saved(saved);
                    this.apply(snapshot, window, cx);
                    if let Some(then) = then {
                        then(this, window, cx);
                    }
                }
                Err(message) => {
                    this.feedback = Feedback::Failed(message);
                    cx.notify();
                }
            },
            window,
            cx,
        );
    }

    /// Shows `snapshot`. A field being typed in keeps what is in it.
    fn apply(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.git = Some(snapshot.git);
        self.sources = snapshot.sources;
        self.repo_state = snapshot.repo_state;
        self.apply_identities(snapshot.book, window, cx);
        match snapshot.values {
            Ok(values) => {
                self.apply_values(values, window, cx);
                self.load_error = None;
            }
            Err(message) => self.load_error = Some(message),
        }
        cx.notify();
    }

    fn apply_values(&mut self, values: Values, window: &mut Window, cx: &mut Context<Self>) {
        for field in &self.fields {
            let shown = values.get(field.key).unwrap_or("").to_owned();
            let typed_in = self.dirty.contains(&format!("text:{}", field.key));
            sync(&field.input, shown, typed_in, window, cx);
        }

        let wanted = values.aliases();
        self.aliases
            .retain(|row| wanted.iter().any(|alias| alias.name == row.name));
        for alias in wanted {
            match self.aliases.iter().find(|row| row.name == alias.name) {
                Some(row) => {
                    let typed_in = self.dirty.contains(&format!("alias:{}", row.name));
                    sync(&row.input, alias.command, typed_in, window, cx);
                }
                None => {
                    let input = field("What git runs", &alias.command, window, cx);
                    let name = alias.name.clone();
                    let events = cx.subscribe_in(
                        &input,
                        window,
                        move |this, _, event: &InputEvent, window, cx| {
                            if this.edited(&format!("alias:{name}"), event) {
                                this.commit_alias(&name, window, cx);
                            }
                        },
                    );
                    self.aliases.push(AliasRow {
                        name: alias.name,
                        input,
                        _events: events,
                    });
                }
            }
        }
        self.aliases.sort_by(|a, b| a.name.cmp(&b.name));
        self.values = Some(values);
    }

    /// Shows the default and the identities, as rows. A row being typed in keeps what
    /// is in it.
    fn apply_identities(&mut self, book: Book, window: &mut Window, cx: &mut Context<Self>) {
        let mut wanted: Vec<(Option<String>, String, Profile)> =
            vec![(None, "Default".to_owned(), book.default.clone())];
        for identity in &book.identities {
            wanted.push((
                Some(identity.id.clone()),
                identity.label.clone(),
                identity.profile.clone(),
            ));
        }
        self.identity_rows
            .retain(|row| wanted.iter().any(|(id, ..)| *id == row.id));

        for (id, label, profile) in &wanted {
            let typed_in = self.dirty.contains(&identity_key(id.as_deref()));
            if let Some(row) = self.identity_rows.iter().find(|row| row.id == *id) {
                sync(&row.label, label.clone(), typed_in, window, cx);
                sync(&row.name, profile.name.clone(), typed_in, window, cx);
                sync(&row.email, profile.email.clone(), typed_in, window, cx);
                sync(&row.key, profile.signing_key.clone(), typed_in, window, cx);
                continue;
            }
            let label_input = field("What to call it", label, window, cx);
            let name = field("Name", &profile.name, window, cx);
            let email = field("Email", &profile.email, window, cx);
            let key = field("Signing key (optional)", &profile.signing_key, window, cx);
            let rule = field(NewKind::Host.placeholder(), "", window, cx);
            let mut events = Vec::new();
            for input in [&label_input, &name, &email, &key] {
                let id = id.clone();
                events.push(cx.subscribe_in(
                    input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        if this.edited(&identity_key(id.as_deref()), event) {
                            this.commit_identity(id.clone(), window, cx);
                        }
                    },
                ));
            }
            if let Some(id) = id.clone() {
                events.push(cx.subscribe_in(
                    &rule,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.add_rule(&id, window, cx);
                        }
                        cx.notify();
                    },
                ));
            }
            self.identity_rows.push(IdentityRow {
                id: id.clone(),
                label: label_input,
                name,
                email,
                key,
                rule,
                rule_kind: NewKind::Host,
                adding_rule: false,
                _events: events,
            });
        }
        // The default first, then in the order of the book.
        self.identity_rows.sort_by_key(|row| {
            wanted
                .iter()
                .position(|(id, ..)| *id == row.id)
                .unwrap_or(usize::MAX)
        });
        self.book = book;
    }

    fn write(
        &mut self,
        key: &'static str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = config::saved_message(key, Some(value));
        let value = value.to_owned();
        self.change(
            saved,
            move |scope, _| config::set(scope, key, &value),
            None,
            window,
            cx,
        );
    }

    /// Notes what happened to an input, and says whether to save it now. Only text the
    /// user typed is ever saved: an input that was merely focused and left, which may
    /// not even have been filled in yet, must not overwrite what the file holds.
    fn edited(&mut self, id: &str, event: &InputEvent) -> bool {
        match event {
            InputEvent::Change => {
                self.dirty.insert(id.to_owned());
                false
            }
            event if commits(event) => self.dirty.remove(id),
            _ => false,
        }
    }

    fn commit_text(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.fields.iter().find(|field| field.key == key) else {
            return;
        };
        let typed = field.input.read(cx).value().trim().to_owned();
        let current = self.values.as_ref().and_then(|values| values.get(key));
        if current.unwrap_or("") != typed {
            self.write(key, &typed, window, cx);
        }
    }

    fn commit_alias(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.aliases.iter().find(|row| row.name == name) else {
            return;
        };
        let typed = row.input.read(cx).value().trim().to_owned();
        let current = self.values.as_ref().and_then(|values| {
            values
                .aliases()
                .into_iter()
                .find(|alias| alias.name == name)
                .map(|alias| alias.command)
        });
        if current.as_deref() == Some(typed.as_str()) {
            return;
        }
        let name = name.to_owned();
        let saved = format!("Saved git {name} as {typed}.");
        self.change(
            saved,
            move |scope, _| config::set_alias(scope, &name, &typed),
            None,
            window,
            cx,
        );
    }

    fn add_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_owned();
        let command = self.new_command.read(cx).value().trim().to_owned();
        if let Some(problem) = config::alias_problem(&name) {
            self.feedback = Feedback::Failed(problem.to_owned());
            cx.notify();
            return;
        }
        let saved = format!("Added git {name}.");
        self.change(
            saved,
            move |scope, _| config::set_alias(scope, &name, &command),
            Some(|this, window, cx| {
                for input in [&this.new_name, &this.new_command] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }),
            window,
            cx,
        );
    }

    fn remove_alias(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        let saved = format!("Removed git {name}.");
        self.change(
            saved,
            move |scope, _| config::remove_alias(scope, &name),
            None,
            window,
            cx,
        );
    }

    /// Saves what was typed for the default (`None`) or an identity. The label of the
    /// default is fixed.
    fn commit_identity(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.identity_rows.iter().find(|row| row.id == id) else {
            return;
        };
        let profile = Profile {
            name: row.name.read(cx).value().trim().to_owned(),
            email: row.email.read(cx).value().trim().to_owned(),
            signing_key: row.key.read(cx).value().trim().to_owned(),
        };
        let label = row.label.read(cx).value().trim().to_owned();
        match id {
            None => {
                if profile == self.book.default {
                    return;
                }
                let saved = format!("Saved the default, {}.", profile.display());
                self.change(
                    saved,
                    move |_, base| identity::save_default(base, &profile),
                    None,
                    window,
                    cx,
                );
            }
            Some(id) => {
                let unchanged = self
                    .book
                    .identity(&id)
                    .is_some_and(|known| known.label == label && known.profile == profile);
                if unchanged {
                    return;
                }
                let saved = format!("Saved {label}.");
                let identity = Identity { id, label, profile };
                self.change(
                    saved,
                    move |_, base| identity::save_identity(base, &identity).map(drop),
                    None,
                    window,
                    cx,
                );
            }
        }
    }

    fn add_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let identity = Identity {
            id: String::new(),
            label: self.new_label.read(cx).value().trim().to_owned(),
            profile: Profile {
                name: self.new_identity_name.read(cx).value().trim().to_owned(),
                email: self.new_identity_email.read(cx).value().trim().to_owned(),
                signing_key: self.new_identity_key.read(cx).value().trim().to_owned(),
            },
        };
        let saved = format!("Added the identity {}.", identity.label);
        self.change(
            saved,
            move |_, base| identity::save_identity(base, &identity).map(drop),
            Some(|this, window, cx| {
                for input in [
                    &this.new_label,
                    &this.new_identity_name,
                    &this.new_identity_email,
                    &this.new_identity_key,
                ] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }),
            window,
            cx,
        );
    }

    fn delete_identity(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let label = self
            .book
            .identity(id)
            .map_or_else(|| id.to_owned(), |identity| identity.label.clone());
        let id = id.to_owned();
        let saved = format!("Removed {label}, and the rules that picked it.");
        self.change(
            saved,
            move |_, base| identity::delete_identity(base, &id),
            None,
            window,
            cx,
        );
    }

    fn show_rule_form(&mut self, id: &str, shown: bool, cx: &mut Context<Self>) {
        if let Some(row) = self
            .identity_rows
            .iter_mut()
            .find(|row| row.id.as_deref() == Some(id))
        {
            row.adding_rule = shown;
            cx.notify();
        }
    }

    fn set_rule_kind(
        &mut self,
        id: &str,
        kind: NewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self
            .identity_rows
            .iter_mut()
            .find(|row| row.id.as_deref() == Some(id))
        {
            row.rule_kind = kind;
            row.rule.update(cx, |input, cx| {
                input.set_placeholder(kind.placeholder(), window, cx)
            });
            cx.notify();
        }
    }

    fn add_rule(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .identity_rows
            .iter()
            .find(|row| row.id.as_deref() == Some(id))
        else {
            return;
        };
        let matcher = row
            .rule_kind
            .matcher(row.rule.read(cx).value().trim().to_owned());
        let label = self
            .book
            .identity(id)
            .map_or_else(|| id.to_owned(), |identity| identity.label.clone());
        let saved = format!("{label} is now picked for {}.", matcher.tidy().describe());
        let id = id.to_owned();
        self.change(
            saved,
            move |_, base| identity::add_rule(base, &id, &matcher),
            Some(|this, window, cx| {
                for row in &mut this.identity_rows {
                    row.adding_rule = false;
                    row.rule
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }),
            window,
            cx,
        );
    }

    fn remove_rule(&mut self, matcher: &Match, window: &mut Window, cx: &mut Context<Self>) {
        let matcher = matcher.clone();
        let saved = format!("No identity is picked for {} any more.", matcher.describe());
        self.change(
            saved,
            move |_, base| identity::remove_rule(base, &matcher),
            None,
            window,
            cx,
        );
    }

    /// Makes the open repository use `choice`.
    fn pick_identity(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let name = repository_name(&repo);
        let kept = match &self.repo_state {
            Some(Ok(state)) => state
                .own
                .as_ref()
                .is_some_and(|own| !self.book.has_profile(own)),
            _ => false,
        };
        let mut saved = match &choice {
            Choice::Automatic => format!("{name} now follows the rules."),
            Choice::Identity(id) => {
                let label = self
                    .book
                    .identity(id)
                    .map_or(id.as_str(), |identity| identity.label.as_str());
                format!("{name} now commits as {label}.")
            }
        };
        if kept {
            saved += " Its own name and email were kept as an identity.";
        }
        self.change(
            saved,
            move |_, base| identity::pick(base, &repo, &choice),
            None,
            window,
            cx,
        );
    }

    fn render_header(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (text, color) = match &self.feedback {
            Feedback::None => (
                match self.page {
                    Page::GitConfig => format!(
                        "Editing the {}. Changes save as you make them.",
                        self.scope().name()
                    ),
                    Page::Identities => {
                        "Identities are kept next to your global git config and save as you make them."
                            .to_owned()
                    }
                    Page::Ssh => "What ssh will do for each host. Read only.".to_owned(),
                },
                tokens(theme.is_dark()).subtext0,
            ),
            Feedback::Saved(message) => (message.clone(), theme.success),
            Feedback::Failed(message) => (message.clone(), theme.danger),
        };
        header_bar(cx).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(15.))
                .text_color(color)
                .child(text),
        )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.list_head, theme.border, theme.muted_foreground);
        let pages = Page::ALL.into_iter().enumerate().map(|(index, page)| {
            panel_row(("page", index), self.page == page, cx)
                .h(px(32.))
                .text_size(px(14.))
                .child(page.name())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    this.feedback = Feedback::None;
                    cx.notify();
                }))
        });
        div()
            .v_flex()
            .w(SIDEBAR_WIDTH)
            .flex_none()
            .p_2()
            .gap_1()
            .bg(panel)
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .child(section_label(muted).child("Settings")),
            )
            .children(pages)
    }

    /// The top of a page: its name, and a sentence about it.
    fn page_head(&self, name: &str, sentence: &str, cx: &App) -> Div {
        let subtext = tokens(cx.theme().is_dark()).subtext0;
        div()
            .v_flex()
            .gap_1()
            .child(title().child(name.to_owned()))
            .child(
                div()
                    .text_size(px(15.))
                    .line_height(px(20.))
                    .text_color(subtext)
                    .child(sentence.to_owned()),
            )
    }

    /// What replaces a page when git can't be asked.
    fn render_no_git(&self, message: &str, cx: &mut Context<Self>) -> AnyElement {
        let danger = cx.theme().danger;
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(danger)
                    .child(message.to_owned()),
            )
            .child(
                div().h_flex().child(
                    Button::new("try-git-again")
                        .ghost()
                        .label("Try again")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.git = None;
                            this.load(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_git_config(&self, cx: &mut Context<Self>) -> AnyElement {
        let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
        let sentence = match self.target {
            Target::Repository if self.repo.is_some() => {
                "These apply only to this repository, and win over your global config."
            }
            _ => "These apply to every repository on this computer.",
        };
        let mut page = div()
            .v_flex()
            .w_full()
            .max_w(PAGE_WIDTH)
            .gap_6()
            .child(self.page_head("Git config", sentence, cx))
            .child(self.render_target(cx));

        if let Some(Err(message)) = &self.git {
            return page
                .child(self.render_no_git(message, cx))
                .into_any_element();
        }
        let Some(values) = self.values.clone() else {
            let (text, color) = match &self.load_error {
                Some(error) => (error.clone(), danger),
                None => ("Reading your git config…".to_owned(), muted),
            };
            return page
                .child(div().text_size(px(14.)).text_color(color).child(text))
                .into_any_element();
        };

        let version = self.git.as_ref().and_then(|git| git.as_ref().ok()).copied();
        for (index, section) in config::sections().iter().enumerate() {
            let rows: Vec<AnyElement> = section
                .settings
                .iter()
                .enumerate()
                .map(|(row, setting)| {
                    self.render_setting(index * 100 + row, setting, &values, version, cx)
                })
                .collect();
            page = page.child(self.render_group(section.title, rows, cx));
        }
        page.child(self.render_aliases(cx)).into_any_element()
    }

    /// Everywhere, or only the open repository.
    fn render_target(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(repo) = self.repo.as_deref() else {
            return div()
                .text_size(px(13.))
                .text_color(muted)
                .child("Open a repository to change its settings on their own.")
                .into_any_element();
        };
        let name = repository_name(repo);
        div()
            .h_flex()
            .gap_1()
            .child(
                chip(
                    "target-everywhere",
                    "Everywhere",
                    self.target == Target::Everywhere,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_target(Target::Everywhere, window, cx)
                })),
            )
            .child(
                chip(
                    "target-repository",
                    &format!("Only {name}"),
                    self.target == Target::Repository,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_target(Target::Repository, window, cx)
                })),
            )
            .into_any_element()
    }

    /// A titled card holding rows, with a line between them.
    fn render_group(&self, name: &str, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.list_head, theme.border, theme.muted_foreground);
        let rows = rows.into_iter().enumerate().map(|(index, row)| {
            div()
                .when(index > 0, |row| row.border_t_1().border_color(border))
                .child(row)
        });
        div()
            .v_flex()
            .gap_2()
            .child(section_label(muted).child(name.to_owned()))
            .child(
                div()
                    .v_flex()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(border)
                    .bg(panel)
                    .overflow_hidden()
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_setting(
        &self,
        id: usize,
        setting: &Setting,
        values: &Values,
        version: Option<Version>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let setting = *setting;
        let scope = self.scope();
        let muted = cx.theme().muted_foreground;
        let inherited = self.sources.inherited(setting.key, values, &scope);
        let too_old = version.and_then(|git| setting.unavailable(git));
        let hint = too_old.clone().unwrap_or_else(|| setting.hint.to_owned());
        let words = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .when(too_old.is_some(), |words| words.opacity(0.6))
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .child(setting.label),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(muted)
                    .child(hint),
            )
            .children(
                inherited
                    .as_ref()
                    .filter(|_| too_old.is_none())
                    .map(|(value, note)| {
                        let shown = if value.is_empty() {
                            note.clone()
                        } else {
                            format!("{} {value}", note.trim_end_matches('.').to_owned() + ":")
                        };
                        div()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .text_color(muted)
                            .child(shown)
                    }),
            );
        let row = div().px_4().py_3().gap_3();
        if too_old.is_some() {
            return row.h_flex().child(words).into_any_element();
        }

        match setting.kind {
            Kind::Text { .. } => {
                let input = self
                    .fields
                    .iter()
                    .find(|field| field.key == setting.key)
                    .map(|field| field.input.clone());
                row.h_flex()
                    .gap_4()
                    .child(words)
                    .children(
                        input.map(|input| {
                            div().w(INPUT_WIDTH).flex_none().child(Input::new(&input))
                        }),
                    )
                    .into_any_element()
            }
            Kind::Toggle { .. } => {
                let on = self.sources.is_on(&setting);
                let on = if values.is_set(setting.key) {
                    values.is_on(&setting)
                } else {
                    on
                };
                let key = setting.key;
                let reset = values.is_set(key).then(|| {
                    Button::new(("reset", id))
                        .ghost()
                        .compact()
                        .label(scope.reset_label())
                        .tooltip(scope.reset_hint())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.write(key, "", window, cx);
                        }))
                });
                row.h_flex()
                    .gap_4()
                    .child(words)
                    .children(reset)
                    .child(
                        checkbox(("toggle", id), on, false, cx).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.write(key, if on { "false" } else { "true" }, window, cx);
                            },
                        )),
                    )
                    .into_any_element()
            }
            Kind::Choice { options } => {
                let key = setting.key;
                let current = values.get(key);
                let unknown = current.filter(|value| !options.iter().any(|(v, _)| v == value));
                let git = version.unwrap_or(Version(u32::MAX, 0));
                let mut chips = vec![
                    chip(
                        ("choice", id * 10),
                        scope.reset_label_for_choice(),
                        current.is_none(),
                        cx,
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.write(key, "", window, cx)),
                    ),
                ];
                for (position, (value, label)) in options.iter().enumerate() {
                    if !config::option_available(key, value, git) {
                        continue;
                    }
                    let on = current == Some(*value);
                    let value = *value;
                    chips.push(
                        chip(("choice", id * 10 + position + 1), label, on, cx).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.write(key, value, window, cx)
                            }),
                        ),
                    );
                }
                // Something set by hand, which no chip stands for, stays visible.
                if let Some(value) = unknown {
                    chips.push(chip(("choice", id * 10 + 9), value, true, cx));
                }
                row.v_flex()
                    .child(words)
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .into_any_element()
            }
        }
    }

    fn render_aliases(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        let listed = self
            .values
            .as_ref()
            .map_or(0, |values| values.aliases().len());
        if listed == 0 {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(muted)
                    .child(
                        "No aliases yet. An alias named co that runs checkout lets you type git co.",
                    )
                    .into_any_element(),
            );
        }
        for (index, row) in self.aliases.iter().enumerate() {
            let name = row.name.clone();
            rows.push(
                div()
                    .h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .child(
                        div()
                            .w(NAME_WIDTH)
                            .flex_none()
                            .truncate()
                            .text_size(px(14.))
                            .child(format!("git {}", row.name)),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&row.input)))
                    .child(
                        Button::new(("remove-alias", index))
                            .ghost()
                            .compact()
                            .label("Remove")
                            .tooltip(format!("Delete the alias git {}", row.name))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_alias(&name, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }

        let ready = config::alias_problem(self.new_name.read(cx).value().trim()).is_none()
            && !self.new_command.read(cx).value().trim().is_empty();
        rows.push(
            div()
                .h_flex()
                .px_4()
                .py_2()
                .gap_3()
                .child(
                    div()
                        .w(NAME_WIDTH)
                        .flex_none()
                        .child(Input::new(&self.new_name)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.new_command)),
                )
                .child(
                    Button::new("add-alias")
                        .ghost()
                        .compact()
                        .label("Add alias")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, window, cx| this.add_alias(window, cx))),
                )
                .into_any_element(),
        );
        self.render_group("Aliases", rows, cx)
    }

    fn render_identities(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = div().v_flex().w_full().max_w(PAGE_WIDTH).gap_6().child(
            self.page_head(
                "Identities",
                "Who you commit as. Make one for each, then pick it for a repository or let rules choose.",
                cx,
            ),
        );
        if let Some(Err(message)) = &self.git {
            return page
                .child(self.render_no_git(message, cx))
                .into_any_element();
        }
        let rows: Vec<AnyElement> = self
            .identity_rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.render_identity(index, row, cx))
            .collect();
        page.child(self.render_picker(cx))
            .child(self.render_group("Your identities", rows, cx))
            .child(self.render_new_identity(cx))
            .into_any_element()
    }

    /// The open repository's identity, and one click to change it.
    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let quiet = |text: String| {
            div()
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(muted)
                .child(text)
        };
        let Some(repo) = self.repo.as_deref() else {
            return self.render_group(
                "Right now",
                vec![
                    div()
                        .px_4()
                        .py_3()
                        .child(quiet(
                            "Open a repository to choose who it commits as.".to_owned(),
                        ))
                        .into_any_element(),
                ],
                cx,
            );
        };
        let body = match &self.repo_state {
            Some(Ok(state)) => {
                let (own, pinned) = (state.own.as_ref(), state.pinned.as_deref());
                let mut chips = vec![
                    chip(
                        "pick-automatic",
                        "Automatic",
                        own.is_none() && pinned.is_none(),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pick_identity(Choice::Automatic, window, cx)
                    }))
                    .into_any_element(),
                ];
                for (index, identity) in self.book.identities.iter().enumerate() {
                    let id = identity.id.clone();
                    let on = own.is_none() && pinned == Some(identity.id.as_str());
                    chips.push(
                        chip(("pick-identity", index), &identity.label, on, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_identity(Choice::Identity(id.clone()), window, cx)
                            }))
                            .into_any_element(),
                    );
                }
                if own.is_some() {
                    chips.push(
                        chip("pick-own", "This repository's own", true, cx).into_any_element(),
                    );
                }
                let hint = if own.is_some() {
                    "Set in this repository's own config. Picking another replaces it, and keeps it as an identity."
                } else if pinned.is_some() {
                    "Chosen for this repository only. Automatic goes back to the rules."
                } else {
                    "Rules pick an identity by host or folder. With no match, the Default applies."
                };
                div()
                    .v_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .child(state.committing.sentence()),
                    )
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .child(quiet(hint.to_owned()))
                    .into_any_element()
            }
            Some(Err(message)) => div()
                .px_4()
                .py_3()
                .child(quiet(message.clone()))
                .into_any_element(),
            None => div()
                .px_4()
                .py_3()
                .child(quiet("Reading…".to_owned()))
                .into_any_element(),
        };
        self.render_group(&repository_name(repo), vec![body], cx)
    }

    /// One identity: its name, email and key, and where it is used automatically.
    fn render_identity(
        &self,
        index: usize,
        row: &IdentityRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let id = row.id.clone();
        let labelled = |text: &'static str, input: &Entity<InputState>| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(Input::new(input))
        };
        let head = match &id {
            None => div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(14.)).font_semibold().child("Default"))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .text_color(muted)
                        .child("Your global name and email, used when no rule or choice picks another."),
                ),
            Some(id) => {
                let id = id.clone();
                div()
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(Input::new(&row.label)))
                    .child(
                        Button::new(("remove-identity", index))
                            .ghost()
                            .compact()
                            .label("Remove")
                            .tooltip("Delete this identity and its rules")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_identity(&id, window, cx)
                            })),
                    )
            }
        };
        let card = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(div().h_flex().child(head))
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(labelled("Name", &row.name))
                    .child(labelled("Email", &row.email))
                    .child(labelled("Signing key", &row.key)),
            );
        match id {
            None => card.into_any_element(),
            Some(id) => card
                .child(self.render_rules(index, row, &id, cx))
                .into_any_element(),
        }
    }

    /// Where an identity is picked automatically, and a way to add to that.
    fn render_rules(
        &self,
        index: usize,
        row: &IdentityRow,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, selected, quiet) = (
            theme.muted_foreground,
            selected_row(theme),
            theme.secondary_foreground,
        );
        let rules: Vec<Match> = self
            .book
            .rules_for(id)
            .map(|rule| rule.matcher.clone())
            .collect();
        let shown: Vec<AnyElement> = rules
            .iter()
            .enumerate()
            .map(|(position, matcher)| {
                let stop = matcher.clone();
                div()
                    .h_flex()
                    .h(px(26.))
                    .pl_2()
                    .gap_1()
                    .rounded(px(6.))
                    .bg(selected)
                    .text_size(px(13.))
                    .text_color(quiet)
                    .child(sentence_case(&matcher.describe()))
                    .child(
                        Button::new(("remove-rule", index * 100 + position))
                            .ghost()
                            .compact()
                            .label("×")
                            .tooltip("Stop picking this identity for that")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_rule(&stop, window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let mut area = div()
            .v_flex()
            .gap_2()
            .child(section_label(muted).child("Picked automatically for"));
        area = if shown.is_empty() {
            area.child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(muted)
                    .child("Nothing yet. Pick it by hand for a repository, or add a rule."),
            )
        } else {
            area.child(div().flex().flex_wrap().gap_1().children(shown))
        };

        if !row.adding_rule {
            let open = id.to_owned();
            return area
                .child(
                    div().h_flex().child(
                        Button::new(("add-rule", index))
                            .ghost()
                            .compact()
                            .label("Add a rule")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_rule_form(&open, true, cx)
                            })),
                    ),
                )
                .into_any_element();
        }

        let kinds: Vec<AnyElement> =
            NewKind::ALL
                .into_iter()
                .enumerate()
                .map(|(position, (kind, name))| {
                    let id = id.to_owned();
                    chip(
                        ("rule-kind", index * 10 + position),
                        name,
                        row.rule_kind == kind,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_rule_kind(&id, kind, window, cx)
                    }))
                    .into_any_element()
                })
                .collect();
        let known: Vec<AnyElement> = match (row.rule_kind, &self.ssh) {
            (NewKind::Host, Some(info)) => info
                .hosts
                .iter()
                .filter(|host| !rules.contains(&Match::Host(host.alias.clone())))
                .enumerate()
                .map(|(position, host)| {
                    let pick = host.alias.clone();
                    let input = row.rule.clone();
                    chip(
                        ("rule-host", index * 100 + position),
                        &host.alias,
                        false,
                        cx,
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let pick = pick.clone();
                        input.update(cx, |input, cx| input.set_value(pick, window, cx));
                    }))
                    .into_any_element()
                })
                .collect(),
            _ => Vec::new(),
        };
        let help = match row.rule_kind {
            NewKind::Host => {
                "The name after @ in a remote like git@github-work:team/app.git, as in your ssh config."
            }
            NewKind::Folder => "Every repository inside this folder.",
            NewKind::Remote => {
                "A pattern for the remote's URL. * stops at a slash and ** after a slash goes on."
            }
        };
        let ready = !row.rule.read(cx).value().trim().is_empty();
        let (add, cancel) = (id.to_owned(), id.to_owned());
        area.child(div().h_flex().gap_1().children(kinds))
            .child(Input::new(&row.rule))
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(muted)
                    .child(help),
            )
            .when(!known.is_empty(), |area| {
                area.child(div().flex().flex_wrap().gap_1().children(known))
            })
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_1()
                    .child(
                        Button::new(("cancel-rule", index))
                            .ghost()
                            .compact()
                            .label("Cancel")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_rule_form(&cancel, false, cx)
                            })),
                    )
                    .child(
                        Button::new(("save-rule", index))
                            .ghost()
                            .compact()
                            .label("Add rule")
                            .disabled(!ready)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_rule(&add, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_new_identity(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let labelled = |text: &'static str, input: &Entity<InputState>| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(Input::new(input))
        };
        let ready = !self.new_label.read(cx).value().trim().is_empty()
            && [
                &self.new_identity_name,
                &self.new_identity_email,
                &self.new_identity_key,
            ]
            .iter()
            .any(|input| !input.read(cx).value().trim().is_empty());
        self.render_group(
            "Add an identity",
            vec![
                div()
                    .v_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(labelled("What to call it", &self.new_label))
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .child(labelled("Name", &self.new_identity_name))
                            .child(labelled("Email", &self.new_identity_email))
                            .child(labelled("Signing key", &self.new_identity_key)),
                    )
                    .child(
                        div().h_flex().justify_end().child(
                            Button::new("add-identity")
                                .ghost()
                                .compact()
                                .label("Add identity")
                                .disabled(!ready)
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.add_identity(window, cx)
                                    }),
                                ),
                        ),
                    )
                    .into_any_element(),
            ],
            cx,
        )
    }

    fn render_ssh(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let page = div()
            .v_flex()
            .w_full()
            .max_w(PAGE_WIDTH)
            .gap_6()
            .child(
                div()
                    .h_flex()
                    .gap_4()
                    .items_start()
                    .child(
                        div().flex_1().min_w_0().child(self.page_head(
                            "SSH hosts",
                            "What ssh does for each host, as the installed OpenSSH reports it. Edit your ssh config to change it.",
                            cx,
                        )),
                    )
                    .children(ssh::config_file().filter(|file| file.exists()).map(|file| {
                        Button::new("show-ssh-config")
                            .ghost()
                            .label("Show ssh config")
                            .tooltip(file.display().to_string())
                            .on_click(move |_, _, cx| cx.reveal_path(&file))
                    })),
            );
        let Some(info) = &self.ssh else {
            return page
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(muted)
                        .child("Asking ssh…"),
                )
                .into_any_element();
        };
        let mut page = page;
        if let Some(problem) = &info.problem {
            page = page.child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(danger)
                    .child(problem.clone()),
            );
        }

        let host_line = |host: &Host| {
            let keys = if host.keys.is_empty() {
                "no key".to_owned()
            } else if host.default_keys {
                "OpenSSH's default keys".to_owned()
            } else {
                format!("key {}", host.key_names().join(", "))
            };
            format!("{} · {keys}", host.login())
        };

        if self.repo.is_some() {
            let rows: Vec<AnyElement> = if info.remotes.is_empty() {
                vec![
                    div()
                        .px_4()
                        .py_3()
                        .text_size(px(13.))
                        .text_color(muted)
                        .child("This repository has no remotes.")
                        .into_any_element(),
                ]
            } else {
                info.remotes
                    .iter()
                    .map(|remote| {
                        let detail = match &remote.alias {
                            None => "Not SSH, so no key is involved.".to_owned(),
                            Some(alias) => info
                                .hosts
                                .iter()
                                .find(|host| host.alias == *alias)
                                .map_or_else(
                                    || format!("Host {alias} couldn't be looked up."),
                                    |host| format!("Host {alias} → {}", host_line(host)),
                                ),
                        };
                        div()
                            .v_flex()
                            .px_4()
                            .py_3()
                            .gap_0p5()
                            .child(
                                div()
                                    .h_flex()
                                    .gap_3()
                                    .text_size(px(14.))
                                    .child(div().flex_none().child(remote.remote.clone()))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(muted)
                                            .child(remote.url.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(18.))
                                    .text_color(muted)
                                    .child(detail),
                            )
                            .into_any_element()
                    })
                    .collect()
            };
            page = page.child(self.render_group("Used by this repository", rows, cx));
        }

        let rows: Vec<AnyElement> = if info.hosts.is_empty() {
            vec![
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(13.))
                    .text_color(muted)
                    .child("No hosts have a block in your ssh config.")
                    .into_any_element(),
            ]
        } else {
            info.hosts
                .iter()
                .map(|host| {
                    let matcher = Match::Host(host.alias.clone());
                    let identity = self
                        .book
                        .rules
                        .iter()
                        .find(|rule| rule.matcher == matcher)
                        .and_then(|rule| self.book.identity(&rule.identity));
                    div()
                        .v_flex()
                        .px_4()
                        .py_3()
                        .gap_0p5()
                        .child(div().text_size(px(14.)).child(host.alias.clone()))
                        .child(
                            div()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .text_color(muted)
                                .child(host_line(host)),
                        )
                        .children(identity.map(|identity| {
                            div()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .text_color(muted)
                                .child(format!(
                                    "Commits as {}, {}",
                                    identity.label,
                                    identity.profile.display()
                                ))
                        }))
                        .into_any_element()
                })
                .collect()
        };
        page.child(self.render_group("Hosts", rows, cx))
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, text) = (theme.background, theme.foreground);
        let page = match self.page {
            Page::GitConfig => self.render_git_config(cx),
            Page::Identities => self.render_identities(cx),
            Page::Ssh => self.render_ssh(cx),
        };
        div()
            .v_flex()
            .size_full()
            .bg(background)
            .text_color(text)
            .child(self.render_header(cx))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .id("settings-page")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .child(div().h_flex().justify_center().p_6().child(page)),
                    ),
            )
    }
}

/// "the host work" as a heading: "The host work".
fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Whether an input has finished being edited: Enter, or leaving it.
fn commits(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

/// Puts `text` in `input`, unless the user has typed in it or it already says that.
///
/// What counts is whether it was edited, not whether it has focus: an input that had
/// focus before its value arrived must still be filled in. Otherwise leaving it would
/// "save" the empty box over the real value.
fn sync(
    input: &Entity<InputState>,
    text: String,
    typed_in: bool,
    window: &mut Window,
    cx: &mut Context<Settings>,
) {
    if !typed_in && input.read(cx).value().as_ref() != text {
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    }
}

/// One option of a choice, with a tick when picked so that colour isn't the only cue.
fn chip(id: impl Into<ElementId>, label: &str, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (selected, hover) = (selected_row(theme), theme.list_hover);
    let (text, quiet) = (theme.foreground, theme.secondary_foreground);
    div()
        .id(id)
        .h_flex()
        .h(px(26.))
        .px_2()
        .gap_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .when(on, |chip| {
            chip.bg(selected).text_color(text).font_semibold()
        })
        .when(!on, |chip| {
            chip.text_color(quiet).hover(|chip| chip.bg(hover))
        })
        .when(on, |chip| chip.child("✓"))
        .child(label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Feedback, Match, Scope, Settings, Target, config, identity};
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
    fn the_default_is_listed_first_as_an_identity_and_edits_the_global_name(
        cx: &mut TestAppContext,
    ) {
        let (_dir, scope) = file_scope();
        config::set(&scope, "user.name", "Pretext").unwrap();
        config::set(&scope, "user.email", "p@example.com").unwrap();
        let (view, mut cx) = open(None, scope.clone(), cx);
        view.read_with(&cx, |view, cx| {
            assert_eq!(view.identity_rows.len(), 1);
            let row = &view.identity_rows[0];
            assert_eq!(row.id, None, "the default is the first row");
            assert_eq!(row.email.read(cx).value().as_ref(), "p@example.com");
        });

        // Typing a new email into the default saves the global one.
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
        assert_eq!(values.get("user.email"), Some("new@example.com"));
        assert_eq!(values.get("user.name"), Some("Pretext"), "the rest is kept");
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
                gitclient_core::identity::Choice::Identity("work".into()),
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
                gitclient_core::identity::Choice::Identity("home".into()),
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
            view.pick_identity(gitclient_core::identity::Choice::Automatic, window, cx);
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
}
