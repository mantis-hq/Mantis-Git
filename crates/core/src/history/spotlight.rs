//! Works out what stays lit when branches are spotlighted. One click picks a branch, and
//! picking more adds to it.

use crate::history::Entry;
use std::collections::HashMap;
use std::sync::Arc;

/// Which parts of the graph belong to a branch's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spotlight {
    /// The commit each spotlighted branch or tag points at, which identifies what's
    /// spotlighted. Never empty.
    tips: Vec<Arc<str>>,
    /// By row: whether its commit is in the history.
    commits: Vec<bool>,
    /// By row: the columns entering the top of the row that lead to the history. Rows
    /// with the same set share one allocation.
    columns: Vec<Arc<[u32]>>,
}

impl Spotlight {
    /// Walks the list once. Children always come before parents, so a commit is known to
    /// be in the history by the time the list reaches it.
    pub fn new(tips: Vec<Arc<str>>, entries: &[Entry]) -> Self {
        let rows: HashMap<&str, usize> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (&*entry.commit.oid, index))
            .collect();

        let mut commits = vec![false; entries.len()];
        for tip in &tips {
            if let Some(&index) = rows.get(&**tip) {
                commits[index] = true;
            }
        }
        for index in 0..entries.len() {
            if commits[index] {
                for parent in &entries[index].commit.parents {
                    if let Some(&row) = rows.get(&**parent) {
                        commits[row] = true;
                    }
                }
            }
        }

        let mut columns = Vec::with_capacity(entries.len());
        let mut lit: Vec<u32> = Vec::new();
        let mut shared: Arc<[u32]> = Arc::from([]);
        for (entry, in_history) in entries.iter().zip(&commits) {
            if *shared != *lit {
                shared = Arc::from(&lit[..]);
            }
            columns.push(shared.clone());
            let row = &entry.row;
            lit.retain(|column| !row.incoming.iter().any(|lane| lane.column == *column));
            if *in_history {
                for lane in &row.outgoing {
                    if !lit.contains(&lane.column) {
                        lit.push(lane.column);
                    }
                }
                lit.sort_unstable();
            }
        }

        Self {
            tips,
            commits,
            columns,
        }
    }

    pub fn tips(&self) -> &[Arc<str>] {
        &self.tips
    }

    pub fn commit_is_lit(&self, row: usize) -> bool {
        self.commits.get(row).copied().unwrap_or(false)
    }

    /// Whether a lane entering the top of a row leads to the history.
    pub fn column_is_lit(&self, row: usize, column: u32) -> bool {
        self.columns
            .get(row)
            .is_some_and(|lit| lit.contains(&column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Commit;
    use crate::history::lanes::GraphBuilder;

    fn entries(commits: &[(&str, &[&str])]) -> Vec<Entry> {
        let mut builder = GraphBuilder::new();
        commits
            .iter()
            .map(|(oid, parents)| {
                let parents: Box<[Arc<str>]> = parents.iter().map(|&p| p.into()).collect();
                Entry {
                    working: false,
                    row: builder.push(oid, &parents),
                    commit: Commit {
                        oid: (*oid).into(),
                        parents,
                        author: "a".into(),
                        email: "".into(),
                        time: 0,
                        subject: (*oid).into(),
                    },
                }
            })
            .collect()
    }

    #[test]
    fn a_branch_lights_its_history_and_not_its_sibling() {
        let list = entries(&[
            ("mine", &["base"]),
            ("other", &["base"]),
            ("base", &["root"]),
            ("root", &[]),
        ]);
        let spot = Spotlight::new(vec!["mine".into()], &list);
        assert!(spot.commit_is_lit(0));
        assert!(!spot.commit_is_lit(1));
        assert!(spot.commit_is_lit(2) && spot.commit_is_lit(3));
    }

    #[test]
    fn a_merge_lights_both_of_its_parents() {
        let list = entries(&[
            ("merge", &["left", "right"]),
            ("left", &["base"]),
            ("right", &["base"]),
            ("base", &[]),
        ]);
        let spot = Spotlight::new(vec!["merge".into()], &list);
        assert!((0..4).all(|row| spot.commit_is_lit(row)));

        let spot = Spotlight::new(vec!["right".into()], &list);
        assert!(!spot.commit_is_lit(0) && !spot.commit_is_lit(1));
        assert!(spot.commit_is_lit(2) && spot.commit_is_lit(3));
    }

    #[test]
    fn a_line_passing_a_row_is_lit_only_if_it_leads_to_the_history() {
        let list = entries(&[
            ("mine", &["base"]),
            ("other", &["base"]),
            ("third", &["other"]),
            ("base", &[]),
        ]);
        // `other` sits in column 1 and third's line runs beside mine's.
        let spot = Spotlight::new(vec!["mine".into()], &list);
        assert!(spot.column_is_lit(1, 0), "mine's line beside `other`");
        assert!(!spot.column_is_lit(1, 1), "nothing of mine in column 1");
        assert!(spot.column_is_lit(3, 0), "mine's line arrives at base");
        assert!(!spot.column_is_lit(3, 1), "other's line arrives unlit");
    }

    #[test]
    fn several_branches_light_the_union_of_their_histories() {
        let list = entries(&[
            ("mine", &["base"]),
            ("other", &["base"]),
            ("third", &["root"]),
            ("base", &["root"]),
            ("root", &[]),
        ]);
        let spot = Spotlight::new(vec!["mine".into(), "third".into()], &list);
        assert!(spot.commit_is_lit(0) && !spot.commit_is_lit(1));
        assert!(spot.commit_is_lit(2) && spot.commit_is_lit(3) && spot.commit_is_lit(4));
    }

    #[test]
    fn a_tip_outside_the_list_lights_nothing() {
        let list = entries(&[("a", &[])]);
        assert!(!Spotlight::new(vec!["gone".into()], &list).commit_is_lit(0));
    }

    #[test]
    fn unchanged_rows_share_their_column_list() {
        let list = entries(&[("a", &["b"]), ("b", &["c"]), ("c", &["d"]), ("d", &[])]);
        let spot = Spotlight::new(vec!["a".into()], &list);
        assert!(Arc::ptr_eq(&spot.columns[1], &spot.columns[2]));
    }
}
