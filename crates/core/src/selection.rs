//! Which commits are selected. One is the main selection, which the inspector shows, and
//! the rest are marked with ctrl-click or shift-click.

use crate::git::{Commit, Entry};
use std::collections::HashSet;
use std::sync::Arc;

/// The commits marked beside the main selection. They're kept by hash, so a reload that
/// reorders history keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    marked: HashSet<Arc<str>>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    pub fn clear(&mut self) {
        self.marked.clear();
    }

    pub fn is_marked(&self, oid: &str) -> bool {
        self.marked.contains(oid)
    }

    /// Marks a commit, or unmarks it if it was marked.
    pub fn toggle(&mut self, oid: &Arc<str>) {
        if !self.marked.remove(oid) {
            self.marked.insert(oid.clone());
        }
    }

    /// Takes one marked commit out, for when the main selection is ctrl-clicked away. The
    /// newest goes first, so the choice doesn't depend on hash order.
    pub fn promote(&mut self, entries: &[Entry]) -> Option<Arc<str>> {
        let next = entries
            .iter()
            .find(|e| self.marked.contains(&e.commit.oid))?
            .commit
            .oid
            .clone();
        self.marked.remove(&next);
        Some(next)
    }

    /// Marks every commit from the main selection to `to`, inclusive, replacing earlier marks.
    pub fn extend(&mut self, entries: &[Entry], main: usize, to: usize) {
        let (first, last) = (main.min(to), main.max(to));
        self.marked = entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| (first..=last).contains(index) && !entry.working)
            .map(|(_, entry)| entry.commit.oid.clone())
            .collect();
        if let Some(entry) = entries.get(main) {
            self.marked.remove(&entry.commit.oid);
        }
    }

    /// The selected commits, newest first: the main one and the marked ones. The working
    /// changes are never one of them.
    pub fn commits(&self, entries: &[Entry], main: Option<usize>) -> Vec<Commit> {
        entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                !entry.working && (Some(*index) == main || self.marked.contains(&entry.commit.oid))
            })
            .map(|(_, entry)| entry.commit.clone())
            .collect()
    }

    /// Whether a click on `index` is inside the selection.
    pub fn contains(&self, entries: &[Entry], main: Option<usize>, index: usize) -> bool {
        main == Some(index)
            || entries
                .get(index)
                .is_some_and(|entry| self.marked.contains(&entry.commit.oid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;

    /// Linear history, newest first. The first row stands in for the working changes.
    fn entries(count: usize) -> Vec<Entry> {
        let mut builder = GraphBuilder::new();
        (0..count)
            .map(|index| {
                let oid: Arc<str> = format!("oid{index}").into();
                let parents: Box<[Arc<str>]> = (index + 1 < count)
                    .then(|| format!("oid{}", index + 1).into())
                    .into_iter()
                    .collect();
                Entry {
                    row: builder.push(&oid, &parents),
                    commit: Commit {
                        oid,
                        parents,
                        author: "Ada".into(),
                        time: 0,
                        subject: format!("commit {index}").into(),
                    },
                    working: index == 0,
                }
            })
            .collect()
    }

    fn subjects(commits: &[Commit]) -> Vec<&str> {
        commits.iter().map(|c| &*c.subject).collect()
    }

    #[test]
    fn the_main_commit_and_marked_ones_come_out_newest_first() {
        let entries = entries(6);
        let mut selection = Selection::default();
        selection.toggle(&entries[4].commit.oid);
        selection.toggle(&entries[2].commit.oid);
        let picked = selection.commits(&entries, Some(3));
        assert_eq!(subjects(&picked), ["commit 2", "commit 3", "commit 4"]);

        selection.toggle(&entries[2].commit.oid);
        assert_eq!(
            subjects(&selection.commits(&entries, Some(3))),
            ["commit 3", "commit 4"]
        );
    }

    #[test]
    fn a_range_covers_everything_between_and_skips_the_working_changes() {
        let entries = entries(6);
        let mut selection = Selection::default();
        selection.extend(&entries, 4, 0);
        let picked = selection.commits(&entries, Some(4));
        assert_eq!(
            subjects(&picked),
            ["commit 1", "commit 2", "commit 3", "commit 4"]
        );

        // A new range replaces the old one rather than adding to it.
        selection.extend(&entries, 4, 5);
        assert_eq!(
            subjects(&selection.commits(&entries, Some(4))),
            ["commit 4", "commit 5"]
        );
    }

    #[test]
    fn taking_the_main_selection_away_promotes_the_newest_marked_commit() {
        let entries = entries(6);
        let mut selection = Selection::default();
        selection.toggle(&entries[5].commit.oid);
        selection.toggle(&entries[2].commit.oid);
        assert_eq!(selection.promote(&entries).as_deref(), Some("oid2"));
        assert!(!selection.is_marked("oid2") && selection.is_marked("oid5"));
        assert!(selection.contains(&entries, Some(1), 1));
        assert!(selection.contains(&entries, Some(1), 5));
        assert!(!selection.contains(&entries, Some(1), 3));
    }
}
