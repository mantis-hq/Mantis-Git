//! The settings window: a side panel of pages. Git config edits the user's config without
//! making them know its keys, identities give a host or folder its own name and email,
//! and SSH hosts shows which key each remote will use.
//!
//! There is no Save button. Toggles and choices apply when clicked, text applies when you
//! press Enter or leave the field, and the header says what happened.

use super::parts::{
    Edge, checkbox, chip, fit, grip, header_bar, on_resize, panel_row, section_label, title,
};
use super::parts::{TEXT_BODY, TEXT_SMALL, TEXT_STATUS};
use super::repository_name;
use crate::theme::{selected_row, tokens};
use gitclient_core::cache::keys::Caches;
use gitclient_core::settings::config::{self, Kind, Scope, Setting, Sources, Values, Version};
use gitclient_core::settings::identity::{self, Book, Choice, Identity, Match, Profile, RepoState};
use gitclient_core::settings::ssh;
use gitclient_core::settings::state::{
    self, AppState, CACHE_SIZES_MB, COMMIT_LIMITS, bytes_label, cache_size_label,
};
use gitclient_core::text::count;
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
    App,
}

impl Page {
    const ALL: [Page; 3] = [Page::GitConfig, Page::Identities, Page::App];

    fn name(self) -> &'static str {
        match self {
            Page::GitConfig => "Git config",
            Page::Identities => "Identities",
            Page::App => "App",
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

/// Names to pick from instead of typing: SSH hosts, and the keys in `~/.ssh`.
#[derive(Default)]
struct Suggestions {
    hosts: Vec<String>,
    /// As written in a config, such as `~/.ssh/id_work`.
    keys: Vec<String>,
}

fn read_suggestions(repo: Option<PathBuf>) -> Suggestions {
    Suggestions {
        hosts: ssh::host_names(repo.as_deref()),
        keys: ssh::keys()
            .iter()
            .map(|key| ssh::display_key(key))
            .collect(),
    }
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
    ssh_key: Entity<InputState>,
    /// What a new rule for this identity matches on.
    rule: Entity<InputState>,
    rule_kind: NewKind,
    adding_rule: bool,
    _events: Vec<Subscription>,
}

/// Where a picked SSH key goes.
#[derive(Clone)]
enum KeyTarget {
    Identity(String),
    New,
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
    /// The width the user dragged the side panel to.
    sidebar_width: Option<f32>,
    target: Target,
    git: Option<Result<Version, String>>,
    /// What the file being edited holds. `None` until it has been read.
    values: Option<Values>,
    sources: Sources,
    load_error: Option<String>,
    book: Book,
    repo_state: Option<Result<RepoState, String>>,
    suggestions: Option<Suggestions>,
    fields: Vec<TextField>,
    aliases: Vec<AliasRow>,
    identity_rows: Vec<IdentityRow>,
    new_name: Entity<InputState>,
    new_command: Entity<InputState>,
    new_label: Entity<InputState>,
    new_identity_name: Entity<InputState>,
    new_identity_email: Entity<InputState>,
    new_identity_key: Entity<InputState>,
    new_identity_ssh: Entity<InputState>,
    /// The app's own preferences, and the file that keeps them. There is no file when
    /// the window edits a test config, or when the system has no home folder.
    app: AppState,
    state_file: Option<PathBuf>,
    /// The caches the app runs with, which the cache settings act on.
    caches: Caches,
    /// How much the caches hold on disk, once counted.
    cache_size: Option<u64>,
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
        let new_identity_ssh = field("~/.ssh/id_work", "", window, cx);
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
            &new_identity_ssh,
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

        let state_file = matches!(base, Scope::Global)
            .then(state::default_file)
            .flatten();
        let mut this = Self {
            base,
            repo,
            page: Page::GitConfig,
            sidebar_width: None,
            target: Target::Everywhere,
            git: None,
            values: None,
            sources: Sources::default(),
            load_error: None,
            book: Book::default(),
            repo_state: None,
            suggestions: None,
            fields,
            aliases: Vec::new(),
            identity_rows: Vec::new(),
            new_name,
            new_command,
            new_label,
            new_identity_name,
            new_identity_email,
            new_identity_key,
            new_identity_ssh,
            app: state_file
                .as_deref()
                .map(AppState::load)
                .unwrap_or_default(),
            state_file,
            caches: super::shared_caches(cx),
            cache_size: None,
            feedback: Feedback::None,
            dirty: Default::default(),
            _new_inputs: new_inputs,
        };
        this.load(window, cx);
        this.load_suggestions(window, cx);
        this.load_cache_size(cx);
        this
    }

    /// Shows the page at `index` of the side panel.
    pub fn show_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(page) = Page::ALL.get(index) {
            self.page = *page;
            if *page == Page::App {
                self.load_cache_size(cx);
            }
            cx.notify();
        }
    }

    /// Whether everything the pages show has been read.
    pub fn is_loaded(&self) -> bool {
        self.values.is_some() && self.suggestions.is_some()
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
        self.load_suggestions(window, cx);
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

    fn load_suggestions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = self.repo.clone();
        Self::blocking(
            move || read_suggestions(repo),
            |this, info, _, cx| {
                this.suggestions = Some(info);
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
                sync(&row.ssh_key, profile.ssh_key.clone(), typed_in, window, cx);
                continue;
            }
            let label_input = field("What to call it", label, window, cx);
            let name = field("Name", &profile.name, window, cx);
            let email = field("Email", &profile.email, window, cx);
            let key = field("Signing key (optional)", &profile.signing_key, window, cx);
            let ssh_key = field("~/.ssh/id_work", &profile.ssh_key, window, cx);
            let rule = field(NewKind::Host.placeholder(), "", window, cx);
            let mut events = Vec::new();
            for input in [&label_input, &name, &email, &key, &ssh_key] {
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
                ssh_key,
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
            ssh_key: row.ssh_key.read(cx).value().trim().to_owned(),
        };
        let label = row.label.read(cx).value().trim().to_owned();
        match id {
            // The default is the global config, edited on the Git config page.
            None => {}
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
                ssh_key: self.new_identity_ssh.read(cx).value().trim().to_owned(),
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
                    &this.new_identity_ssh,
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

    /// Puts `key` in an identity's SSH key, or takes it out if it already is that. Picking
    /// is a decision, so for a saved identity it is saved at once.
    fn pick_key(
        &mut self,
        target: &KeyTarget,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match target {
            KeyTarget::Identity(id) => self
                .identity_rows
                .iter()
                .find(|row| row.id.as_deref() == Some(id))
                .map(|row| row.ssh_key.clone()),
            KeyTarget::New => Some(self.new_identity_ssh.clone()),
        };
        let Some(input) = input else {
            return;
        };
        let next = if input.read(cx).value().trim() == key {
            String::new()
        } else {
            key.to_owned()
        };
        input.update(cx, |input, cx| input.set_value(next, window, cx));
        match target {
            KeyTarget::Identity(id) => self.commit_identity(Some(id.clone()), window, cx),
            KeyTarget::New => cx.notify(),
        }
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
                    Page::App => "App preferences save as you make them.".to_owned(),
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
                .text_size(TEXT_STATUS)
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
                .text_size(TEXT_BODY)
                .child(page.name())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    this.feedback = Feedback::None;
                    cx.notify();
                }))
        });
        div()
            .v_flex()
            .w(self.sidebar_width.map_or(SIDEBAR_WIDTH, px))
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
                    .text_size(TEXT_STATUS)
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
                    .text_size(TEXT_BODY)
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
                .child(div().text_size(TEXT_BODY).text_color(color).child(text))
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
                .text_size(TEXT_SMALL)
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
                    .text_size(TEXT_BODY)
                    .line_height(px(20.))
                    .child(setting.label),
            )
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
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
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
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
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
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
                            .text_size(TEXT_BODY)
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
                .text_size(TEXT_SMALL)
                .line_height(px(17.))
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
                            .text_size(TEXT_BODY)
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
                .child(div().text_size(TEXT_BODY).font_semibold().child("Default"))
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .line_height(px(17.))
                        .text_color(muted)
                        .child("Your global name and email, used when no rule or choice picks another. Change them on the Git config page."),
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
        // The default is shown, not edited here: it is the global config, which has its
        // own page, and a card that looks like the others invites changing it by accident.
        let shown = |text: &'static str, value: &str| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(section_label(muted).child(text))
                .child(
                    div()
                        .min_h(px(32.))
                        .truncate()
                        .text_size(TEXT_BODY)
                        .line_height(px(32.))
                        .when(value.is_empty(), |value| value.text_color(muted))
                        .child(if value.is_empty() { "Not set" } else { value }.to_owned()),
                )
        };
        let fields = if id.is_none() {
            let profile = &self.book.default;
            div()
                .h_flex()
                .gap_3()
                .child(shown("Name", &profile.name))
                .child(shown("Email", &profile.email))
                .child(shown("Signing key", &profile.signing_key))
                .child(shown("SSH key", &profile.ssh_key))
        } else {
            div()
                .h_flex()
                .gap_3()
                .child(labelled("Name", &row.name))
                .child(labelled("Email", &row.email))
                .child(labelled("Signing key", &row.key))
        };
        let card = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(div().h_flex().child(head))
            .child(fields);
        match id {
            None => card.into_any_element(),
            Some(id) => card
                .child(self.render_ssh_key(
                    KeyTarget::Identity(id.clone()),
                    index,
                    &row.ssh_key,
                    cx,
                ))
                .child(self.render_rules(index, row, &id, cx))
                .into_any_element(),
        }
    }

    /// The SSH key an identity pushes and pulls with: typed as a path, or picked from the
    /// keys found in `~/.ssh`.
    fn render_ssh_key(
        &self,
        target: KeyTarget,
        slot: usize,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let current = input.read(cx).value().trim().to_owned();
        let keys = self
            .suggestions
            .as_ref()
            .map(|suggestions| suggestions.keys.clone())
            .unwrap_or_default();
        let chips: Vec<AnyElement> = keys
            .into_iter()
            .enumerate()
            .map(|(position, key)| {
                let name = key.rsplit('/').next().unwrap_or(&key).to_owned();
                let on = key == current;
                let target = target.clone();
                chip(("ssh-key", slot * 100 + position), &name, on, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_key(&target, &key, window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .v_flex()
            .gap_1()
            .child(section_label(muted).child("SSH key (optional)"))
            .child(Input::new(input))
            .when(!chips.is_empty(), |area| {
                area.child(div().flex().flex_wrap().gap_1().children(chips))
            })
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(
                        "Fetch, pull and push use this key, and only this one, wherever the identity applies.",
                    ),
            )
            .into_any_element()
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
                    .text_size(TEXT_SMALL)
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
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
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
        let known: Vec<AnyElement> = match (row.rule_kind, &self.suggestions) {
            (NewKind::Host, Some(info)) => info
                .hosts
                .iter()
                .filter(|host| !rules.contains(&Match::Host((*host).clone())))
                .enumerate()
                .map(|(position, host)| {
                    let pick = host.clone();
                    let input = row.rule.clone();
                    chip(("rule-host", index * 100 + position), host, false, cx)
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
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
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
                &self.new_identity_ssh,
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
                    .child(self.render_ssh_key(
                        KeyTarget::New,
                        usize::MAX / 200,
                        &self.new_identity_ssh,
                        cx,
                    ))
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

    /// Turns "reopen repositories on launch" on or off and saves it at once.
    fn set_reopen_tabs(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(file) = &self.state_file else {
            return;
        };
        // Read again, so that the tabs the main window saved meanwhile are kept.
        let mut state = AppState::load(file);
        state.reopen_tabs = on;
        match state.save(file) {
            Ok(()) => {
                self.feedback = Feedback::Saved(state.reopen_saved_message().to_owned());
                self.app = state;
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    /// Counts what the caches hold on disk. The count waits behind any write or trim
    /// already queued, so it is read off the UI thread.
    fn load_cache_size(&mut self, cx: &mut Context<Self>) {
        let caches = self.caches.clone();
        cx.spawn(async move |this, cx| {
            let size = cx.background_spawn(async move { caches.disk_size() }).await;
            this.update(cx, |this, cx| {
                this.cache_size = Some(size);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Saves a change to the cache settings at once and applies it to the caches in use.
    fn change_cache_setting(
        &mut self,
        change: impl FnOnce(&mut AppState),
        said: impl FnOnce(&AppState) -> String,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.state_file else {
            return;
        };
        // Read again, so that the tabs the main window saved meanwhile are kept.
        let mut state = AppState::load(file);
        change(&mut state);
        match state.save(file) {
            Ok(()) => {
                self.caches.set_disk(state.disk_settings());
                self.feedback = Feedback::Saved(said(&state));
                self.app = state;
                self.load_cache_size(cx);
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    fn set_disk_cache(&mut self, on: bool, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.disk_cache = on,
            AppState::disk_cache_saved_message,
            cx,
        );
    }

    fn set_cache_mb(&mut self, megabytes: u32, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.cache_mb = megabytes,
            AppState::cache_size_saved_message,
            cx,
        );
    }

    fn set_commit_limit(&mut self, limit: u32, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.commit_limit = limit,
            AppState::commit_limit_saved_message,
            cx,
        );
    }

    /// The "Graph" card: how many commits are loaded from a repository.
    fn render_graph_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut limits: Vec<u32> = COMMIT_LIMITS.to_vec();
        // A limit set by hand, which no chip stands for, stays visible.
        if !limits.contains(&self.app.commit_limit) {
            limits.push(self.app.commit_limit);
            limits.sort_unstable();
        }
        let chips = limits.into_iter().map(|limit| {
            chip(
                ("commit-limit", limit as usize),
                &count(limit, "commit"),
                limit == self.app.commit_limit,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_commit_limit(limit, cx)))
        });
        let note = if self.state_file.is_some() {
            self.app.commit_limit_sentence()
        } else {
            "gitclient can't find a folder to keep this in.".to_owned()
        };
        let row = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child("Commits to load"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(note),
                    ),
            )
            .children(
                self.state_file
                    .is_some()
                    .then(|| div().flex().flex_wrap().gap_1().children(chips)),
            )
            .into_any_element();
        self.render_group("Graph", vec![row], cx)
    }

    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.caches.clear();
        self.feedback = Feedback::Saved(AppState::cache_cleared_message().to_owned());
        self.load_cache_size(cx);
        cx.notify();
    }

    /// The "Cache" card: whether commits are kept on disk, how much, and how to empty it.
    fn render_cache_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let enabled = self.state_file.is_some();
        let on = self.app.disk_cache;
        let words = |title: &str, note: String| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_size(TEXT_BODY)
                        .line_height(px(20.))
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .line_height(px(17.))
                        .text_color(muted)
                        .child(note),
                )
        };

        let switch = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(words(
                "Keep a cache on disk",
                if enabled {
                    self.app.disk_cache_sentence()
                } else {
                    "gitclient can't find a folder to keep this in.".to_owned()
                },
            ))
            .children(enabled.then(|| {
                checkbox("disk-cache", on, false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_disk_cache(!on, cx)))
            }))
            .into_any_element();

        let mut sizes: Vec<u32> = CACHE_SIZES_MB.to_vec();
        // A size set by hand, which no chip stands for, stays visible.
        if !sizes.contains(&self.app.cache_mb) {
            sizes.push(self.app.cache_mb);
        }
        let chips = sizes.into_iter().map(|megabytes| {
            chip(
                ("cache-size", megabytes as usize),
                &cache_size_label(megabytes),
                megabytes == self.app.cache_mb,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_cache_mb(megabytes, cx)))
        });
        let size = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(words(
                "Size limit",
                "The oldest entries go first when the cache is full.".to_owned(),
            ))
            .child(div().flex().flex_wrap().gap_1().children(chips))
            .into_any_element();

        let used = match self.cache_size {
            Some(bytes) => format!("Using {} on disk.", bytes_label(bytes)),
            None => "Counting…".to_owned(),
        };
        let clear = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(words("Clear the cache", used))
            .child(
                Button::new("clear-cache")
                    .ghost()
                    .small()
                    .label("Clear cache")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_cache(cx))),
            )
            .into_any_element();

        let rows = if enabled && on {
            vec![switch, size, clear]
        } else if enabled {
            vec![switch, clear]
        } else {
            vec![switch]
        };
        self.render_group("Cache", rows, cx)
    }

    fn render_app(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let on = self.app.reopen_tabs;
        let words = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .child(
                div()
                    .text_size(TEXT_BODY)
                    .line_height(px(20.))
                    .child("Reopen repositories on launch"),
            )
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(match &self.state_file {
                        Some(_) => self.app.reopen_sentence(),
                        None => "gitclient can't find a folder to keep this in.",
                    }),
            );
        let switch = self.state_file.is_some().then(|| {
            checkbox("reopen-tabs", on, false, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.set_reopen_tabs(!on, cx)))
        });
        div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head(
                "App",
                "What gitclient itself remembers. Git settings are on the other pages.",
                cx,
            ))
            .child(self.render_group(
                "Startup",
                vec![
                    div()
                        .px_4()
                        .py_3()
                        .gap_3()
                        .h_flex()
                        .child(words)
                        .children(switch)
                        .into_any_element(),
                ],
                cx,
            ))
            .child(self.render_graph_group(cx))
            .child(self.render_cache_group(cx))
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
            Page::App => self.render_app(cx),
        };
        let body = div()
            .h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(self.render_sidebar(cx))
            .child(grip(Edge::Settings, true, cx))
            .child(
                div()
                    .id("settings-page")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(div().h_flex().justify_center().p_6().child(page)),
            );
        let body = on_resize(body, Edge::Settings, cx, |this, pointer, bounds| {
            let max = f32::from(bounds.size.width) - 400.;
            this.sidebar_width = Some(fit(pointer.x - bounds.left(), 160., max));
        });
        div()
            .v_flex()
            .size_full()
            .bg(background)
            .text_size(TEXT_BODY)
            .text_color(text)
            .child(self.render_header(cx))
            .child(body)
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

#[cfg(test)]
mod tests {
    use super::{Feedback, Match, Scope, Settings, Target, config, identity};
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
    fn the_default_is_listed_first_as_an_identity_but_cannot_be_changed_here(
        cx: &mut TestAppContext,
    ) {
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
    fn a_state_file_that_cannot_be_written_is_reported_and_changes_nothing(
        cx: &mut TestAppContext,
    ) {
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
}
