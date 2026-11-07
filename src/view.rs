//! The commit graph window: a virtualized list of commits with their lanes drawn beside
//! them.

use crate::git::{self, Entry, LoadEvent, Ref, RefKind, Refs};
use crate::graph::{Lane, Row};
use chrono::{DateTime, Local};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ROW_HEIGHT: Pixels = px(28.);
const LANE_WIDTH: f32 = 14.;
const GRAPH_PADDING: f32 = 6.;
const LINE_WIDTH: f32 = 1.5;
const NODE_RADIUS: f32 = 4.;
/// Wider graphs are clipped, so that a few hundred stale branches can't push the text out
/// of the window.
const MAX_VISIBLE_LANES: u32 = 24;
const MAX_VISIBLE_REFS: usize = 3;
const PAGE_ROWS: usize = 20;

const AUTHOR_WIDTH: Pixels = px(150.);
const DATE_WIDTH: Pixels = px(136.);
const OID_WIDTH: Pixels = px(72.);

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

    fn graph_width(&self) -> f32 {
        let lanes = self.max_columns.clamp(1, MAX_VISIBLE_LANES);
        lanes as f32 * LANE_WIDTH + GRAPH_PADDING * 2.
    }

    fn render_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (foreground, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let (background, hover, active) = (theme.background, theme.list_hover, theme.list_active);
        let (warning, mono) = (theme.warning, theme.mono_font_family.clone());
        let graph_width = self.graph_width();

        let mut rows = Vec::with_capacity(range.len());
        for index in range {
            let entry = &self.entries[index];
            let commit = &entry.commit;
            let selected = self.selected == Some(index);
            let row_background = if selected { active } else { background };
            let lane_row = entry.row.clone();
            let is_head = self.refs.head.as_ref() == Some(&commit.oid);
            let is_merge = commit.parents.len() > 1;
            let commit_refs = self.refs.by_commit.get(&commit.oid);
            let lane_color = palette(entry.row.color, is_dark);

            rows.push(
                div()
                    .id(index)
                    .h_flex()
                    .h(ROW_HEIGHT)
                    .w_full()
                    .pr_3()
                    .gap_3()
                    .text_sm()
                    .bg(row_background)
                    .when(!selected, |row| row.hover(|style| style.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(index, cx);
                        window.focus(&this.focus, cx);
                    }))
                    .child(
                        div()
                            .w(px(graph_width))
                            .h_full()
                            .flex_none()
                            .overflow_hidden()
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        paint_row(
                                            bounds,
                                            &lane_row,
                                            NodeStyle {
                                                is_head,
                                                is_merge,
                                                hollow: row_background,
                                            },
                                            is_dark,
                                            window,
                                        );
                                    },
                                )
                                .size_full(),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1p5()
                            .overflow_hidden()
                            .when_some(commit_refs, |row, refs| {
                                row.children(
                                    refs.iter().take(MAX_VISIBLE_REFS).map(|r| {
                                        ref_label(r, lane_color, warning, foreground, muted)
                                    }),
                                )
                                .when(
                                    refs.len() > MAX_VISIBLE_REFS,
                                    |row| {
                                        row.child(
                                            div().flex_none().text_xs().text_color(muted).child(
                                                format!("+{}", refs.len() - MAX_VISIBLE_REFS),
                                            ),
                                        )
                                    },
                                )
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(foreground)
                                    .child(SharedString::from(commit.subject.clone())),
                            ),
                    )
                    .child(
                        div()
                            .w(AUTHOR_WIDTH)
                            .flex_none()
                            .truncate()
                            .text_color(muted)
                            .child(SharedString::from(commit.author.clone())),
                    )
                    .child(
                        div()
                            .w(DATE_WIDTH)
                            .flex_none()
                            .truncate()
                            .text_color(muted)
                            .child(format_date(commit.time)),
                    )
                    .child(
                        div()
                            .w(OID_WIDTH)
                            .flex_none()
                            .font_family(mono.clone())
                            .text_xs()
                            .text_color(muted)
                            .child(commit.short_oid().to_owned()),
                    )
                    .border_b_1()
                    .border_color(border.opacity(0.25)),
            );
        }
        rows
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let name = self.repo.as_deref().map(repository_name);
        let path = self.repo.as_ref().map(|path| path.display().to_string());
        let loading = self.status == Status::Loading;

        div()
            .h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .when_some(name, |bar, name| {
                bar.child(div().font_semibold().child(name))
            })
            .when_some(path, |bar, path| {
                bar.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted)
                        .child(path),
                )
            })
            .child(div().flex_1())
            .when(loading, |bar| bar.child(Spinner::new().small()))
            .when(!self.entries.is_empty(), |bar| {
                bar.child(
                    div()
                        .flex_none()
                        .text_sm()
                        .text_color(muted)
                        .child(format!("{} commits", self.entries.len())),
                )
            })
            .when(self.repo.is_some(), |bar| {
                bar.child(
                    Button::new("refresh")
                        .ghost()
                        .small()
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
            })
            .child(
                Button::new("open")
                    .ghost()
                    .small()
                    .label("Open…")
                    .on_click(cx.listener(|this, _, _, cx| this.prompt_for_repository(cx))),
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .h_flex()
            .h(px(26.))
            .flex_none()
            .pr_3()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .bg(theme.list_head)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(self.graph_width()))
                    .flex_none()
                    .pl_2()
                    .child("Graph"),
            )
            .child(div().flex_1().min_w_0().child("Description"))
            .child(div().w(AUTHOR_WIDTH).flex_none().child("Author"))
            .child(div().w(DATE_WIDTH).flex_none().child("Date"))
            .child(div().w(OID_WIDTH).flex_none().child("Commit"))
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

impl Render for GraphView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground) = (theme.background, theme.foreground);

        let content = if self.entries.is_empty() {
            self.render_message(cx).into_any_element()
        } else {
            div()
                .v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_header(cx))
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
                .into_any_element()
        };

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
            .child(self.render_toolbar(cx))
            .child(content)
    }
}

fn repository_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn format_date(time: i64) -> String {
    DateTime::from_timestamp(time, 0)
        .map(|date| {
            date.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn ref_label(r: &Ref, lane: Hsla, tag: Hsla, foreground: Hsla, muted: Hsla) -> impl IntoElement {
    let (color, text) = match r.kind {
        RefKind::Tag => (tag, foreground),
        RefKind::Remote => (muted, muted),
        RefKind::Branch | RefKind::DetachedHead => (lane, foreground),
    };
    div()
        .flex_none()
        .max_w(px(180.))
        .truncate()
        .px_1p5()
        .rounded_sm()
        .text_xs()
        .text_color(text)
        .border_1()
        .border_color(color.opacity(0.7))
        .bg(color.opacity(if r.is_head { 0.4 } else { 0.12 }))
        .when(r.is_head, |label| label.font_semibold())
        .child(r.name.clone())
}

fn palette(color: u32, is_dark: bool) -> Hsla {
    const HUES: [f32; 8] = [212., 150., 34., 330., 265., 178., 8., 92.];
    let hue = HUES[color as usize % HUES.len()] / 360.;
    hsla(hue, 0.75, if is_dark { 0.65 } else { 0.42 }, 1.)
}

struct NodeStyle {
    is_head: bool,
    is_merge: bool,
    /// Fills the middle of merge nodes, so it has to match the row behind them.
    hollow: Hsla,
}

fn paint_row(
    bounds: Bounds<Pixels>,
    row: &Row,
    node: NodeStyle,
    is_dark: bool,
    window: &mut Window,
) {
    let x_of = |column: u32| bounds.left() + px(GRAPH_PADDING + (column as f32 + 0.5) * LANE_WIDTH);
    let (top, bottom) = (bounds.top(), bounds.bottom());
    let middle = top + bounds.size.height / 2.;
    let node_x = x_of(row.column);

    for lane in row.through.iter() {
        paint_vertical(x_of(lane.column), top, bottom, lane, is_dark, window);
    }
    for lane in &row.incoming {
        paint_edge(
            point(x_of(lane.column), top),
            point(node_x, middle),
            lane,
            is_dark,
            window,
        );
    }
    for lane in &row.outgoing {
        paint_edge(
            point(node_x, middle),
            point(x_of(lane.column), bottom),
            lane,
            is_dark,
            window,
        );
    }

    let color = palette(row.color, is_dark);
    if node.is_head {
        paint_circle(
            node_x,
            middle,
            NODE_RADIUS + 2.5,
            color.opacity(0.35),
            window,
        );
    }
    paint_circle(node_x, middle, NODE_RADIUS, color, window);
    if node.is_merge {
        paint_circle(node_x, middle, NODE_RADIUS - 1.75, node.hollow, window);
    }
}

fn paint_vertical(
    x: Pixels,
    top: Pixels,
    bottom: Pixels,
    lane: &Lane,
    is_dark: bool,
    window: &mut Window,
) {
    let half = px(LINE_WIDTH / 2.);
    let bounds = Bounds::from_corners(point(x - half, top), point(x + half, bottom));
    window.paint_quad(fill(bounds, palette(lane.color, is_dark)));
}

fn paint_edge(
    from: Point<Pixels>,
    to: Point<Pixels>,
    lane: &Lane,
    is_dark: bool,
    window: &mut Window,
) {
    if from.x == to.x {
        paint_vertical(from.x, from.y, to.y, lane, is_dark, window);
        return;
    }
    let bend = (to.y - from.y) * 0.6;
    let mut path = PathBuilder::stroke(px(LINE_WIDTH));
    path.move_to(from);
    path.cubic_bezier_to(to, point(from.x, from.y + bend), point(to.x, to.y - bend));
    if let Ok(path) = path.build() {
        window.paint_path(path, palette(lane.color, is_dark));
    }
}

fn paint_circle(x: Pixels, y: Pixels, radius: f32, color: Hsla, window: &mut Window) {
    let radius = px(radius);
    let bounds = Bounds::from_corners(point(x - radius, y - radius), point(x + radius, y + radius));
    window.paint_quad(fill(bounds, color).corner_radii(radius));
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
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
