//! The commit rows: lanes on a canvas, then the subject, author and time.

use super::avatars::{self, Face, avatar_mark};
use super::menu::wire_ref;
use super::parts::{TEXT_BODY, TEXT_CHIP, TEXT_SMALL, status_icon};
use super::{GraphView, SwitchWorktree};
use crate::icons::Icon;
use crate::theme::{lane_color, selected_row, tokens};
use chrono::{Datelike, Local, TimeZone, Utc};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mantisgit_core::history::columns::{COUNT, Column, ColumnOrder};
use mantisgit_core::history::lanes::{Lane, Row};
use mantisgit_core::history::refs::{Chip, RefKind};
use mantisgit_core::history::spotlight::Spotlight;
use mantisgit_core::history::{Commit, Signature, split_subject};
use mantisgit_core::repo::branch::Dragged;
use mantisgit_core::repo::forge::Forge;
use mantisgit_core::text::short_oid;
use std::ops::Range;
use std::time::Duration;

const ROW_HEIGHT: Pixels = px(32.);
const LANE_WIDTH: f32 = 16.;
const GRAPH_PADDING: f32 = 6.;
const LINE_WIDTH: f32 = 1.75;
const NODE_RADIUS: f32 = 4.;
/// The graph column picks no more than this many lanes for itself, so that a few hundred
/// stale branches can't push the text out of the window. How many lanes show is what the
/// column's width holds (`visible_lanes`), so dragging it wider reveals more. Rows with a
/// commit past the edge show a marker there instead.
const MAX_AUTO_LANES: u32 = 8;
const CLIPPED_MARKER_RADIUS: f32 = 3.;
const MAX_VISIBLE_REFS: usize = 1;
/// Refs get their own column left of the graph, so subjects stay aligned. It is as wide as
/// the widest chip needs (`Refs::widest_chips`), between these two.
const REFS_MIN_WIDTH: f32 = 48.;
const REFS_MAX_WIDTH: f32 = 180.;
/// One character of Maple Mono at the chip size, and a chip's padding around its text.
const CHIP_CELL: f32 = 6.6;
const CHIP_PADDING: f32 = 12.;
/// Long enough to cross from a chip onto the card that lists its siblings.
const REFS_CLOSE_DELAY: Duration = Duration::from_millis(150);
/// How much of Green a signed commit's shield keeps.
const SIGNED_MARK: f32 = 0.7;

/// How far things outside a spotlighted branch fade.
const FADED_ROW: f32 = 0.28;
const FADED_LINE: f32 = 0.16;

/// Seven characters of Maple Mono at the body size, with room to spare.
const HASH_WIDTH: f32 = 68.;
const AUTHOR_WIDTH: f32 = 120.;
const DATE_WIDTH: f32 = 72.;

impl GraphView {
    /// A column's width: the one the user dragged it to, else the one the app picks.
    pub(super) fn column_width(&self, column: Column) -> f32 {
        self.column_widths.get(column).unwrap_or(match column {
            Column::Refs => self.auto_refs_width(),
            Column::Graph => self.auto_graph_width(),
            Column::Subject => 0.,
            Column::Hash => HASH_WIDTH,
            Column::Author => AUTHOR_WIDTH,
            Column::Date => DATE_WIDTH,
            Column::Checks => super::checks::WIDTH,
        })
    }

    pub(super) fn graph_width(&self) -> f32 {
        self.column_width(Column::Graph)
    }

    pub(super) fn refs_width(&self) -> f32 {
        self.column_width(Column::Refs)
    }

    fn auto_graph_width(&self) -> f32 {
        let lanes = self.max_columns.clamp(1, MAX_AUTO_LANES);
        lanes as f32 * LANE_WIDTH + GRAPH_PADDING * 2.
    }

    /// The refs column's width, from the longest chip the repository has, so a repository
    /// of short branch names doesn't leave a wide empty strip beside the graph.
    fn auto_refs_width(&self) -> f32 {
        let cells = self.refs.widest_chips(MAX_VISIBLE_REFS);
        let width = cells as f32 * CHIP_CELL + CHIP_PADDING + 8.;
        width.clamp(REFS_MIN_WIDTH, REFS_MAX_WIDTH)
    }

    /// The refs at a commit. A commit with more than one shows the first, and hovering opens
    /// a card listing all of them, each of which can be dragged and right-clicked like the
    /// chip itself.
    fn render_refs_column(
        &self,
        index: usize,
        chips: &[Chip<'_>],
        width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let colors = ChipColors {
            is_dark: theme.is_dark(),
            head: theme.primary,
            on_head: theme.primary_foreground,
            tag: theme.warning,
        };
        let has_more = chips.len() > MAX_VISIBLE_REFS;
        let open = has_more && self.hovered_refs == Some(index);

        let first = chips
            .iter()
            .take(MAX_VISIBLE_REFS)
            .enumerate()
            .map(|(slot, chip)| {
                chip_element(
                    ("chip", index * 1000 + slot),
                    chip,
                    self.refs.chip_forge(chip),
                    &colors,
                    index,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let popover = open.then(|| {
            let all = chips
                .iter()
                .enumerate()
                .map(|(slot, chip)| {
                    chip_element(
                        ("popover-chip", index * 1000 + slot),
                        chip,
                        self.refs.chip_forge(chip),
                        &colors,
                        index,
                        cx,
                    )
                })
                .collect::<Vec<_>>();
            deferred(
                div()
                    .id(("refs-popover", index))
                    .absolute()
                    .top_0()
                    .left_0()
                    .v_flex()
                    .items_start()
                    .gap_1()
                    .p_1()
                    .min_w(px(width))
                    .rounded(px(8.))
                    .bg(panel)
                    .border_1()
                    .border_color(border)
                    .occlude()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        // While dragging, `on_drag_move` tracks the pointer instead.
                        if cx.has_active_drag() {
                            return;
                        }
                        this.popover_hovered = *hovered;
                        if !hovered {
                            this.leave_refs(index, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<Dragged>, _, cx| {
                            // Hover events pause during a drag, so follow the pointer instead.
                            let inside = event.bounds.contains(&event.event.position);
                            if this.popover_hovered != inside {
                                this.popover_hovered = inside;
                                if !inside {
                                    this.leave_refs(index, cx);
                                }
                            }
                        },
                    ))
                    .children(all),
            )
            .with_priority(5)
        });

        div()
            .id(("refs", index))
            .relative()
            .w(px(width))
            .h_full()
            .flex_none()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if cx.has_active_drag() {
                    return;
                }
                if *hovered {
                    if this.hovered_refs != Some(index) {
                        this.hovered_refs = Some(index);
                        cx.notify();
                    }
                } else {
                    this.leave_refs(index, cx);
                }
            }))
            .child(
                div()
                    .size_full()
                    .h_flex()
                    .gap_1p5()
                    .overflow_hidden()
                    .children(first)
                    .when(has_more, |col| {
                        col.child(
                            div()
                                .flex_none()
                                .text_size(TEXT_CHIP)
                                .text_color(muted)
                                .child(format!("+{}", chips.len() - MAX_VISIBLE_REFS)),
                        )
                    }),
            )
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<Dragged>, _, cx| {
                    // Hover events pause during a drag, so follow the pointer instead.
                    // The open card covers the rows below it, whose columns also contain the
                    // pointer, so a pointer on the card must not move the card elsewhere.
                    if event.bounds.contains(&event.event.position) {
                        if this.hovered_refs != Some(index) && !this.popover_hovered {
                            this.hovered_refs = Some(index);
                            cx.notify();
                        }
                    } else if this.hovered_refs == Some(index) && !this.popover_hovered {
                        this.leave_refs(index, cx);
                    }
                }),
            )
            .children(popover)
    }

    /// Closes the refs card shortly after the pointer leaves, unless it moved onto the card.
    pub(super) fn leave_refs(&mut self, index: usize, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFS_CLOSE_DELAY).await;
            this.update(cx, |this, cx| {
                if this.hovered_refs == Some(index) && !this.popover_hovered {
                    this.hovered_refs = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (foreground, muted) = (theme.foreground, theme.muted_foreground);
        let head = theme.primary;
        let danger = theme.danger;
        // Green, toned down so a column of signed commits stays quieter than the subjects.
        let signed = theme.success.opacity(SIGNED_MARK);
        let on_tint = theme.primary_foreground;
        let (background, hover, active) = (theme.background, theme.list_hover, selected_row(theme));
        let subtext0 = tokens(is_dark).subtext0;
        let graph_width = self.graph_width();
        let visible_lanes = visible_lanes(graph_width);
        let refs_width = self.refs_width();
        let hash_width = self.column_width(Column::Hash);
        let author_width = self.column_width(Column::Author);
        let date_width = self.column_width(Column::Date);

        let mut rows = Vec::with_capacity(range.len());
        for index in range {
            let face = {
                let commit = self.entries[index].commit.clone();
                self.avatar_face(&commit, cx)
            };
            let entry = &self.entries[index];
            let commit = &entry.commit;
            let selected = self.selection.contains(&self.entries, self.selected, index);
            let row_background = if selected { active } else { background };
            let lane_row = entry.row.clone();
            let spotlight = self.sidebar.spotlight.clone();
            let commit_lit = spotlight.as_ref().is_none_or(|s| s.commit_is_lit(index));
            let is_head = self.refs.is_head(&commit.oid);
            let is_merge = commit.is_merge();
            let is_working = entry.working;
            let is_stash = self.refs.stash_at(&commit.oid).is_some();
            let chips = self.refs.chips_at(&commit.oid);
            let signature = self.signature_of(&commit.oid);
            let worktree = self
                .refs
                .other_worktree_at(&commit.oid)
                .map(|tree| (tree.path.clone(), tree.open_hint()));
            let requests = self.requests_at(&commit.oid);
            let (prefix, description) = split_subject(&commit.subject);
            let covered = !matches!(face, Face::None)
                && !(is_working || is_stash || is_clipped(&lane_row, visible_lanes));
            let node_column = lane_row.column;
            let node_ring = if is_head {
                head
            } else {
                lane_color(lane_row.color, is_dark)
            };
            let subject_color = if is_merge { subtext0 } else { foreground };

            rows.push(
                div()
                    .id(index)
                    .h_flex()
                    .h(ROW_HEIGHT)
                    .w_full()
                    .px_3()
                    .gap_3()
                    .text_size(TEXT_BODY)
                    .bg(row_background)
                    .when(!commit_lit, |row| row.opacity(FADED_ROW))
                    .when(!selected, |row| row.hover(|style| style.bg(hover)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.open_commit_menu(index, event.position, cx);
                            window.focus(&this.focus, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.click_row(index, event.modifiers(), cx);
                        window.focus(&this.focus, cx);
                    }))
                    .children(cells(
                        self.columns,
                        self.checks_shown(),
                        [
                            self.render_refs_column(index, &chips, refs_width, cx)
                                .into_any_element(),
                            div()
                                .w(px(graph_width))
                                .h_full()
                                .flex_none()
                                .relative()
                                .overflow_hidden()
                                .child(
                                    canvas(
                                        |_, _, _| (),
                                        move |bounds, _, window, _| {
                                            paint_row(
                                                bounds,
                                                &lane_row,
                                                visible_lanes,
                                                spotlight.as_deref().map(|spot| (spot, index)),
                                                NodeStyle {
                                                    is_head,
                                                    is_merge,
                                                    is_working,
                                                    is_stash,
                                                    covered,
                                                    head,
                                                    hollow: row_background,
                                                },
                                                is_dark,
                                                window,
                                            );
                                        },
                                    )
                                    .size_full(),
                                )
                                .when(covered, |cell| {
                                    cell.child(avatar_mark(
                                        face,
                                        GRAPH_PADDING + (node_column as f32 + 0.5) * LANE_WIDTH
                                            - avatars::SIZE / 2.,
                                        (f32::from(ROW_HEIGHT) - avatars::SIZE) / 2.,
                                        node_ring,
                                        is_dark,
                                        on_tint,
                                    ))
                                })
                                .into_any_element(),
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
                                        // Your place in the history, and the row you picked.
                                        .when(is_head || selected, |subject| {
                                            subject.font_semibold()
                                        })
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(muted)
                                                .child(prefix.to_owned()),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .child(description.to_owned()),
                                        ),
                                )
                                .children(requests.into_iter().enumerate().map(
                                    |(n, (label, url))| {
                                        div()
                                            .id((
                                                ElementId::from(("request", index)),
                                                n.to_string(),
                                            ))
                                            .flex_none()
                                            .px_1p5()
                                            .rounded(px(4.))
                                            .cursor_pointer()
                                            .text_size(TEXT_CHIP)
                                            .font_semibold()
                                            .text_color(head)
                                            .bg(head.opacity(0.14))
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    "Open on the forge",
                                                )
                                                .build(window, cx)
                                            })
                                            .tooltip_show_delay(std::time::Duration::ZERO)
                                            .on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                cx.open_url(&url);
                                            })
                                            .child(label)
                                    },
                                ))
                                .children(signature.hint().map(|hint| {
                                    let (icon, color) = match signature {
                                        Signature::Bad => (Icon::ShieldAlert, danger),
                                        _ => (Icon::ShieldCheck, signed),
                                    };
                                    status_icon(
                                        (ElementId::from(("signature", index)), "signed"),
                                        icon,
                                        hint.to_owned(),
                                        color,
                                    )
                                }))
                                .children(worktree.map(|(path, hint)| {
                                    let menu_path = path.clone();
                                    status_icon(
                                        (ElementId::from(("worktree-open", index)), "worktree"),
                                        Icon::FolderOpen,
                                        hint,
                                        muted,
                                    )
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.open_worktree_menu(&menu_path, event.position, cx);
                                        }),
                                    )
                                    .on_click(cx.listener(
                                        move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.emit(SwitchWorktree(path.clone()));
                                        },
                                    ))
                                }))
                                .into_any_element(),
                            div()
                                .w(px(hash_width))
                                .flex_none()
                                .overflow_hidden()
                                .text_size(TEXT_SMALL)
                                .text_color(muted)
                                .when(!is_working, |hash| {
                                    hash.child(short_oid(&commit.oid).to_owned())
                                })
                                .into_any_element(),
                            div()
                                .w(px(author_width))
                                .flex_none()
                                .truncate()
                                .text_right()
                                .text_size(TEXT_SMALL)
                                .text_color(muted)
                                .child(SharedString::from(commit.author.clone()))
                                .into_any_element(),
                            div()
                                .w(px(date_width))
                                .flex_none()
                                .overflow_hidden()
                                .text_right()
                                .text_size(TEXT_SMALL)
                                .text_color(muted)
                                .when(!is_working, |date| {
                                    date.child(format_when(commit, Utc::now().timestamp()))
                                })
                                .into_any_element(),
                            self.render_checks_cell(index, &commit.oid, cx),
                        ],
                    )),
            );
        }
        rows
    }

    /// The row of headings over the commits, one per column, in the same order and widths.
    /// Each can be dragged onto another to change the order, which is remembered.
    pub(super) fn render_column_headers(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let primary = theme.primary;
        let separator = theme.border;
        let order = self.columns.columns();
        let subject_at = order.iter().position(|c| *c == Column::Subject);
        let headers = (order.iter().copied())
            .enumerate()
            .filter(|(_, column)| self.checks_shown() || *column != Column::Checks)
            .map(|(at, column)| {
                // The subject takes what is left, so a column after it is held by its right
                // edge and grows leftwards: its handle is on the left edge, where the pointer is.
                let after_subject = subject_at.is_some_and(|s| at > s);
                let heading = div()
                    .id(("column", column as usize))
                    .h_full()
                    .w_full()
                    .h_flex()
                    .cursor_pointer()
                    .rounded(px(4.))
                    .when(matches!(column, Column::Author | Column::Date), |h| {
                        h.justify_end()
                    })
                    .child(column.label())
                    .on_drag(DraggedColumn(column), |dragged, _, _, cx| {
                        cx.new(|_| dragged.clone())
                    })
                    .can_drop(move |dragged, _, _| {
                        dragged
                            .downcast_ref::<DraggedColumn>()
                            .is_some_and(|source| source.0 != column)
                    })
                    .drag_over::<DraggedColumn>(|style, _, _, cx| {
                        style.bg(cx.theme().primary.opacity(0.14))
                    })
                    .on_drop(cx.listener(move |this, source: &DraggedColumn, _, cx| {
                        this.move_column(source.0, column, cx)
                    }))
                    .tooltip(move |window, cx| {
                        Tooltip::new(format!("Drag to move {}", column.label())).build(window, cx)
                    });
                // The wrapper takes the column's width, so the handle can sit on its right edge
                // without being part of the heading that drags to reorder.
                let wrapper = div().h_full().relative().min_w_0();
                if column == Column::Subject {
                    return wrapper.flex_1().child(heading);
                }
                let width_now = self.column_width(column);
                // A 1 px line shows where the edge is. It sits in the gap between columns.
                let handle = div()
                    .id(("column-edge", column as usize))
                    .absolute()
                    .top_0()
                    .when(after_subject, |h| h.left(px(-10.)))
                    .when(!after_subject, |h| h.right(px(-10.)))
                    .w(px(8.))
                    .h_full()
                    .h_flex()
                    .justify_center()
                    .cursor_col_resize()
                    .hover(move |style| style.bg(primary.opacity(0.5)))
                    .child(div().w(px(1.)).h(px(14.)).self_center().bg(separator))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.column_grab = Some((f32::from(event.position.x), width_now));
                        }),
                    )
                    .on_drag(ResizeColumn(column), |_, _, _, cx| cx.new(|_| Ghost))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        if event.click_count() == 2 {
                            this.reset_column_width(column, cx);
                        }
                    }))
                    .tooltip(move |window, cx| {
                        Tooltip::new(format!("Drag to resize {}", column.label())).build(window, cx)
                    });
                wrapper
                    .w(px(width_now))
                    .flex_none()
                    .child(heading)
                    .child(deferred(handle).with_priority(5))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<ResizeColumn>, _, cx| {
                            if event.drag(cx).0 != column {
                                return;
                            }
                            let Some((start_x, start_width)) = this.column_grab else {
                                return;
                            };
                            // The edge the pointer holds moves and the other stays: a column
                            // before the subject grows rightwards, one after it leftwards.
                            // Measured from the grab, not from the column's bounds, which
                            // change as it is resized.
                            let moved = f32::from(event.event.position.x) - start_x;
                            let width = if after_subject {
                                start_width - moved
                            } else {
                                start_width + moved
                            };
                            this.set_column_width(column, width);
                            cx.notify();
                        },
                    ))
            });
        div()
            .h_flex()
            .h(px(28.))
            .w_full()
            .flex_none()
            // The same inset and gap as the rows, so each heading stands over its column.
            .px_3()
            .gap_3()
            .bg(panel)
            .border_b_1()
            .border_color(border)
            .text_size(TEXT_SMALL)
            .font_medium()
            .text_color(muted)
            // Whichever way the drag ends, what was dragged to is kept.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_column_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_column_resize(cx)),
            )
            .children(headers)
    }
}

/// What a drag of a column's right edge carries. It draws nothing: the column itself moves.
struct ResizeColumn(Column);

struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// What the cursor carries while a column heading is dragged.
#[derive(Clone)]
struct DraggedColumn(Column);

impl Render for DraggedColumn {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .h_flex()
            .h(px(28.))
            .px_2()
            .rounded(px(6.))
            .bg(theme.title_bar)
            .border_1()
            .border_color(theme.border)
            .text_size(TEXT_SMALL)
            .font_medium()
            .text_color(theme.foreground)
            .child(self.0.label())
    }
}

/// The cells of one row, given in the columns' usual order, set in the order chosen.
fn cells(order: ColumnOrder, checks: bool, cells: [AnyElement; COUNT]) -> Vec<AnyElement> {
    let mut cells: Vec<_> = (Column::ALL.into_iter().zip(cells))
        // The checks column is only there while there is a forge to ask.
        .filter(|(column, _)| checks || *column != Column::Checks)
        .collect();
    let place = |column: Column| order.columns().iter().position(|&other| other == column);
    cells.sort_by_key(|(column, _)| place(*column));
    cells.into_iter().map(|(_, cell)| cell).collect()
}

/// When a commit happened, as short as it can be said.
pub(super) fn format_when(commit: &Commit, now: i64) -> String {
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

/// What chips are drawn with.
struct ChipColors {
    is_dark: bool,
    head: Hsla,
    on_head: Hsla,
    tag: Hsla,
}

/// A chip that can be right-clicked and, unless HEAD is detached, dragged.
fn chip_element(
    id: impl Into<ElementId>,
    chip: &Chip,
    forge: Option<Forge>,
    colors: &ChipColors,
    row: usize,
    cx: &mut Context<GraphView>,
) -> Stateful<Div> {
    let label = chip.label;
    let lane = lane_color(label.color, colors.is_dark);
    let element = ref_label(
        id,
        chip,
        forge,
        lane,
        colors.head,
        colors.on_head,
        colors.tag,
    );
    match label.kind {
        // Not a name anything can be done to.
        RefKind::DetachedHead => element,
        kind => {
            let color = if kind == RefKind::Tag {
                colors.tag
            } else {
                lane
            };
            let target = Dragged {
                kind,
                name: label.name.clone(),
            };
            let clicked = target.clone();
            wire_ref(element, target, color, Some(row), cx).on_click(cx.listener(
                move |this, event: &ClickEvent, _, cx| {
                    this.open_on_double_click(&clicked, event, cx)
                },
            ))
        }
    }
}

fn ref_label(
    id: impl Into<ElementId>,
    chip: &Chip,
    forge: Option<Forge>,
    lane: Hsla,
    head: Hsla,
    on_head: Hsla,
    tag: Hsla,
) -> Stateful<Div> {
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
        .id(id)
        .h_flex()
        .flex_none()
        .gap_1()
        .h(px(20.))
        .max_w(px(REFS_MAX_WIDTH - 36.))
        .px_1p5()
        .rounded(px(4.))
        .text_size(TEXT_CHIP)
        .font_semibold()
        .text_color(text)
        .bg(fill)
        .when(r.kind == RefKind::Tag, |label| {
            label.child(chip_icon(Icon::Tag, text))
        })
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
        .when_some(forge, |label, forge| {
            let icon = match forge {
                Forge::GitHub => Icon::GitHub,
                Forge::GitLab => Icon::GitLab,
                Forge::Gitea => Icon::Gitea,
            };
            label.child(chip_icon(icon, text))
        })
}

/// A small picture in a chip, in the chip's text colour.
fn chip_icon(icon: Icon, color: Hsla) -> impl IntoElement {
    svg()
        .path(icon.path())
        .size(px(11.))
        .flex_none()
        .text_color(color)
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
    /// A stash: a dashed ring in its lane's colour, as it isn't history yet.
    is_stash: bool,
    /// An avatar is drawn over the dot, so the dot isn't.
    covered: bool,
    head: Hsla,
    /// Fills the middle of merge nodes and the HEAD ring, so it has to match the row
    /// behind them.
    hollow: Hsla,
}

fn paint_row(
    bounds: Bounds<Pixels>,
    row: &Row,
    visible_lanes: u32,
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

    if is_clipped(row, visible_lanes) {
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

    if node.covered {
        return;
    }
    if node.is_working {
        paint_dashed_ring(node_x, middle, NODE_RADIUS + 1., node.head, window);
        return;
    }
    if node.is_stash {
        let color = lane_color(row.color, is_dark);
        paint_dashed_ring(node_x, middle, NODE_RADIUS + 1., color, window);
        return;
    }
    if node.is_head {
        paint_circle(node_x, middle, NODE_RADIUS + 3., node.head, window);
        paint_circle(node_x, middle, NODE_RADIUS + 1.5, node.hollow, window);
        paint_circle(node_x, middle, NODE_RADIUS, node.head, window);
        if node.is_merge {
            paint_circle(node_x, middle, NODE_RADIUS - 1.75, node.hollow, window);
        }
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
fn is_clipped(row: &Row, visible_lanes: u32) -> bool {
    row.column >= visible_lanes
}

/// The lanes a graph column of this width holds, at least one.
fn visible_lanes(width: f32) -> u32 {
    (((width - GRAPH_PADDING * 2.) / LANE_WIDTH).floor() as u32).max(1)
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
    use super::{GRAPH_PADDING, LANE_WIDTH, format_when, is_clipped, visible_lanes};
    use chrono::{Local, TimeZone};
    use mantisgit_core::history::Commit;
    use mantisgit_core::history::lanes::GraphBuilder;
    use std::sync::Arc;

    #[test]
    fn only_commits_past_the_last_visible_lane_are_clipped() {
        let mut builder = GraphBuilder::new();
        let parents: [Arc<str>; 1] = ["root".into()];
        let rows: Vec<_> = (0..9)
            .map(|tip| builder.push(&tip.to_string(), &parents))
            .collect();
        assert!(!is_clipped(&rows[7], 8));
        assert!(is_clipped(&rows[8], 8));
        // A wider column shows the ninth lane too.
        assert!(!is_clipped(&rows[8], 9));
    }

    #[test]
    fn a_column_shows_the_lanes_its_width_holds() {
        assert_eq!(visible_lanes(8. * LANE_WIDTH + GRAPH_PADDING * 2.), 8);
        assert_eq!(visible_lanes(8. * LANE_WIDTH + GRAPH_PADDING * 2. - 1.), 7);
        assert_eq!(visible_lanes(600.), 36);
        assert_eq!(visible_lanes(0.), 1);
    }

    fn commit_at(time: i64) -> Commit {
        Commit {
            oid: "a".into(),
            parents: Box::new([]),
            author: "a".into(),
            email: "".into(),
            time,
            subject: "s".into(),
            signature: Default::default(),
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
}
