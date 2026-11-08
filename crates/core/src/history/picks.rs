//! Which branches, tags and remote branches are picked in the sidebar. The graph spotlights
//! their histories, and the menu acts on all of them.

use crate::branch::Dragged;
use crate::history::refs::{RefKind, Refs};
use std::sync::Arc;

/// How a click changes the picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// Picks one alone, or clears when it is the only one picked.
    Plain,
    /// Adds a ref, or takes it out if it is already picked.
    Add,
    /// Picks everything from the last click to this one.
    Range,
}

/// The picked refs, by name so that two branches at one commit stay apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picks {
    items: Vec<Dragged>,
    /// Where a range starts: the ref last picked.
    anchor: Option<Dragged>,
}

impl Picks {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[Dragged] {
        &self.items
    }

    pub fn contains(&self, target: &Dragged) -> bool {
        self.items.contains(target)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Applies a click on `target`. `visible` is every ref row on screen in its list, top
    /// to bottom, which a range runs along. A range that can't be drawn there, because the
    /// last click was in another list or folded away, adds the one ref instead.
    pub fn click(&mut self, target: &Dragged, click: Click, visible: &[Dragged]) {
        let position = |wanted: &Dragged| visible.iter().position(|row| row == wanted);
        let range = match (click, &self.anchor) {
            (Click::Range, Some(anchor)) => position(anchor).zip(position(target)),
            _ => None,
        };
        match (click, range) {
            (Click::Range, Some((from, to))) => {
                let (first, last) = (from.min(to), from.max(to));
                self.items = visible[first..=last].to_vec();
            }
            (Click::Add | Click::Range, _) if !self.items.is_empty() => {
                match self.items.iter().position(|item| item == target) {
                    Some(at) => {
                        self.items.remove(at);
                    }
                    None => {
                        self.items.push(target.clone());
                        self.anchor = Some(target.clone());
                    }
                }
            }
            _ if self.items.len() == 1 && self.items[0] == *target => self.clear(),
            _ => {
                self.items = vec![target.clone()];
                self.anchor = Some(target.clone());
            }
        }
        if self.items.is_empty() {
            self.anchor = None;
        }
    }

    /// Where each picked ref points now, without repeats. Refs that no longer exist are
    /// left out.
    pub fn tips(&self, refs: &Refs) -> Vec<Arc<str>> {
        let mut tips: Vec<Arc<str>> = Vec::new();
        for tip in self.items.iter().filter_map(|item| tip_of(refs, item)) {
            if !tips.contains(&tip) {
                tips.push(tip);
            }
        }
        tips
    }

    /// Lets go of refs that were deleted or renamed since they were picked.
    pub fn retain_existing(&mut self, refs: &Refs) {
        self.items.retain(|item| tip_of(refs, item).is_some());
        if self
            .anchor
            .as_ref()
            .is_some_and(|a| tip_of(refs, a).is_none())
        {
            self.anchor = None;
        }
    }
}

/// The commit a branch, remote branch or tag points at.
pub fn tip_of(refs: &Refs, target: &Dragged) -> Option<Arc<str>> {
    match target.kind {
        RefKind::Branch => refs
            .branches
            .iter()
            .find(|r| r.name == target.name)
            .map(|r| r.tip.clone()),
        RefKind::Remote => refs
            .remotes
            .iter()
            .find(|r| r.name == target.name)
            .map(|r| r.tip.clone()),
        RefKind::Tag => refs
            .tags
            .iter()
            .find(|r| r.name == target.name)
            .map(|r| r.tip.clone()),
        RefKind::DetachedHead => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> Dragged {
        Dragged {
            kind: RefKind::Branch,
            name: name.into(),
        }
    }

    fn names(picks: &Picks) -> Vec<&str> {
        picks.items().iter().map(|d| d.name.as_str()).collect()
    }

    fn rows() -> Vec<Dragged> {
        ["a", "b", "c", "d"].map(branch).to_vec()
    }

    #[test]
    fn a_plain_click_picks_one_and_clicking_it_again_clears() {
        let (rows, mut picks) = (rows(), Picks::default());
        picks.click(&rows[0], Click::Plain, &rows);
        picks.click(&rows[1], Click::Plain, &rows);
        assert_eq!(names(&picks), ["b"]);
        picks.click(&rows[1], Click::Plain, &rows);
        assert!(picks.is_empty());
    }

    #[test]
    fn add_toggles_and_a_plain_click_narrows_to_one() {
        let (rows, mut picks) = (rows(), Picks::default());
        picks.click(&rows[0], Click::Add, &rows);
        picks.click(&rows[2], Click::Add, &rows);
        assert_eq!(names(&picks), ["a", "c"]);
        picks.click(&rows[0], Click::Add, &rows);
        assert_eq!(names(&picks), ["c"]);
        picks.click(&rows[2], Click::Add, &rows);
        assert!(picks.is_empty());

        picks.click(&rows[0], Click::Add, &rows);
        picks.click(&rows[1], Click::Add, &rows);
        picks.click(&rows[0], Click::Plain, &rows);
        assert_eq!(names(&picks), ["a"]);
    }

    #[test]
    fn a_range_runs_from_the_last_pick_and_a_new_one_replaces_it() {
        let (rows, mut picks) = (rows(), Picks::default());
        picks.click(&rows[1], Click::Plain, &rows);
        picks.click(&rows[3], Click::Range, &rows);
        assert_eq!(names(&picks), ["b", "c", "d"]);
        picks.click(&rows[0], Click::Range, &rows);
        assert_eq!(names(&picks), ["a", "b"]);
    }

    #[test]
    fn a_range_with_nowhere_to_start_adds_one() {
        let (rows, mut picks) = (rows(), Picks::default());
        picks.click(&rows[2], Click::Range, &rows);
        assert_eq!(names(&picks), ["c"]);
        // The last pick isn't among the rows shown, such as in another list.
        picks.click(&branch("elsewhere"), Click::Add, &[]);
        picks.click(&rows[0], Click::Range, &rows);
        assert_eq!(names(&picks), ["c", "elsewhere", "a"]);
    }
}
