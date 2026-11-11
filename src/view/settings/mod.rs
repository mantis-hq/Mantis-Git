//! The settings window: a side panel of pages. Git config edits the user's config without
//! making them know its keys, identities give a host or folder its own name and email,
//! and SSH hosts shows which key each remote will use.
//!
//! There is no Save button. Toggles and choices apply when clicked, text applies when you
//! press Enter or leave the field, and the header says what happened.

mod about;
mod app;
mod git_config;
mod identities;
mod repository;

use super::parts::{
    Edge, checkbox, chip, fit, grip, header_bar, on_resize, panel_row, section_label, title,
};
use super::parts::{TEXT_BODY, TEXT_SMALL, TEXT_STATUS};
use super::repository_name;
use super::updates::{Updates, shared_updates};
use crate::theme::{selected_row, tokens};
use gitclient_core::cache::keys::{Caches, Usage};
use gitclient_core::install::Channel;
use gitclient_core::install::menu_entry::{Launcher, Status as LauncherStatus};
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
    open_on(repo, None, cx)
}

/// Shows the settings window on the Repository page, where a repository's own settings
/// are, such as the forge it is on.
pub fn open_repository_page(repo: Option<PathBuf>, cx: &mut App) {
    open_on(repo, Some(Page::Repository), cx)
}

fn open_on(repo: Option<PathBuf>, page: Option<Page>, cx: &mut App) {
    if let Some(SettingsWindow { handle, view }) = cx.try_global::<SettingsWindow>() {
        let (handle, view) = (*handle, view.clone());
        let shown = handle.update(cx, |_, window, cx| {
            window.activate_window();
            view.update(cx, |settings, cx| {
                settings.set_repository(repo.clone(), window, cx);
                if let Some(page) = page {
                    settings.open_page(page, cx);
                }
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
        app_id: Some("gitclient".into()),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        crate::theme::follow_system(window, cx);
        cx.new(|cx| {
            let mut settings = Settings::new(repo, Scope::Global, window, cx);
            if let Some(page) = page {
                settings.open_page(page, cx);
            }
            settings
        })
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
    Repository,
    About,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::GitConfig,
        Page::Identities,
        Page::App,
        Page::Repository,
        Page::About,
    ];

    fn name(self) -> &'static str {
        match self {
            Page::GitConfig => "Git config",
            Page::Identities => "Identities",
            Page::App => "App",
            Page::Repository => "Repository",
            Page::About => "About",
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
    Default,
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
    /// What each cache holds on disk, once counted.
    cache_usage: Option<Vec<Usage>>,
    /// The application menu launcher, for a portable copy on a system that has one.
    launcher: Option<Launcher>,
    /// How the launcher stands on disk, read when the page opens and after each change.
    launcher_status: Option<LauncherStatus>,
    /// Looks for and installs newer releases, shared with the band over the main window.
    updates: Option<Entity<Updates>>,
    /// How this copy was installed, which decides whether it manages itself.
    channel: Channel,
    /// How the open repository's commit graph stands, and whether it is being written.
    commit_graph: repository::GraphState,
    writing_graph: bool,
    /// Which forge the open repository is on, once read.
    forge_choice: Option<gitclient_core::repo::forge::choice::Choice>,
    /// Which license on the About page is picked, and how its list is scrolled.
    picked_license: Option<usize>,
    licenses_scroll: UniformListScrollHandle,
    feedback: Feedback,
    /// Inputs the user has typed in since they were last saved or filled in.
    dirty: std::collections::HashSet<String>,
    _new_inputs: Vec<Subscription>,
    _updates: Option<Subscription>,
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
        let channel = Channel::detect();
        // Like the state file, the launcher is the real one only in the real window.
        let launcher = (matches!(base, Scope::Global) && channel.is_portable())
            .then(Launcher::this_system)
            .flatten();
        let updates = shared_updates(cx);
        let watch_updates = updates
            .as_ref()
            .map(|updates| cx.observe(updates, |_, _, cx| cx.notify()));
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
            cache_usage: None,
            launcher,
            launcher_status: None,
            updates,
            channel,
            commit_graph: Default::default(),
            writing_graph: false,
            forge_choice: None,
            picked_license: None,
            licenses_scroll: UniformListScrollHandle::new(),
            feedback: Feedback::None,
            dirty: Default::default(),
            _new_inputs: new_inputs,
            _updates: watch_updates,
        };
        this.load_launcher();
        this.load(window, cx);
        this.load_suggestions(window, cx);
        this.load_cache_size(cx);
        this
    }

    /// Shows the page at `index` of the side panel.
    pub fn show_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(page) = Page::ALL.get(index) {
            self.open_page(*page, cx);
        }
    }

    /// Shows `page`, reading again what it shows that may have changed meanwhile. The
    /// Repository page only shows while a repository is open.
    fn open_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Repository && self.repo.is_none() {
            return;
        }
        self.page = page;
        self.feedback = Feedback::None;
        match page {
            Page::App => self.load_cache_size(cx),
            Page::Repository => {
                self.load_commit_graph(cx);
                self.load_forge_choice(cx);
            }
            Page::GitConfig | Page::Identities | Page::About => {}
        }
        cx.notify();
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
            if self.page == Page::Repository {
                self.page = Page::GitConfig;
            }
        }
        self.feedback = Feedback::None;
        self.commit_graph = Default::default();
        self.writing_graph = false;
        self.forge_choice = None;
        if self.page == Page::Repository {
            self.load_commit_graph(cx);
            self.load_forge_choice(cx);
        }
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
                    Page::About => "Mantis Git, and what is in it.".to_owned(),
                    Page::Repository => format!(
                        "Showing {}, the repository in the front tab.",
                        self.repo.as_deref().map(repository_name).unwrap_or_default()
                    ),
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
        let has_repo = self.repo.is_some();
        let pages = Page::ALL
            .into_iter()
            .enumerate()
            .filter(|(_, page)| *page != Page::Repository || has_repo)
            .map(|(index, page)| {
                panel_row(("page", index), self.page == page, cx)
                    .h(px(32.))
                    .text_size(TEXT_BODY)
                    .child(page.name())
                    .on_click(cx.listener(move |this, _, _, cx| this.open_page(page, cx)))
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
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, text) = (theme.background, theme.foreground);
        let page = match self.page {
            Page::GitConfig => self.render_git_config(cx),
            Page::Identities => self.render_identities(cx),
            Page::App => self.render_app(cx),
            Page::Repository => self.render_repository(cx),
            Page::About => self.render_about(cx),
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
mod tests;
