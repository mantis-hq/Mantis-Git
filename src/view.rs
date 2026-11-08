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
mod remotes;
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
    remote_form: Option<remotes::RemoteForm>,
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
mod tests {
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
            let band = view.confirm.as_ref().unwrap().message.clone();
            assert!(band.contains("replaces 2 commits"), "{band}");
            assert!(band.contains("git reflog"), "says how to undo it: {band}");

            view.confirm = None;
            view.start_action(NextAction::Push { commits: 1 }, cx);
            assert!(!view.is_confirming());
        });
    }

    #[gpui_kit::test]
    fn a_deletion_popup_takes_enter_and_escape_and_the_risky_one_needs_two_presses(
        cx: &mut TestAppContext,
    ) {
        use super::notice::Confirm;
        use gitclient_core::menu::command::Weight;
        use std::cell::Cell;
        use std::rc::Rc;
        let (view, mut cx) = open_view(cx);
        let ran = Rc::new(Cell::new(0));
        let ask = |view: &mut GraphView, weight| {
            let ran = ran.clone();
            view.confirm = Some(Confirm {
                message: "This deletes it.".into(),
                action: "Delete".into(),
                run: Rc::new(move |_, _| ran.set(ran.get() + 1)),
                weight,
                armed: None,
            });
        };
        view.update(&mut cx, |view, cx| {
            ask(view, Weight::Popup);
            view.press_escape(cx);
            assert!(!view.is_confirming());
            assert_eq!(ran.get(), 0, "Escape cancels");

            ask(view, Weight::Popup);
            view.key_held = true;
            view.press_enter(cx);
            assert!(view.is_confirming(), "a held key doesn't answer");
            view.key_held = false;
            view.press_enter(cx);
            assert!(!view.is_confirming());
            assert_eq!(ran.get(), 1, "Enter confirms");

            ask(view, Weight::DoublePress);
            view.press_enter(cx);
            assert!(view.is_confirming());
            assert_eq!(ran.get(), 1, "one press only arms it");
            view.press_enter(cx);
            assert!(!view.is_confirming());
            assert_eq!(ran.get(), 2, "the second press confirms");

            ask(view, Weight::DoublePress);
            view.press_enter(cx);
            view.press_escape(cx);
            assert!(!view.is_confirming());
            assert_eq!(ran.get(), 2);
        });
    }

    #[gpui_kit::test]
    fn the_keys_reach_a_popup_even_when_focus_was_elsewhere(cx: &mut TestAppContext) {
        use super::notice::Confirm;
        use gitclient_core::menu::command::Weight;
        use std::cell::Cell;
        use std::rc::Rc;
        let (view, mut cx) = open_view(cx);
        let ran = Rc::new(Cell::new(0));
        let ask = |cx: &mut VisualTestContext| {
            let ran = ran.clone();
            view.update(cx, |view, cx| {
                view.confirm = Some(Confirm {
                    message: "This deletes it.".into(),
                    action: "Delete".into(),
                    run: Rc::new(move |_, _| ran.set(ran.get() + 1)),
                    weight: Weight::Popup,
                    armed: None,
                });
                cx.notify();
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

        // The second row, below the 52px header.
        cx.simulate_click(point(px(300.), px(52. + 32. + 16.)), Modifiers::none());
        assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(1));
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

    #[gpui_kit::test]
    fn a_result_is_built_from_picked_lines_and_saved_with_one_button(cx: &mut TestAppContext) {
        use super::conflicts::Pick;
        use gitclient_core::repo::conflict::Side;
        use gitclient_core::testing::Repo;
        let repo = Repo::new();
        repo.git(&["config", "user.name", "Ada"]);
        repo.git(&["config", "user.email", "ada@example.com"]);
        repo.write("a", "top\nbase\nbottom\n");
        repo.commit("base");
        let main = repo.git(&["branch", "--show-current"]);
        repo.git(&["checkout", "-q", "-b", "feat"]);
        repo.write("a", "top\nfeat one\nfeat two\nbottom\n");
        repo.commit("feat change");
        repo.git(&["checkout", "-q", &main]);
        repo.write("a", "top\nmain\nbottom\n");
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
        // Drawing reads the file's result into the editor.
        cx.simulate_click(point(px(2.), px(2.)), Modifiers::none());

        // Saving waits for the conflict to be settled in the result.
        view.update(&mut cx, |view, cx| view.save_conflict(cx));
        cx.run_until_parked();
        assert!(view.read_with(&cx, |view, _| view.resolver_is_open()));
        assert!(repo.read("a").contains("<<<<<<<"), "nothing was saved");

        // The second line of the other side, and the first of the first side's.
        view.update(&mut cx, |view, cx| {
            view.toggle_line((0, Side::Other, 1), cx);
            view.toggle_line((0, Side::Mine, 0), cx);
            view.use_chunk(0, Pick::Lines, cx);
        });
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
            assert_eq!(labels(view)[0], "Check out");

            view.close_menu(cx);
            assert!(view.menu.is_none());
        });
    }

    #[gpui_kit::test]
    fn editing_a_remote_starts_from_its_address_and_escape_closes_the_menu_before_the_form(
        cx: &mut TestAppContext,
    ) {
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

            view.open_remote_form(Some("origin".into()), cx);
            let form = view.remote_form.as_ref().unwrap();
            assert_eq!(form.prefill(), ("origin", "git@host:a.git"));

            view.open_remote_menu("origin", point(px(10.), px(10.)), cx);
            view.dismiss(cx);
            assert!(view.menu.is_none() && view.remote_form.is_some());
            view.dismiss(cx);
            assert!(view.remote_form.is_none());
        });
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
            assert_eq!(at(view).as_deref(), Some("Check out"));
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
}
