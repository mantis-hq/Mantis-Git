//! Assigns commits to lanes for the commit graph.
//!
//! The layout is incremental: commits are fed in one at a time, children before parents
//! (which `git log --date-order` guarantees), so a history of any size can be laid out
//! while it streams in. Nothing here depends on the UI.

use std::collections::HashMap;
use std::sync::Arc;

/// What a lane is drawn in. Blue means you, so it is kept apart from the branch palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaneColor {
    Head,
    /// An index into the palette. Not bounded by the palette size.
    Branch(u32),
}

impl Default for LaneColor {
    fn default() -> Self {
        Self::Branch(0)
    }
}

/// What the layout needs to know about a repository before seeing its commits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Gets the first lane to itself, so that your history is a straight line.
    pub head: Option<Arc<str>>,
    /// Where HEAD's line joins shared history. Below it the line takes `shared_color`.
    pub fork: Option<Arc<str>>,
    pub shared_color: LaneColor,
    /// The colours of named branches, by the commit they point at.
    pub tips: HashMap<Arc<str>, LaneColor>,
    /// The first palette index no named branch uses, for lanes that have no name.
    pub first_free_color: u32,
}

/// One vertical track in a row of the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Lane {
    pub column: u32,
    pub color: LaneColor,
    /// The line from uncommitted work to HEAD, which isn't history yet.
    pub dashed: bool,
}

/// Everything needed to draw one commit's row, independent of its neighbours.
///
/// A row is drawn in two halves that meet at the commit's node. Lanes leaving the bottom
/// of a row are exactly the lanes entering the top of the next one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub column: u32,
    pub color: LaneColor,
    /// Lanes that pass this row without touching the commit. Rows with the same set share
    /// one allocation, which keeps long linear histories cheap.
    pub through: Arc<[Lane]>,
    /// Lanes that end at the commit: edges from its children, drawn in the top half.
    pub incoming: Box<[Lane]>,
    /// Lanes the commit feeds into: edges to its parents, drawn in the bottom half.
    pub outgoing: Box<[Lane]>,
}

impl Row {
    /// The number of columns needed to draw this row.
    pub fn width(&self) -> u32 {
        self.through
            .iter()
            .chain(&self.incoming)
            .chain(&self.outgoing)
            .map(|lane| lane.column)
            .fold(self.column, u32::max)
            + 1
    }
}

#[derive(Clone)]
struct Slot {
    expects: Arc<str>,
    color: LaneColor,
    /// Holds the first lane for HEAD until it arrives. Nothing is drawn for it, because
    /// commits listed above HEAD have no line to it.
    reserved: bool,
    dashed: bool,
}

#[derive(Clone, Default)]
pub struct GraphBuilder {
    slots: Vec<Option<Slot>>,
    next_color: u32,
    plan: Plan,
    last_through: Option<Arc<[Lane]>>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_plan(plan: Plan) -> Self {
        let slots = plan.head.iter().map(|head| {
            Some(Slot {
                expects: head.clone(),
                color: LaneColor::Head,
                reserved: true,
                dashed: false,
            })
        });
        Self {
            slots: slots.collect(),
            next_color: plan.first_free_color,
            plan,
            last_through: None,
        }
    }

    /// Lays out the row for uncommitted changes, which sits above every commit. Its dashed
    /// line runs down HEAD's lane. Call this before any commit.
    pub fn working_row(&mut self) -> Row {
        let outgoing = self.slots.first_mut().and_then(Option::as_mut).map(|slot| {
            slot.reserved = false;
            slot.dashed = true;
            Lane {
                column: 0,
                color: LaneColor::Head,
                dashed: true,
            }
        });
        Row {
            column: 0,
            color: LaneColor::Head,
            through: self.snapshot_through_without(0),
            incoming: Box::default(),
            outgoing: outgoing.into_iter().collect(),
        }
    }

    /// Lays out the next commit. `parents` is in git's order, first parent first.
    pub fn push(&mut self, oid: &str, parents: &[Arc<str>]) -> Row {
        let mut incoming = Vec::new();
        let mut reserved_column = None;
        for (column, slot) in self.slots.iter_mut().enumerate() {
            if slot.as_ref().is_some_and(|slot| &*slot.expects == oid) {
                let Some(slot) = slot.take() else { continue };
                if slot.reserved {
                    reserved_column = Some(column);
                } else {
                    incoming.push(Lane {
                        column: column as u32,
                        color: slot.color,
                        dashed: slot.dashed,
                    });
                }
            }
        }

        // The leftmost child's lane carries on through this commit. With no children this
        // is a branch tip, which starts a new lane in the first gap.
        let (column, mut color) = match (reserved_column, incoming.first()) {
            (Some(column), _) => (column, LaneColor::Head),
            (None, Some(lane)) => (lane.column as usize, lane.color),
            (None, None) => (self.free_column(0), self.color_for(oid)),
        };
        if color == LaneColor::Head && self.plan.fork.as_deref() == Some(oid) {
            color = self.plan.shared_color;
        }

        let through = self.snapshot_through();

        let mut outgoing: Vec<Lane> = Vec::with_capacity(parents.len());
        for (index, parent) in parents.iter().enumerate() {
            if parents[..index].contains(parent) {
                continue;
            }
            let lane = if index == 0 {
                // Stays in this lane even if another lane already expects the parent, so
                // a branch reads as one line down to the commit it forked from.
                self.occupy(column, parent, color)
            } else if let Some(lane) = self.lane_expecting(parent) {
                lane
            } else {
                // Prefer the right of the node, so the new edge crosses fewer lanes.
                let new_column = self.free_column(column + 1);
                let new_color = self.color_for(parent);
                self.occupy(new_column, parent, new_color)
            };
            outgoing.push(lane);
        }

        while matches!(self.slots.last(), Some(None)) {
            self.slots.pop();
        }

        Row {
            column: column as u32,
            color,
            through,
            incoming: incoming.into(),
            outgoing: outgoing.into(),
        }
    }

    fn free_column(&self, from: usize) -> usize {
        (from..self.slots.len())
            .find(|&column| self.slots[column].is_none())
            .unwrap_or(self.slots.len().max(from))
    }

    /// A branch's own colour if a branch points here, otherwise the next unused one.
    fn color_for(&mut self, oid: &str) -> LaneColor {
        if let Some(color) = self.plan.tips.get(oid) {
            return *color;
        }
        let color = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        LaneColor::Branch(color)
    }

    fn occupy(&mut self, column: usize, parent: &Arc<str>, color: LaneColor) -> Lane {
        if self.slots.len() <= column {
            self.slots.resize_with(column + 1, || None);
        }
        self.slots[column] = Some(Slot {
            expects: parent.clone(),
            color,
            reserved: false,
            dashed: false,
        });
        Lane {
            column: column as u32,
            color,
            dashed: false,
        }
    }

    /// The commits the open lanes wait for: parents of commits laid out so far that have
    /// not come yet, and HEAD while it is held for.
    pub fn awaited(&self) -> impl Iterator<Item = &Arc<str>> {
        self.slots.iter().flatten().map(|slot| &slot.expects)
    }

    fn lane_expecting(&self, oid: &str) -> Option<Lane> {
        self.slots.iter().enumerate().find_map(|(column, slot)| {
            let slot = slot
                .as_ref()
                .filter(|slot| !slot.reserved && !slot.dashed)?;
            (&*slot.expects == oid).then_some(Lane {
                column: column as u32,
                color: slot.color,
                dashed: false,
            })
        })
    }

    /// The working row is above everything, so the only lane could be its own.
    fn snapshot_through_without(&self, column: u32) -> Arc<[Lane]> {
        let lanes: Vec<Lane> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _)| *index as u32 != column)
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .filter(|slot| !slot.reserved)
                    .map(|slot| Lane {
                        column: index as u32,
                        color: slot.color,
                        dashed: slot.dashed,
                    })
            })
            .collect();
        lanes.into()
    }

    fn snapshot_through(&mut self) -> Arc<[Lane]> {
        let lanes = self.slots.iter().enumerate().filter_map(|(column, slot)| {
            slot.as_ref()
                .filter(|slot| !slot.reserved)
                .map(|slot| Lane {
                    column: column as u32,
                    color: slot.color,
                    dashed: slot.dashed,
                })
        });
        if let Some(last) = &self.last_through
            && last.iter().copied().eq(lanes.clone())
        {
            return last.clone();
        }
        let through: Arc<[Lane]> = lanes.collect();
        self.last_through = Some(through.clone());
        through
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn layout(commits: &[(&str, &[&str])]) -> Vec<Row> {
        layout_with(Plan::default(), commits)
    }

    fn layout_with(plan: Plan, commits: &[(&str, &[&str])]) -> Vec<Row> {
        let mut builder = GraphBuilder::with_plan(plan);
        commits
            .iter()
            .map(|(oid, parents)| {
                let parents: Vec<Arc<str>> = parents.iter().map(|&p| p.into()).collect();
                builder.push(oid, &parents)
            })
            .collect()
    }

    fn columns(lanes: &[Lane]) -> Vec<u32> {
        lanes.iter().map(|lane| lane.column).collect()
    }

    /// Lanes leaving the bottom of a row must be the lanes entering the top of the next,
    /// or the drawn lines would have gaps or stubs.
    fn assert_continuous(rows: &[Row]) {
        for (index, pair) in rows.windows(2).enumerate() {
            let bottom: BTreeSet<Lane> = pair[0]
                .through
                .iter()
                .chain(&pair[0].outgoing)
                .copied()
                .collect();
            let top: BTreeSet<Lane> = pair[1]
                .through
                .iter()
                .chain(&pair[1].incoming)
                .copied()
                .collect();
            assert_eq!(bottom, top, "discontinuity below row {index}");
        }
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let rows = layout(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        assert!(
            rows.iter()
                .all(|row| row.column == 0 && row.color == LaneColor::Branch(0))
        );
        assert!(rows[0].incoming.is_empty(), "a tip has nothing above it");
        assert!(rows[2].outgoing.is_empty(), "a root has nothing below it");
        assert_continuous(&rows);
    }

    #[test]
    fn linear_history_shares_the_through_allocation() {
        let rows = layout(&[
            ("x", &["y"]),
            ("c", &["b"]),
            ("b", &["a"]),
            ("a", &["z"]),
            ("y", &["z"]),
            ("z", &[]),
        ]);
        assert!(Arc::ptr_eq(&rows[2].through, &rows[3].through));
    }

    #[test]
    fn branch_and_merge() {
        // m merges side into main; both fork from base.
        let rows = layout(&[
            ("m", &["main", "side"]),
            ("main", &["base"]),
            ("side", &["base"]),
            ("base", &[]),
        ]);
        assert_eq!(rows[0].column, 0);
        assert_eq!(columns(&rows[0].outgoing), [0, 1]);
        assert_eq!(rows[1].column, 0);
        assert_eq!(columns(&rows[1].through), [1]);
        assert_eq!(rows[2].column, 1);
        assert_eq!(rows[3].column, 0);
        assert_eq!(columns(&rows[3].incoming), [0, 1]);
        assert_ne!(rows[0].outgoing[0].color, rows[0].outgoing[1].color);
        assert_continuous(&rows);
    }

    #[test]
    fn two_tips_converge_at_their_fork_point() {
        let rows = layout(&[("a", &["base"]), ("b", &["base"]), ("base", &[])]);
        assert_eq!((rows[0].column, rows[1].column), (0, 1));
        assert_eq!(columns(&rows[2].incoming), [0, 1]);
        assert_eq!(rows[2].column, 0, "the leftmost lane continues");
        assert_continuous(&rows);
    }

    #[test]
    fn second_parent_joins_a_lane_that_already_expects_it() {
        let rows = layout(&[
            ("tip", &["p"]),
            ("merge", &["q", "p"]),
            ("p", &["q"]),
            ("q", &[]),
        ]);
        assert_eq!(rows[1].column, 1);
        assert_eq!(columns(&rows[1].outgoing), [1, 0]);
        assert_continuous(&rows);
    }

    #[test]
    fn octopus_merge_opens_a_lane_per_parent() {
        let rows = layout(&[("o", &["a", "b", "c"]), ("a", &[]), ("b", &[]), ("c", &[])]);
        assert_eq!(columns(&rows[0].outgoing), [0, 1, 2]);
        assert_continuous(&rows);
    }

    #[test]
    fn freed_columns_are_reused() {
        let rows = layout(&[
            ("a", &["r"]),
            ("b", &["r2"]),
            ("r2", &[]),
            ("c", &["r"]),
            ("r", &[]),
        ]);
        assert_eq!(rows[1].column, 1);
        assert_eq!(rows[3].column, 1, "the column freed by r2 is taken again");
        assert_continuous(&rows);
    }

    #[test]
    fn duplicate_parents_draw_one_edge() {
        let rows = layout(&[("m", &["p", "p"]), ("p", &[])]);
        assert_eq!(columns(&rows[0].outgoing), [0]);
        assert_continuous(&rows);
    }

    #[test]
    fn unrelated_histories_get_separate_lanes() {
        let rows = layout(&[("a", &["a0"]), ("b", &["b0"]), ("a0", &[]), ("b0", &[])]);
        assert_eq!((rows[0].column, rows[1].column), (0, 1));
        assert_ne!(rows[0].color, rows[1].color);
        assert_continuous(&rows);
    }

    #[test]
    fn parents_outside_the_history_leave_a_lane_open() {
        // A shallow clone can end with edges pointing at commits that were never fetched.
        let rows = layout(&[("a", &["missing"]), ("b", &[])]);
        assert_eq!(columns(&rows[0].outgoing), [0]);
        assert_eq!(columns(&rows[1].through), [0]);
    }

    #[test]
    fn width_counts_every_lane() {
        let rows = layout(&[("o", &["a", "b", "c"]), ("a", &[]), ("b", &[]), ("c", &[])]);
        assert_eq!(rows[0].width(), 3);
        assert_eq!(rows[3].width(), 3);
    }

    fn head_plan(head: &str) -> Plan {
        Plan {
            head: Some(head.into()),
            ..Plan::default()
        }
    }

    #[test]
    fn head_takes_the_first_lane_even_below_newer_commits() {
        let rows = layout_with(
            head_plan("h"),
            &[("x", &["base"]), ("h", &["base"]), ("base", &[])],
        );
        assert_eq!(rows[0].column, 1, "the first lane stays free for HEAD");
        assert_eq!(rows[1].column, 0);
        assert_eq!(rows[1].color, LaneColor::Head);
        assert_eq!(rows[2].column, 0);
        assert_continuous(&rows);
    }

    #[test]
    fn nothing_is_drawn_for_head_before_it_arrives() {
        let rows = layout_with(
            head_plan("h"),
            &[
                ("x", &["h"]),
                ("y", &["h"]),
                ("h", &["base"]),
                ("base", &[]),
            ],
        );
        assert!(rows[0].through.is_empty());
        assert_eq!(columns(&rows[1].through), [1]);
        assert_eq!(rows[2].column, 0);
        assert_eq!(columns(&rows[2].incoming), [1, 2]);
        assert_continuous(&rows);
    }

    #[test]
    fn a_merge_into_head_from_above_gets_a_visible_lane() {
        let rows = layout_with(
            head_plan("h"),
            &[("m", &["other", "h"]), ("other", &[]), ("h", &[])],
        );
        assert_eq!(columns(&rows[0].outgoing), [1, 2]);
        assert_eq!(columns(&rows[2].incoming), [2]);
        assert_continuous(&rows);
    }

    #[test]
    fn working_changes_sit_above_head_on_a_dashed_line() {
        let mut builder = GraphBuilder::with_plan(head_plan("h"));
        let top = builder.working_row();
        let newer = builder.push("x", &["base".into()]);
        let head = builder.push("h", &["base".into()]);
        let base = builder.push("base", &[]);

        assert_eq!(top.column, 0);
        assert!(top.outgoing[0].dashed);
        assert!(newer.through[0].dashed, "the line passes newer commits");
        assert!(head.incoming[0].dashed, "and reaches HEAD");
        assert!(!head.outgoing[0].dashed, "below HEAD it's ordinary history");
        assert_continuous(&[top, newer, head, base]);
    }

    #[test]
    fn working_changes_with_no_commits_have_no_line() {
        let mut builder = GraphBuilder::new();
        assert!(builder.working_row().outgoing.is_empty());
    }

    #[test]
    fn heads_line_is_blue_down_to_the_fork_then_takes_the_shared_colour() {
        let plan = Plan {
            head: Some("h".into()),
            fork: Some("fork".into()),
            shared_color: LaneColor::Branch(0),
            tips: [("main".into(), LaneColor::Branch(0))].into(),
            first_free_color: 1,
        };
        let rows = layout_with(
            plan,
            &[
                ("h", &["h1"]),
                ("main", &["fork"]),
                ("h1", &["fork"]),
                ("fork", &["root"]),
                ("root", &[]),
            ],
        );
        assert_eq!(rows[0].color, LaneColor::Head);
        assert_eq!(rows[1].color, LaneColor::Branch(0));
        assert_eq!(rows[2].color, LaneColor::Head);
        assert_eq!(rows[3].color, LaneColor::Branch(0), "shared history");
        assert_eq!(rows[3].column, 0);
        assert_eq!(rows[3].incoming[0].color, LaneColor::Head, "the line in");
        assert_eq!(rows[3].outgoing[0].color, LaneColor::Branch(0));
        assert_eq!(rows[4].color, LaneColor::Branch(0));
        assert_continuous(&rows);
    }

    #[test]
    fn named_tips_take_their_branch_colour_and_others_a_free_one() {
        let plan = Plan {
            tips: [("named".into(), LaneColor::Branch(4))].into(),
            first_free_color: 5,
            ..Plan::default()
        };
        let rows = layout_with(plan, &[("named", &[]), ("anon", &[])]);
        assert_eq!(rows[0].color, LaneColor::Branch(4));
        assert_eq!(rows[1].color, LaneColor::Branch(5));
    }

    #[test]
    fn random_histories_stay_continuous() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };

        for _ in 0..200 {
            let count = 2 + next(120);
            let names: Vec<String> = (0..count).map(|index| index.to_string()).collect();
            let head = next(count);
            let plan = if next(2) == 0 {
                head_plan(&names[head])
            } else {
                Plan::default()
            };
            let mut builder = GraphBuilder::with_plan(plan);
            let mut rows = Vec::new();
            let mut expected_parents = Vec::new();

            for index in 0..count {
                // Parents always come later, as with --date-order.
                let later = count - index - 1;
                let parents: Vec<Arc<str>> = match (later, next(10)) {
                    (0, _) | (_, 0) => vec![],
                    (_, 1) => (0..3)
                        .map(|_| names[index + 1 + next(later)].as_str().into())
                        .collect(),
                    (_, 2 | 3) => (0..2)
                        .map(|_| names[index + 1 + next(later)].as_str().into())
                        .collect(),
                    _ => vec![names[index + 1 + next(later.min(4))].as_str().into()],
                };
                expected_parents.push(parents.iter().collect::<BTreeSet<_>>().len());
                rows.push(builder.push(&names[index], &parents));
            }

            assert_continuous(&rows);
            for (row, parents) in rows.iter().zip(expected_parents) {
                assert_eq!(row.outgoing.len(), parents);
                let bottom: BTreeSet<u32> = columns(&row.outgoing).into_iter().collect();
                assert_eq!(bottom.len(), parents, "two parents share a lane");
            }
            let last = rows.last().unwrap();
            assert!(last.through.is_empty() && last.outgoing.is_empty());
        }
    }
}
