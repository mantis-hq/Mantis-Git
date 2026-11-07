//! The commit rows: lanes on a canvas, then the subject, refs, author and time.

use super::GraphView;
use crate::git::{Commit, Ref, RefKind, split_subject};
use crate::graph::{Lane, Row};
use crate::spotlight::Spotlight;
use crate::theme::{lane_color, selected_row, tokens};
use chrono::{Datelike, Local, TimeZone, Utc};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;

const ROW_HEIGHT: Pixels = px(32.);
const LANE_WIDTH: f32 = 16.;
const GRAPH_PADDING: f32 = 6.;
const LINE_WIDTH: f32 = 1.75;
const NODE_RADIUS: f32 = 4.;
/// Wider graphs are clipped, so that a few hundred stale branches can't push the text out
/// of the window. Rows with a commit past the edge show a marker there instead.
const MAX_VISIBLE_LANES: u32 = 8;
const CLIPPED_MARKER_RADIUS: f32 = 3.;
const MAX_VISIBLE_REFS: usize = 3;
/// How far things outside a spotlighted branch fade.
const FADED_ROW: f32 = 0.28;
const FADED_LINE: f32 = 0.16;

const AUTHOR_WIDTH: Pixels = px(120.);
const DATE_WIDTH: Pixels = px(72.);

impl GraphView {
    pub(super) fn graph_width(&self) -> f32 {
        let lanes = self.max_columns.clamp(1, MAX_VISIBLE_LANES);
        lanes as f32 * LANE_WIDTH + GRAPH_PADDING * 2.
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
        let warning = theme.warning;
        let subtext0 = tokens(is_dark).subtext0;
        let graph_width = self.graph_width();

        let mut rows = Vec::with_capacity(range.len());
        for index in range {
            let entry = &self.entries[index];
            let commit = &entry.commit;
            let selected = self.selected == Some(index);
            let row_background = if selected { active } else { background };
            let lane_row = entry.row.clone();
            let spotlight = self.sidebar.spotlight.clone();
            let commit_lit = spotlight.as_ref().is_none_or(|s| s.commit_is_lit(index));
            let is_head = self.refs.head.as_ref() == Some(&commit.oid);
            let is_merge = commit.parents.len() > 1;
            let is_working = entry.working;
            let chips = commit_chips(self.refs.by_commit.get(&commit.oid));
            let (prefix, description) = split_subject(&commit.subject);
            let subject_color = if is_merge { subtext0 } else { foreground };

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
                    .when(!commit_lit, |row| row.opacity(FADED_ROW))
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
                                            spotlight.as_deref().map(|spot| (spot, index)),
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
                                    .h_flex()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(subject_color)
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(muted)
                                            .child(prefix.to_owned()),
                                    )
                                    .child(
                                        div().min_w_0().truncate().child(description.to_owned()),
                                    ),
                            )
                            .children(chips.iter().take(MAX_VISIBLE_REFS).map(|chip| {
                                let lane = lane_color(chip.label.color, is_dark);
                                ref_label(chip, lane, head, on_head, warning)
                            }))
                            .when(chips.len() > MAX_VISIBLE_REFS, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("+{}", chips.len() - MAX_VISIBLE_REFS)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(AUTHOR_WIDTH)
                            .flex_none()
                            .truncate()
                            .text_right()
                            .text_color(muted)
                            .child(SharedString::from(commit.author.clone())),
                    )
                    .child(
                        div()
                            .w(DATE_WIDTH)
                            .flex_none()
                            .text_right()
                            .text_color(muted)
                            .when(!is_working, |date| {
                                date.child(format_when(commit, Utc::now().timestamp()))
                            }),
                    ),
            );
        }
        rows
    }
}

/// When a commit happened, as short as it can be said.
fn format_when(commit: &Commit, now: i64) -> String {
    let Some(date) = Local.timestamp_opt(commit.time, 0).single() else {
        return String::new();
    };
    let Some(today) = Local.timestamp_opt(now, 0).single() else {
        return String::new();
    };
    let age = now - commit.time;
    match age {
        ..60 => "now".to_owned(),
        60..3600 => format!("{}m", age / 60),
        3600..86_400 => format!("{}h", age / 3600),
        86_400..604_800 => date.format("%a").to_string(),
        _ if date.year() == today.year() => date.format("%b %-d").to_string(),
        _ => date.format("%Y").to_string(),
    }
}

/// A ref as drawn: a branch and its remote at the same commit are one chip.
struct Chip<'a> {
    label: &'a Ref,
    synced: bool,
}

fn commit_chips(refs: Option<&Vec<Ref>>) -> Vec<Chip<'_>> {
    let refs = refs.map_or(&[][..], Vec::as_slice);
    let local = |remote: &str| {
        let branch = remote.split_once('/').map_or(remote, |(_, branch)| branch);
        refs.iter()
            .any(|r| r.kind == RefKind::Branch && r.name == branch)
    };
    let tracked = |branch: &Ref| {
        refs.iter().any(|r| {
            r.kind == RefKind::Remote
                && r.name.split_once('/').map(|(_, name)| name) == Some(branch.name.as_str())
        })
    };
    refs.iter()
        .filter(|r| r.kind != RefKind::Remote || !local(&r.name))
        .map(|label| Chip {
            label,
            synced: label.kind == RefKind::Branch && tracked(label),
        })
        .collect()
}

fn ref_label(chip: &Chip, lane: Hsla, head: Hsla, on_head: Hsla, tag: Hsla) -> impl IntoElement {
    let r = chip.label;
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
        .h_flex()
        .flex_none()
        .gap_1()
        .h(px(20.))
        .max_w(px(200.))
        .px_1p5()
        .rounded(px(4.))
        .text_xs()
        .font_semibold()
        .text_color(text)
        .bg(fill)
        .child(div().min_w_0().truncate().child(r.name.clone()))
        .when(chip.synced, |label| {
            label.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| paint_cloud(bounds, text, window),
                )
                .flex_none()
                .w(px(12.))
                .h(px(10.)),
            )
        })
}

/// Says "in sync with the remote" by shape, so it doesn't rest on colour alone.
fn paint_cloud(bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    let at = |x: f32, y: f32| point(bounds.left() + px(x), bounds.top() + px(y));
    let mut path = PathBuilder::stroke(px(1.3));
    path.move_to(at(3.2, 8.5));
    path.cubic_bezier_to(at(3.2, 4.1), at(0.2, 8.5), at(0.2, 4.1));
    path.cubic_bezier_to(at(8.9, 3.5), at(3.8, 0.6), at(7.8, 0.6));
    path.cubic_bezier_to(at(8.9, 8.5), at(12.2, 3.5), at(12.2, 8.5));
    path.line_to(at(3.2, 8.5));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
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
    spotlight: Option<(&Spotlight, usize)>,
    node: NodeStyle,
    is_dark: bool,
    window: &mut Window,
) {
    let x_of = |column: u32| bounds.left() + px(GRAPH_PADDING + (column as f32 + 0.5) * LANE_WIDTH);
    let (top, bottom) = (bounds.top(), bounds.bottom());
    let middle = top + bounds.size.height / 2.;
    let node_x = x_of(row.column);

    let commit_lit = spotlight.is_none_or(|(spot, index)| spot.commit_is_lit(index));
    let line_color = |lane: &Lane, lit: bool| {
        let color = lane_color(lane.color, is_dark);
        if lit {
            color
        } else {
            color.opacity(FADED_LINE)
        }
    };
    let column_lit =
        |column: u32| spotlight.is_none_or(|(spot, index)| spot.column_is_lit(index, column));

    for lane in row.through.iter() {
        let color = line_color(lane, column_lit(lane.column));
        paint_vertical(x_of(lane.column), top, bottom, color, lane.dashed, window);
    }
    for lane in &row.incoming {
        let color = line_color(lane, column_lit(lane.column));
        let from = point(x_of(lane.column), top);
        paint_edge(from, point(node_x, middle), color, lane.dashed, window);
    }
    for lane in &row.outgoing {
        let color = line_color(lane, commit_lit);
        let to = point(x_of(lane.column), bottom);
        paint_edge(point(node_x, middle), to, color, lane.dashed, window);
    }

    if is_clipped(row) {
        let edge = bounds.right() - px(CLIPPED_MARKER_RADIUS + 1.);
        paint_circle(
            edge,
            middle,
            CLIPPED_MARKER_RADIUS + 1.5,
            node.hollow,
            window,
        );
        let color = lane_color(row.color, is_dark);
        paint_circle(edge, middle, CLIPPED_MARKER_RADIUS, color, window);
        return;
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

/// Whether a row's commit lands past the visible lanes, where its dot can't be seen.
fn is_clipped(row: &Row) -> bool {
    row.column >= MAX_VISIBLE_LANES
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
    color: Hsla,
    dashed: bool,
    window: &mut Window,
) {
    if dashed {
        let mut path = PathBuilder::stroke(px(LINE_WIDTH)).dash_array(&[px(4.), px(4.)]);
        path.move_to(point(x, top));
        path.line_to(point(x, bottom));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
        return;
    }
    let half = px(LINE_WIDTH / 2.);
    let bounds = Bounds::from_corners(point(x - half, top), point(x + half, bottom));
    window.paint_quad(fill(bounds, color));
}

fn paint_edge(
    from: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
    dashed: bool,
    window: &mut Window,
) {
    if from.x == to.x {
        paint_vertical(from.x, from.y, to.y, color, dashed, window);
        return;
    }
    let bend = (to.y - from.y) * 0.6;
    let mut path = PathBuilder::stroke(px(LINE_WIDTH));
    path.move_to(from);
    path.cubic_bezier_to(to, point(from.x, from.y + bend), point(to.x, to.y - bend));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_circle(x: Pixels, y: Pixels, radius: f32, color: Hsla, window: &mut Window) {
    let radius = px(radius);
    let bounds = Bounds::from_corners(point(x - radius, y - radius), point(x + radius, y + radius));
    window.paint_quad(fill(bounds, color).corner_radii(radius));
}

#[cfg(test)]
mod tests {
    use super::{MAX_VISIBLE_LANES, commit_chips, format_when, is_clipped};
    use crate::git::{Commit, Ref, RefKind};
    use crate::graph::GraphBuilder;
    use chrono::{Local, TimeZone};
    use std::sync::Arc;

    #[test]
    fn only_commits_past_the_last_visible_lane_are_clipped() {
        let mut builder = GraphBuilder::new();
        let parents: [Arc<str>; 1] = ["root".into()];
        let rows: Vec<_> = (0..MAX_VISIBLE_LANES + 1)
            .map(|tip| builder.push(&tip.to_string(), &parents))
            .collect();
        assert!(!is_clipped(&rows[MAX_VISIBLE_LANES as usize - 1]));
        assert!(is_clipped(&rows[MAX_VISIBLE_LANES as usize]));
    }

    fn commit_at(time: i64) -> Commit {
        Commit {
            oid: "a".into(),
            parents: Box::new([]),
            author: "a".into(),
            time,
            subject: "s".into(),
        }
    }

    #[test]
    fn recent_times_are_short_and_older_ones_name_the_date() {
        let now = Local
            .with_ymd_and_hms(2026, 11, 7, 12, 0, 0)
            .unwrap()
            .timestamp();
        let when = |ago: i64| format_when(&commit_at(now - ago), now);
        assert_eq!(when(20), "now");
        assert_eq!(when(12 * 60), "12m");
        assert_eq!(when(3 * 3600), "3h");
        assert_eq!(when(2 * 86_400), "Thu");
        assert_eq!(when(30 * 86_400), "Oct 8");
        assert_eq!(when(400 * 86_400), "2025");
    }

    fn named(kind: RefKind, name: &str) -> Ref {
        Ref {
            kind,
            name: name.to_owned(),
            is_head: false,
            color: Default::default(),
        }
    }

    #[test]
    fn a_branch_and_its_remote_at_one_commit_are_one_synced_chip() {
        let refs = vec![
            named(RefKind::Branch, "main"),
            named(RefKind::Remote, "origin/main"),
            named(RefKind::Remote, "origin/other"),
            named(RefKind::Tag, "v1"),
        ];
        let chips = commit_chips(Some(&refs));
        let names: Vec<_> = chips.iter().map(|c| c.label.name.as_str()).collect();
        assert_eq!(names, ["main", "origin/other", "v1"]);
        assert!(chips[0].synced);
        assert!(!chips[1].synced && !chips[2].synced);
    }
}
