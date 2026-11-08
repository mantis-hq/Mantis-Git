//! The main window. This file owns the state shared across the app and the overall
//! layout; each region and screen lives in its own module.

mod commit_form;
mod conflicts;
mod diff;
mod dock;
mod header;
mod inspector;
mod menu;
mod notice;
mod parts;
mod rows;
mod settings;
mod sidebar;
mod terminal;
mod workspace;

pub use settings::{OpenSettings, Settings, open as open_settings, should_quit};
pub use workspace::Workspace;

use gitclient_core::cache::keys::{self, Caches};
use gitclient_core::history::refs::Refs;
use gitclient_core::history::selection::Selection;
use gitclient_core::history::{self, Entry, LoadEvent};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use notice::{Confirm, Notice};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        ActivateMenu
    ]
);

pub fn bind_keys(cx: &mut App) {
    workspace::bind_workspace_keys(cx);
    settings::bind_keys(cx);
    dock::bind_keys(cx);
    // KeyBinding::new has no OS to resolve a `secondary-` modifier against, and panics.
    let (refresh, sidebar) = if cfg!(target_os = "macos") {
        ("cmd-r", "cmd-b")
    } else {
        ("ctrl-r", "ctrl-b")
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
        KeyBinding::new("escape", CloseMenu, context),
        KeyBinding::new("enter", ActivateMenu, context),
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
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    load_task: Option<Task<()>>,
    /// The running git operation, in words. Only one runs at a time.
    busy: Option<SharedString>,
    notice: Option<Notice>,
    confirm: Option<Confirm>,
    /// Whether the key now down is a repeat of one still held.
    key_held: bool,
    /// The form that adds or edits a remote, while it is open.
    remote_form: Option<sidebar::remotes::RemoteForm>,
    sidebar: sidebar::State,
    inspector: inspector::State,
    diff: diff::State,
    commit: commit_form::State,
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
            selected: None,
            selected_oid: None,
            selection: Selection::default(),
            menu: None,
            hovered_refs: None,
            popover_hovered: false,
            scroll: UniformListScrollHandle::new(),
            focus,
            load_task: None,
            busy: None,
            notice: None,
            confirm: None,
            key_held: false,
            remote_form: None,
            sidebar: sidebar::State::new(),
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
        self.remote_form = None;
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
        self.pending_refs = None;
        self.refresh_fingerprint(cx);
        let limit = self.commit_limit();
        let events = history::spawn_load(repo, limit);
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
            LoadEvent::Failed(message) => {
                self.pending_refs = None;
                self.entries.clear();
                self.selected = None;
                self.status = Status::Failed(message);
            }
        }
        cx.notify();
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
        self.check_paused_operation(cx);
        cx.notify();
    }

    fn commit_pending_refs(&mut self) {
        if let Some(refs) = self.pending_refs.take() {
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

    /// Whether a risky step is waiting for confirmation.
    pub fn is_confirming(&self) -> bool {
        self.confirm.is_some()
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
                None => div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "commits",
                            self.entries.len(),
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                this.render_rows(range, cx)
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .size_full(),
                    )
                    .vertical_scrollbar(&self.scroll)
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
        self.ensure_new_branch_input(window, cx);
        self.ensure_commit_inputs(window, cx);
        self.clear_form_after_commit(window, cx);
        let theme = cx.theme();
        let (background, foreground) = (theme.background, theme.foreground);

        self.sync_conflict_editor(window, cx);
        let bar = Some(self.render_toolbar(cx).into_any_element());
        let content = self.render_graph(cx);
        self.ensure_remote_inputs(window, cx);
        let notice = self.render_notice(cx);
        let remote_form = self.render_remote_form(cx);
        let menu = self.render_menu(cx);
        let popup = self.render_confirm_popup(cx);
        // Enter and Escape are bound on this view, and a click elsewhere (a menu line, a
        // terminal) leaves the focus somewhere they don't reach.
        if popup.is_some() && !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }

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
            .on_action(cx.listener(|this, _: &settings::OpenSettings, _, cx| {
                settings::open(this.repo.clone(), cx)
            }))
            .on_action(cx.listener(|this, _: &CloseMenu, _, cx| this.press_escape(cx)))
            .on_action(cx.listener(|this, _: &ActivateMenu, _, cx| this.press_enter(cx)))
            .children(bar)
            .children(notice)
            .children(remote_form)
            .child(content)
            .children(menu)
            .children(popup)
    }
}

fn repository_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests;
