//! The main window. This file owns the state shared across the app and the overall
//! layout; each region and screen lives in its own module.

mod commit_screen;
mod conflict_screen;
mod diff;
mod header;
mod inspector;
mod menu;
mod notice;
mod parts;
mod rows;
mod sidebar;
mod workspace;

pub use workspace::Workspace;

use gitclient_core::git::{self, Entry, LoadEvent, Refs};
use gitclient_core::selection::Selection;
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
    // KeyBinding::new has no OS to resolve a `secondary-` modifier against, and panics.
    let (refresh, sidebar) = if cfg!(target_os = "macos") {
        ("cmd-r", "cmd-b")
    } else {
        ("ctrl-r", "ctrl-b")
    };
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("k", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("j", SelectNext, context),
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

/// What fills the window. Task screens replace the graph and link back to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Graph,
    Commit,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NoRepository,
    Loading,
    Loaded,
    Failed(String),
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
    screen: Screen,
    /// The running git operation, in words. Only one runs at a time.
    busy: Option<SharedString>,
    notice: Option<Notice>,
    confirm: Option<Confirm>,
    sidebar: sidebar::State,
    inspector: inspector::State,
    diff: diff::State,
    commit: commit_screen::State,
    conflict: conflict_screen::State,
    /// Set while another tab is showing, so that only the visible tab reacts to the
    /// window coming back to the front.
    in_background: bool,
    _activation: Subscription,
}

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
            screen: Screen::Graph,
            busy: None,
            notice: None,
            confirm: None,
            sidebar: Default::default(),
            inspector: Default::default(),
            diff: Default::default(),
            commit: Default::default(),
            conflict: Default::default(),
            in_background: false,
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
        self.reload_if_refs_moved(cx);
        self.check_paused_operation(cx);
    }

    pub fn open(&mut self, repo: PathBuf, cx: &mut Context<Self>) {
        self.repo = Some(repo);
        self.entries.clear();
        self.refs = Arc::default();
        self.max_columns = 0;
        self.selected = None;
        self.selected_oid = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(cx);
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.status = Status::Loading;
        self.pending_refs = None;
        let events = git::spawn_load(repo);
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
            move || git::read_refs(&repo),
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

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn show_screen(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        cx.notify();
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
        let diff = self.render_diff(cx);
        let main = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
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
                                this.render_rows(range, cx)
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .size_full(),
                    )
                    .vertical_scrollbar(&self.scroll),
            )
            .children(diff);

        div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .children(sidebar)
            .child(main)
            .children(inspector)
            .into_any_element()
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sidebar.window_is_wide = window.viewport_size().width >= sidebar::WIDE_WINDOW;
        let theme = cx.theme();
        let (background, foreground) = (theme.background, theme.foreground);

        let (bar, content) = match self.screen {
            Screen::Graph => (
                Some(self.render_toolbar(cx).into_any_element()),
                self.render_graph(cx),
            ),
            Screen::Commit => (None, self.render_commit_screen(window, cx)),
            Screen::Conflict => (None, self.render_conflict_screen(window, cx)),
        };
        let notice = self.render_notice(cx);
        let menu = self.render_menu(cx);

        div()
            .v_flex()
            .relative()
            .size_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .bg(background)
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
            .on_action(cx.listener(|this, _: &CloseMenu, _, cx| this.close_menu(cx)))
            .on_action(cx.listener(|this, _: &ActivateMenu, _, cx| this.activate_menu(cx)))
            .children(bar)
            .children(notice)
            .child(content)
            .children(menu)
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
    use gitclient_core::branch::Dragged;
    use gitclient_core::git::{Commit, RefKind};
    use gitclient_core::graph::GraphBuilder;
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
        view.update(cx, |view, cx| {
            view.status = Status::Loading;
            view.apply(LoadEvent::Refs(Arc::default()), cx);
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
        use gitclient_core::status::NextAction;
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

    #[gpui_kit::test]
    fn spotlighting_twice_turns_it_off_and_a_reload_keeps_it(cx: &mut TestAppContext) {
        let (view, mut cx) = open_view(cx);
        load(&view, &["b", "a"], &mut cx);
        view.update(&mut cx, |view, cx| {
            view.spotlight("a".into(), cx);
            let spot = view.sidebar.spotlight.clone().unwrap();
            assert!(!spot.commit_is_lit(0) && spot.commit_is_lit(1));

            view.spotlight("a".into(), cx);
            assert!(view.sidebar.spotlight.is_none());
            view.spotlight("a".into(), cx);
        });
        load(&view, &["c", "b", "a"], &mut cx);
        view.read_with(&cx, |view, _| {
            let spot = view.sidebar.spotlight.as_ref().unwrap();
            assert!(spot.commit_is_lit(2) && !spot.commit_is_lit(0));
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
            assert_eq!(at(view).as_deref(), Some("Copy name"));
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
