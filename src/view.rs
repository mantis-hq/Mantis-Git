//! The main window. This file owns the state shared across the app and the overall
//! layout; each region and screen lives in its own module.

mod commit_screen;
mod conflict_screen;
mod diff;
mod header;
mod inspector;
mod notice;
mod rows;
mod sidebar;

use crate::git::{self, Entry, LoadEvent, Refs};
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
        OpenRepository
    ]
);

pub fn bind_keys(cx: &mut App) {
    // KeyBinding::new has no OS to resolve a `secondary-` modifier against, and panics.
    let (refresh, open) = if cfg!(target_os = "macos") {
        ("cmd-r", "cmd-o")
    } else {
        ("ctrl-r", "ctrl-o")
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
        KeyBinding::new(open, OpenRepository, context),
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
    _activation: Subscription,
}

impl GraphView {
    /// A view with no repository open. Call [`GraphView::open`] to show one.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reload_if_refs_moved(cx);
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
            _activation: activation,
        }
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
        cx.spawn(async move |this, cx| {
            let refs = cx
                .background_spawn(async move { git::read_refs(&repo) })
                .await;
            let moved = !matches!(&refs, Ok(refs) if refs.fingerprint() == current);
            if moved {
                this.update(cx, |this, cx| this.reload(cx)).ok();
            }
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

    pub fn commit_count(&self) -> usize {
        self.entries.len()
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        self.selected = Some(index);
        self.selected_oid = Some(entry.commit.oid.clone());
        self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        self.selection_changed(cx);
        cx.notify();
    }

    /// Moves the selection, clamped to the list. Starts at the top if nothing is selected.
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
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
            .child(self.render_column_header(cx))
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

        div()
            .v_flex()
            .size_full()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .bg(background)
            .text_color(foreground)
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
            .on_action(
                cx.listener(|this, _: &OpenRepository, _, cx| this.prompt_for_repository(cx)),
            )
            .children(bar)
            .children(notice)
            .child(content)
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
    use super::rows::MIN_GRAPH_WIDTH;
    use super::{Entry, GraphView, LoadEvent, Status, bind_keys};
    use crate::git::Commit;
    use crate::graph::GraphBuilder;
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
    fn a_single_lane_leaves_room_for_the_graph_heading(cx: &mut TestAppContext) {
        let (view, mut cx) = open_view(cx);
        load(&view, &["a", "b"], &mut cx);
        view.read_with(&cx, |view, _| {
            assert_eq!(view.graph_width(), MIN_GRAPH_WIDTH);
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
    fn clicking_a_row_selects_it(cx: &mut TestAppContext) {
        let (view, mut cx) = open_view(cx);
        load(&view, &["a", "b", "c"], &mut cx);
        cx.run_until_parked();

        // The second row, below the 44px toolbar and 26px header.
        cx.simulate_click(
            point(px(300.), px(44. + 26. + 28. + 14.)),
            Modifiers::none(),
        );
        assert_eq!(view.read_with(&cx, |view, _| view.selected()), Some(1));
    }
}
