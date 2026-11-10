//! The main window. This file owns the state shared across the app and the overall
//! layout; each region and screen lives in its own module.

mod conflicts;
mod dialog;
mod diff;
mod dock;
mod header;
mod inspector;
mod menu;
mod notice;
mod parts;
mod rows;
mod search;
mod settings;
mod sidebar;
mod updates;
mod workspace;

pub use settings::{
    OpenSettings, Settings, open as open_settings, open_repository_page, should_quit,
};
pub use updates::{Updates, install_updates};
pub use workspace::Workspace;

use dialog::Dialog;
use gitclient_core::cache::keys::{self, Caches};
use gitclient_core::history::columns::ColumnOrder;
use gitclient_core::history::refs::Refs;
use gitclient_core::history::selection::Selection;
use gitclient_core::history::{self, Entry, LoadEvent};
use gitclient_core::repo::commit_graph;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use notice::Notice;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PAGE_ROWS: usize = 20;

const KEY_CONTEXT: &str = "GraphView";

actions!(
    graph_view,
    [
        SelectPrevious,
        SelectNext,
        SelectPreviousPage,
        SelectNextPage,
        SelectFirst,
        SelectLast,
        Refresh,
        ToggleSidebar,
        CloseMenu,
        ActivateMenu,
        ToggleChoice,
        NextField,
        PreviousField,
        PreviousConflict,
        NextConflict,
        FindCommits
    ]
);

pub fn bind_keys(cx: &mut App) {
    workspace::bind_workspace_keys(cx);
    settings::bind_keys(cx);
    dock::bind_keys(cx);
    // KeyBinding::new has no OS to resolve a `secondary-` modifier against, and panics.
    let (refresh, sidebar, find) = if cfg!(target_os = "macos") {
        ("cmd-r", "cmd-b", "cmd-f")
    } else {
        ("ctrl-r", "ctrl-b", "ctrl-f")
    };
    let context = Some(KEY_CONTEXT);
    // Letter keys would swallow typing, so they only act while no text input has focus.
    let letters = Some("GraphView && !Input");
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("k", SelectPrevious, letters),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("j", SelectNext, letters),
        KeyBinding::new("pageup", SelectPreviousPage, context),
        KeyBinding::new("pagedown", SelectNextPage, context),
        KeyBinding::new("home", SelectFirst, context),
        KeyBinding::new("end", SelectLast, context),
        KeyBinding::new("f5", Refresh, context),
        KeyBinding::new(refresh, Refresh, context),
        KeyBinding::new(sidebar, ToggleSidebar, context),
        KeyBinding::new(find, FindCommits, context),
        KeyBinding::new("escape", CloseMenu, context),
        KeyBinding::new("enter", ActivateMenu, context),
        KeyBinding::new("space", ToggleChoice, letters),
        KeyBinding::new("tab", NextField, context),
        KeyBinding::new("shift-tab", PreviousField, context),
        KeyBinding::new("alt-up", PreviousConflict, context),
        KeyBinding::new("alt-down", NextConflict, context),
    ]);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NoRepository,
    Loading,
    Loaded,
    Failed(String),
}

/// The caches every window shares. Set once at startup with [`install_caches`]; a view
/// that finds none, as in tests, keeps its own in memory.
#[derive(Clone)]
struct AppCaches(Caches);

impl Global for AppCaches {}

/// Makes `caches` the ones all windows use.
pub fn install_caches(caches: Caches, cx: &mut App) {
    cx.set_global(AppCaches(caches));
}

fn shared_caches(cx: &App) -> Caches {
    cx.try_global::<AppCaches>()
        .map_or_else(Caches::memory_only, |shared| shared.0.clone())
}

pub struct GraphView {
    repo: Option<PathBuf>,
    status: Status,
    entries: Vec<Entry>,
    refs: Arc<Refs>,
    /// Refs of a load in progress. The old history stays on screen until the new one has
    /// something to show, so that a refresh doesn't flash an empty window.
    pending_refs: Option<Arc<Refs>>,
    max_columns: u32,
    /// Where history continues past what is loaded, while there may be more of it.
    more: Option<history::More>,
    /// Reads the next page of history, while it runs.
    more_task: Option<Task<()>>,
    selected: Option<usize>,
    /// Keeps the selection on the same commit when a reload reorders history.
    selected_oid: Option<Arc<str>>,
    /// The commits marked beside `selected`, for actions on several at once.
    selection: Selection,
    /// The context menu, while one is open.
    menu: Option<menu::Menu>,
    /// The row whose refs card is open, and whether the pointer is on the card.
    hovered_refs: Option<usize>,
    popover_hovered: bool,
    /// The graph's columns, left to right, as the user arranged them.
    columns: ColumnOrder,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    load_task: Option<Task<()>>,
    /// The running git operation, in words. Only one runs at a time.
    busy: Option<SharedString>,
    notice: Option<Notice>,
    /// The notice whose closing timer is running, so a new one starts a timer of its own.
    toast_timed: Option<Notice>,
    /// The pointer is over the toast, which keeps it open while it is read.
    toast_hovered: bool,
    /// Why counting the branches was slow, when a missing commit graph is the reason.
    graph_hint: Option<SharedString>,
    /// Repositories whose hint was dismissed, so it isn't shown again this session.
    graph_hint_dismissed: HashSet<PathBuf>,
    /// When the load under way began, to tell how long its history took to arrive.
    load_started: Option<Instant>,
    /// The popup over the window: a question before a deletion, or a form.
    dialog: Option<Dialog>,
    /// Whether the key now down is a repeat of one still held.
    key_held: bool,
    sidebar: sidebar::State,
    search: search::State,
    inspector: inspector::State,
    diff: diff::State,
    commit: inspector::commit_form::State,
    conflict: conflicts::State,
    /// Set while another tab is showing, so that only the visible tab reacts to the
    /// window coming back to the front.
    in_background: bool,
    caches: Caches,
    /// What besides a commit decides how git reports it, which keys the caches. `None`
    /// until it has been read, and the caches go unused until then.
    fingerprint: Option<Arc<str>>,
    _activation: Subscription,
}

/// Asks whoever holds this view to show another working tree of the same repository: the
/// tab already on it, or a new one.
pub struct SwitchWorktree(pub PathBuf);

impl EventEmitter<SwitchWorktree> for GraphView {}

/// Asks whoever holds this view to show a worktree in a tab of its own.
pub struct OpenWorktreeInTab(pub PathBuf);

impl EventEmitter<OpenWorktreeInTab> for GraphView {}

impl GraphView {
    /// A view with no repository open. Call [`GraphView::open`] to show one.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && !this.in_background {
                this.catch_up(cx);
            }
        });
        Self {
            repo: None,
            status: Status::NoRepository,
            entries: Vec::new(),
            refs: Arc::default(),
            pending_refs: None,
            max_columns: 0,
            more: None,
            more_task: None,
            selected: None,
            selected_oid: None,
            selection: Selection::default(),
            menu: None,
            hovered_refs: None,
            popover_hovered: false,
            columns: ColumnOrder::default(),
            scroll: UniformListScrollHandle::new(),
            focus,
            load_task: None,
            busy: None,
            notice: None,
            toast_timed: None,
            toast_hovered: false,
            graph_hint: None,
            graph_hint_dismissed: HashSet::new(),
            load_started: None,
            dialog: None,
            key_held: false,
            sidebar: sidebar::State::new(),
            search: Default::default(),
            inspector: Default::default(),
            diff: Default::default(),
            commit: Default::default(),
            conflict: Default::default(),
            in_background: false,
            caches: shared_caches(cx),
            fingerprint: None,
            _activation: activation,
        }
    }

    pub fn has_repository(&self) -> bool {
        self.repo.is_some()
    }

    pub fn repository(&self) -> Option<&Path> {
        self.repo.as_deref()
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub(super) fn set_in_background(&mut self, in_background: bool) {
        self.in_background = in_background;
    }

    /// Picks up what happened outside the app while this view was out of sight.
    pub(super) fn catch_up(&mut self, cx: &mut Context<Self>) {
        self.refresh_fingerprint(cx);
        self.reload_if_refs_moved(cx);
        if self.working_row_is_selected() {
            self.reload_working(cx);
        }
        self.check_paused_operation(cx);
    }

    pub fn open(&mut self, repo: PathBuf, cx: &mut Context<Self>) {
        self.repo = Some(repo);
        self.entries.clear();
        self.refs = Arc::default();
        self.max_columns = 0;
        self.selected = None;
        self.selected_oid = None;
        // Nothing of the old repository, or worktree, applies to the new one.
        self.selection = Selection::default();
        self.menu = None;
        self.dialog = None;
        self.conflict.open = false;
        self.sidebar.picks.clear();
        self.sidebar.spotlight = None;
        self.stop_loading_details();
        self.fingerprint = None;
        self.inspector = Default::default();
        self.diff = Default::default();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(cx);
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.status = Status::Loading;
        self.load_started = Some(Instant::now());
        self.graph_hint = None;
        self.pending_refs = None;
        self.more = None;
        self.more_task = None;
        self.refresh_fingerprint(cx);
        // Reloading keeps as much history as was scrolled into view, so the graph doesn't
        // shrink back to the first page under the pointer.
        let limit = self.commit_limit().max(self.commit_count() as u32);
        // What is on show always belongs to `refs`, even while a load is under way: a new
        // one replaces both at once. So when refs only moved forward, git lists just what
        // is new and the rest is laid out again from here.
        let previous = history::reload::Previous {
            refs: self.refs.clone(),
            commits: self
                .entries
                .iter()
                .filter(|entry| !entry.working)
                .map(|entry| entry.commit.clone())
                .collect(),
        };
        let events = history::reload::spawn_reload(repo, previous, limit);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update(cx, |this, cx| this.apply(event, cx)).is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| this.finish_load(cx)).ok();
        }));
        cx.notify();
    }

    /// Reads again what decides how git reports a commit, off the UI thread. A change to
    /// git's config makes the caches start from scratch rather than answer wrongly.
    fn refresh_fingerprint(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let asked = repo.clone();
        Self::spawn_blocking(
            move || keys::fingerprint(&repo),
            move |this, fingerprint, _| {
                if this.repo.as_deref() == Some(asked.as_path()) {
                    this.fingerprint = Some(fingerprint.into());
                }
            },
            cx,
        );
    }

    /// Picks up commits and branch changes made outside the app. Comparing refs costs a
    /// couple of git calls, so this is cheap to do on every window activation.
    fn reload_if_refs_moved(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.status != Status::Loaded {
            return;
        }
        let current = self.refs.fingerprint().to_owned();
        Self::spawn_blocking(
            move || history::refs::read_refs(&repo),
            move |this, refs, cx| {
                let moved = !matches!(&refs, Ok(refs) if refs.fingerprint() == current);
                if moved {
                    this.reload(cx);
                }
            },
            cx,
        );
    }

    /// Runs `work` off the UI thread, then gives its result to `done`. Git is slow enough
    /// that anything calling it goes through here.
    fn spawn_blocking<T: Send + 'static>(
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            this.update(cx, |this, cx| done(this, result, cx)).ok();
        })
        .detach();
    }

    /// Applies one step of a load.
    pub fn apply(&mut self, event: LoadEvent, cx: &mut Context<Self>) {
        match event {
            LoadEvent::Refs(refs) => self.pending_refs = Some(refs),
            LoadEvent::Batch(batch) => {
                // The first batch is how long the history took to start arriving, which is
                // where git sorts everything and a commit graph saves the most.
                if let Some(started) = self.load_started.take() {
                    self.explain_slowness("Loading the history", started.elapsed(), cx);
                }
                self.commit_pending_refs();
                if let Some(oid) = &self.selected_oid
                    && self.selected.is_none()
                    && let Some(index) = batch.iter().position(|e| e.commit.oid == *oid)
                {
                    self.selected = Some(self.entries.len() + index);
                }
                let width = batch.iter().map(|e| e.row.width()).max().unwrap_or(0);
                self.max_columns = self.max_columns.max(width);
                self.entries.extend(batch);
            }
            LoadEvent::More(more) => self.more = Some(more),
            LoadEvent::Failed(message) => {
                self.pending_refs = None;
                self.entries.clear();
                self.selected = None;
                self.status = Status::Failed(message);
            }
        }
        cx.notify();
    }

    /// Starts reading the next page of history once the rows in view come within
    /// `LOAD_AHEAD` of the end, so it is usually in before it is scrolled to.
    fn load_more_if_near_the_end(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        const LOAD_AHEAD: usize = 200;
        if self.more_task.is_some()
            || self.load_task.is_some()
            || visible_end + LOAD_AHEAD < self.entries.len()
        {
            return;
        }
        let (Some(repo), Some(more)) = (self.repo.clone(), self.more.take()) else {
            return;
        };
        let events = history::spawn_load_more(repo, more, self.commit_limit());
        self.more_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let applied = this.update(cx, |this, cx| match event {
                    // What is already on screen stays, and the band says why no more came.
                    LoadEvent::Failed(message) => {
                        this.notice =
                            Some(Notice::Error(history::more_failed_message(&message).into()));
                        cx.notify();
                    }
                    event => this.apply(event, cx),
                });
                if applied.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.more_task = None;
                // Picked branches may reach into the commits that just arrived.
                this.refresh_spotlight();
                cx.notify();
            })
            .ok();
        }));
    }

    /// Marks the load complete, which an empty repository reaches without any batch.
    pub fn finish_load(&mut self, cx: &mut Context<Self>) {
        self.focus_conflicts(cx);
        self.commit_pending_refs();
        if self.selected.is_none() {
            self.selected_oid = None;
        }
        if self.status == Status::Loading {
            self.status = Status::Loaded;
        }
        self.load_task = None;
        self.select_head_if_unselected(cx);
        self.refresh_spotlight();
        self.probe_forge(cx);
        self.check_paused_operation(cx);
        self.load_branch_counts(cx);
        cx.notify();
    }

    /// Counts how far each branch is from the default one, off the UI thread and after the
    /// refs are on screen, because without a commit-graph that takes seconds in a big
    /// repository. The sidebar shows the counts when they arrive.
    fn load_branch_counts(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if !self.refs.needs_counts() {
            return;
        }
        let refs = self.refs.clone();
        let asked = refs.fingerprint().to_owned();
        Self::spawn_blocking(
            move || {
                let started = Instant::now();
                let counts = history::refs::read_counts(&repo, &refs);
                (counts, started.elapsed())
            },
            move |this, (counts, took), cx| {
                // Refs that moved meanwhile are being counted again by their own load.
                if this.refs.fingerprint() == asked {
                    Arc::make_mut(&mut this.refs).set_counts(&counts);
                    this.explain_slowness("Counting the branches", took, cx);
                    cx.notify();
                }
            },
            cx,
        );
    }

    /// Says why something took `took`, when that was long and a missing commit graph is the
    /// reason, with the button that writes one. A quick step is not worth a word, and only
    /// a slow one is worth the check, which reads settings and a folder.
    fn explain_slowness(&mut self, what: &'static str, took: Duration, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if took < commit_graph::SLOW_AFTER
            || self.graph_hint.is_some()
            || self.graph_hint_dismissed.contains(&repo)
        {
            return;
        }
        let asked = repo.clone();
        Self::spawn_blocking(
            move || commit_graph::is_missing(&repo),
            move |this, missing, cx| {
                if missing && this.repo.as_deref() == Some(asked.as_path()) {
                    this.graph_hint = Some(commit_graph::slow_sentence(what, took).into());
                    cx.notify();
                }
            },
            cx,
        );
    }

    fn commit_pending_refs(&mut self) {
        if let Some(mut refs) = self.pending_refs.take() {
            // Counting against the default branch is slow in a big repository, so a reload
            // keeps the counts it had rather than showing none while it asks again.
            Arc::make_mut(&mut refs).keep_counts_from(&self.refs);
            self.refs = refs;
            self.entries.clear();
            self.max_columns = 0;
            self.selected = None;
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// What the band under the header says, if anything.
    pub fn notice_text(&self) -> Option<SharedString> {
        self.notice.as_ref().map(|notice| match notice {
            Notice::Info(message) | Notice::Error(message) => message.clone(),
        })
    }

    /// The folder git commands should run in: the top of the working tree once known.
    pub fn workdir(&self) -> Option<PathBuf> {
        let workdir = self.refs.workdir.clone();
        workdir.or_else(|| self.repo.clone())
    }

    /// How many commits are listed, leaving out the working changes row.
    pub fn commit_count(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.working).count()
    }

    /// Where a commit is in the list.
    fn row_of(&self, oid: &str) -> Option<usize> {
        self.entries.iter().position(|e| &*e.commit.oid == oid)
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        self.selected = Some(index);
        self.selected_oid = Some(entry.commit.oid.clone());
        self.selection.clear();
        self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        self.selection_changed(cx);
        cx.notify();
    }

    /// A click on a row: plain selects it, ctrl or cmd marks it beside the others, and shift
    /// marks everything from the selected row to it.
    fn click_row(&mut self, index: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let (Some(entry), Some(main)) = (self.entries.get(index), self.selected) else {
            return self.select(index, cx);
        };
        let secondary = modifiers.secondary(cx);
        if modifiers.shift {
            self.selection.extend(&self.entries, main, index);
        } else if secondary && index != main {
            let oid = entry.commit.oid.clone();
            self.selection.toggle(&oid);
        } else if secondary {
            // Unticking the main row hands the inspector to another marked one.
            let Some(next) = self.selection.promote(&self.entries) else {
                return;
            };
            let Some(next) = self.row_of(&next) else {
                return;
            };
            let marked = std::mem::take(&mut self.selection);
            self.select(next, cx);
            self.selection = marked;
        } else {
            return self.select(index, cx);
        }
        cx.notify();
    }

    /// Moves the selection, clamped to the list. Starts at the top if nothing is selected.
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if delta.abs() == 1 && self.step_menu(delta, cx) {
            return;
        }
        let Some(last) = self.entries.len().checked_sub(1) else {
            return;
        };
        let target = match self.selected {
            Some(index) => index.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.select(target, cx);
    }

    fn prompt_for_repository(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.open(path, cx)).ok();
            }
        })
        .detach();
    }

    fn render_message(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (title, detail): (&str, Option<String>) = match &self.status {
            Status::NoRepository => (
                "No repository open",
                Some("Choose a folder containing a git repository.".to_owned()),
            ),
            Status::Failed(message) => ("Could not read this repository", Some(message.clone())),
            Status::Loading => ("Loading history…", None),
            Status::Loaded => ("No commits yet", None),
        };
        let show_open = matches!(self.status, Status::NoRepository | Status::Failed(_));

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .child(div().font_semibold().child(title))
            .when_some(detail, |message, detail| {
                message.child(
                    div()
                        .max_w(px(640.))
                        .text_sm()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                )
            })
            .when(show_open, |message| {
                message.child(
                    Button::new("open-repository")
                        .primary()
                        .label("Open repository")
                        .on_click(cx.listener(|this, _, _, cx| this.prompt_for_repository(cx))),
                )
            })
            .when(show_open, |message| {
                message.child(
                    Button::new("clone-repository")
                        .ghost()
                        .label("Clone from an address")
                        .tooltip("Paste a link to a repository to download it (Ctrl Shift O)")
                        .on_click(cx.listener(|this, _, _, cx| this.open_clone_form(cx))),
                )
            })
    }
}

impl GraphView {
    fn render_graph(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.entries.is_empty() {
            return self.render_message(cx).into_any_element();
        }
        let sidebar = self.render_sidebar(cx);
        let inspector = self.render_inspector(cx);
        // One file's diff, or the two sides of a conflict, covers the graph.
        let diff = self.render_diff(cx).or_else(|| self.render_resolver(cx));
        let (has_sidebar, has_inspector) = (sidebar.is_some(), inspector.is_some());
        // An open diff takes the place of the whole graph, and the inspector beside it stays
        // for picking another file.
        let main = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(match diff {
                Some(diff) => diff,
                // The search bar sits over the graph, and what it finds takes the graph's place.
                None => div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .children(self.render_search_bar(cx))
                    .child(match self.render_search_results(cx) {
                        Some(results) => results,
                        None => div()
                            .v_flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_column_headers(cx))
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        uniform_list(
                                            "commits",
                                            self.entries.len(),
                                            cx.processor(|this, range: Range<usize>, _, cx| {
                                                this.load_more_if_near_the_end(range.end, cx);
                                                this.render_rows(range, cx)
                                            }),
                                        )
                                        .track_scroll(&self.scroll)
                                        .size_full(),
                                    )
                                    .vertical_scrollbar(&self.scroll),
                            )
                            .into_any_element(),
                    })
                    .into_any_element(),
            });
        // The graph keeps at least this much, so no edge can be dragged over it.
        const GRAPH_MIN: f32 = 320.;

        let row = div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .children(sidebar)
            .when(has_sidebar, |row| {
                row.child(parts::grip(parts::Edge::Sidebar, true, cx))
            })
            .child(main)
            .when(has_inspector, |row| {
                row.child(parts::grip(parts::Edge::Inspector, true, cx))
            })
            .children(inspector);
        let row = parts::on_resize(row, parts::Edge::Sidebar, cx, |this, pointer, bounds| {
            let other = this.inspector.width.unwrap_or(380.);
            let width = pointer.x - bounds.left();
            this.sidebar.width = Some(parts::fit(
                width,
                200.,
                f32::from(bounds.size.width) - other - GRAPH_MIN,
            ));
        });
        parts::on_resize(row, parts::Edge::Inspector, cx, |this, pointer, bounds| {
            let other = this.sidebar.width.unwrap_or(300.);
            let width = bounds.right() - pointer.x;
            this.inspector.width = Some(parts::fit(
                width,
                240.,
                f32::from(bounds.size.width) - other - GRAPH_MIN,
            ));
        })
        .into_any_element()
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sidebar.window_is_wide = window.viewport_size().width >= sidebar::WIDE_WINDOW;
        self.ensure_commit_inputs(window, cx);
        self.refocus_after_search(window, cx);
        self.clear_form_after_commit(window, cx);
        let theme = cx.theme();
        let (background, foreground) = (theme.background, theme.foreground);

        self.sync_conflict_editor(window, cx);
        self.sync_result_scroll(cx);
        let bar = Some(self.render_toolbar(cx).into_any_element());
        let content = self.render_graph(cx);
        self.sync_toast(cx);
        let cards = self.render_cards(cx);
        let menu = self.render_menu(cx);
        let dialog = self.render_dialog(window, cx);

        div()
            .v_flex()
            .relative()
            .size_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
                this.key_held = event.is_held;
            }))
            .bg(background)
            .text_size(parts::TEXT_BODY)
            .text_color(foreground)
            // Maple Mono would otherwise draw `->` as an arrow, which isn't what code says.
            .font_features(FontFeatures::disable_ligatures())
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousPage, _, cx| {
                this.move_selection(-(PAGE_ROWS as isize), cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNextPage, _, cx| {
                this.move_selection(PAGE_ROWS as isize, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| {
                this.select(this.entries.len().saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(
                cx.listener(|this, _: &FindCommits, window, cx| this.open_search(window, cx)),
            )
            .on_action(cx.listener(|this, _: &settings::OpenSettings, _, cx| {
                settings::open(this.repo.clone(), cx)
            }))
            .on_action(cx.listener(|this, _: &CloseMenu, _, cx| this.press_escape(cx)))
            .on_action(cx.listener(|this, _: &ActivateMenu, _, cx| this.press_enter(cx)))
            .on_action(cx.listener(|this, _: &ToggleChoice, _, cx| {
                // Only the welcome panel has a box to tick. Elsewhere Space goes on to
                // whatever has focus, such as a button.
                if !this.toggle_welcome_launcher(cx) {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextField, window, cx| this.step_field(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousField, window, cx| this.step_field(-1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousConflict, _, cx| this.go_to_conflict(false, cx)),
            )
            .on_action(cx.listener(|this, _: &NextConflict, _, cx| this.go_to_conflict(true, cx)))
            .children(bar)
            .child(content)
            .children(menu)
            .children(cards)
            .children(dialog)
    }
}

fn repository_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests;
