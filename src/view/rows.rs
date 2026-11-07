//! The commit rows: lanes on a canvas, then the subject, refs, author, date and hash.

use super::GraphView;
use crate::git::{Ref, RefKind};
use crate::graph::{Lane, Row};
use crate::theme::{lane_color, selected_row};
use chrono::{DateTime, Local};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;

const ROW_HEIGHT: Pixels = px(32.);
const LANE_WIDTH: f32 = 16.;
const GRAPH_PADDING: f32 = 6.;
/// Keeps the "Graph" heading on one line when a history has few lanes.
pub(super) const MIN_GRAPH_WIDTH: f32 = 56.;
const LINE_WIDTH: f32 = 1.75;
const NODE_RADIUS: f32 = 4.;
/// Wider graphs are clipped, so that a few hundred stale branches can't push the text out
/// of the window.
const MAX_VISIBLE_LANES: u32 = 24;
const MAX_VISIBLE_REFS: usize = 3;

const AUTHOR_WIDTH: Pixels = px(150.);
const DATE_WIDTH: Pixels = px(136.);
const OID_WIDTH: Pixels = px(72.);

impl GraphView {
    pub(super) fn graph_width(&self) -> f32 {
        let lanes = self.max_columns.clamp(1, MAX_VISIBLE_LANES);
        (lanes as f32 * LANE_WIDTH + GRAPH_PADDING * 2.).max(MIN_GRAPH_WIDTH)
    }

    pub(super) fn render_rows(
        &self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (foreground, muted) = (theme.foreground, theme.muted_foreground);
        let (background, hover, active) = (theme.background, theme.list_hover, selected_row(theme));
        let (head, on_head) = (theme.primary, theme.primary_foreground);
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
            let is_working = entry.working;
            let commit_refs = self.refs.by_commit.get(&commit.oid);

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
                        if this.entries.get(index).is_some_and(|entry| entry.working) {
                            this.open_commit_screen(cx);
                        } else {
                            this.select(index, cx);
                        }
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
                                                is_working,
                                                head,
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
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(foreground)
                                    .child(SharedString::from(commit.subject.clone())),
                            )
                            .when_some(commit_refs, |row, refs| {
                                row.children(refs.iter().take(MAX_VISIBLE_REFS).map(|r| {
                                    let lane = lane_color(r.color, is_dark);
                                    ref_label(r, lane, head, on_head, warning)
                                }))
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
                            }),
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
                            .when(!is_working, |date| date.child(format_date(commit.time))),
                    )
                    .child(
                        div()
                            .w(OID_WIDTH)
                            .flex_none()
                            .font_family(mono.clone())
                            .text_size(px(13.))
                            .text_color(muted)
                            .child(commit.short_oid().to_owned()),
                    ),
            );
        }
        rows
    }

    pub(super) fn render_column_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .h_flex()
            .h(px(26.))
            .flex_none()
            .pr_3()
            .gap_3()
            .text_size(px(13.))
            .font_medium()
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

fn ref_label(r: &Ref, lane: Hsla, head: Hsla, on_head: Hsla, tag: Hsla) -> impl IntoElement {
    let color = match r.kind {
        RefKind::Tag => tag,
        RefKind::Branch | RefKind::Remote | RefKind::DetachedHead => lane,
    };
    let (fill, text) = if r.is_head {
        (head, on_head)
    } else {
        (color.opacity(0.14), color)
    };
    div()
        .flex_none()
        .max_w(px(180.))
        .truncate()
        .px_1p5()
        .rounded(px(4.))
        .text_xs()
        .font_semibold()
        .text_color(text)
        .bg(fill)
        .child(r.name.clone())
}

struct NodeStyle {
    is_head: bool,
    is_merge: bool,
    is_working: bool,
    head: Hsla,
    /// Fills the middle of merge nodes and the HEAD ring, so it has to match the row
    /// behind them.
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

    if node.is_working {
        paint_dashed_ring(node_x, middle, NODE_RADIUS + 1., node.head, window);
        return;
    }
    if node.is_head {
        paint_circle(node_x, middle, NODE_RADIUS + 3., node.head, window);
        paint_circle(node_x, middle, NODE_RADIUS + 1.5, node.hollow, window);
        paint_circle(node_x, middle, NODE_RADIUS, node.head, window);
        return;
    }
    paint_circle(
        node_x,
        middle,
        NODE_RADIUS,
        lane_color(row.color, is_dark),
        window,
    );
    if node.is_merge {
        paint_circle(node_x, middle, NODE_RADIUS - 1.75, node.hollow, window);
    }
}

/// A ring of short arcs, for work that isn't a commit yet.
fn paint_dashed_ring(x: Pixels, y: Pixels, radius: f32, color: Hsla, window: &mut Window) {
    const DASHES: u32 = 6;
    for dash in 0..DASHES {
        let start = dash as f32 / DASHES as f32 * std::f32::consts::TAU;
        let end = start + std::f32::consts::TAU / DASHES as f32 * 0.55;
        let at = |angle: f32| point(x + px(angle.cos() * radius), y + px(angle.sin() * radius));
        let mut path = PathBuilder::stroke(px(1.5));
        path.move_to(at(start));
        path.line_to(at((start + end) / 2.));
        path.line_to(at(end));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
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
    if lane.dashed {
        let mut path = PathBuilder::stroke(px(LINE_WIDTH)).dash_array(&[px(4.), px(4.)]);
        path.move_to(point(x, top));
        path.line_to(point(x, bottom));
        if let Ok(path) = path.build() {
            window.paint_path(path, lane_color(lane.color, is_dark));
        }
        return;
    }
    let half = px(LINE_WIDTH / 2.);
    let bounds = Bounds::from_corners(point(x - half, top), point(x + half, bottom));
    window.paint_quad(fill(bounds, lane_color(lane.color, is_dark)));
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
        window.paint_path(path, lane_color(lane.color, is_dark));
    }
}

fn paint_circle(x: Pixels, y: Pixels, radius: f32, color: Hsla, window: &mut Window) {
    let radius = px(radius);
    let bounds = Bounds::from_corners(point(x - radius, y - radius), point(x + radius, y + radius));
    window.paint_quad(fill(bounds, color).corner_radii(radius));
}
