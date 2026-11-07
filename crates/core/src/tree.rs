//! How the commit screen lists changed files: flat, or grouped by folder.

use std::collections::{BTreeMap, HashSet};

/// The two ways to list files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileView {
    #[default]
    List,
    Tree,
}

impl FileView {
    pub const ALL: [FileView; 2] = [FileView::List, FileView::Tree];

    /// The word the state file keeps.
    pub fn key(self) -> &'static str {
        match self {
            FileView::List => "list",
            FileView::Tree => "tree",
        }
    }

    /// Reads a word from the state file. Anything unknown is the list.
    pub fn from_key(key: &str) -> Self {
        match key.trim() {
            "tree" => FileView::Tree,
            _ => FileView::List,
        }
    }

    /// The label of the option.
    pub fn label(self) -> &'static str {
        match self {
            FileView::List => "List",
            FileView::Tree => "Tree",
        }
    }
}

/// One line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    Folder {
        /// The whole path, which names the folder when it is opened or closed.
        path: String,
        /// What the row shows: a chain of folders with one child each is one row.
        name: String,
        depth: usize,
        open: bool,
        /// Every file below it, by position in the list given.
        files: Vec<usize>,
    },
    File {
        /// The position in the list given.
        index: usize,
        name: String,
        depth: usize,
    },
}

#[derive(Default)]
struct Node {
    folders: BTreeMap<String, Node>,
    files: Vec<(String, usize)>,
    all: Vec<usize>,
}

/// Groups `paths` by folder. Folders come before files, each sorted by name, and what is
/// below a folder in `closed` is left out.
pub fn rows(paths: &[&str], closed: &HashSet<String>) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (index, path) in paths.iter().enumerate() {
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().unwrap_or_default();
        let mut node = &mut root;
        for part in parts {
            node = node.folders.entry(part.to_owned()).or_default();
            node.all.push(index);
        }
        node.files.push((name.to_owned(), index));
    }
    let mut out = Vec::new();
    emit(root, "", 0, closed, &mut out);
    out
}

fn emit(
    mut node: Node,
    parent: &str,
    depth: usize,
    closed: &HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for (name, mut child) in std::mem::take(&mut node.folders) {
        let mut shown = name;
        while child.files.is_empty() && child.folders.len() == 1 {
            let (next, only) = child.folders.pop_first().expect("one folder");
            shown = format!("{shown}/{next}");
            child = only;
        }
        let path = if parent.is_empty() {
            shown.clone()
        } else {
            format!("{parent}/{shown}")
        };
        let open = !closed.contains(&path);
        let mut files = std::mem::take(&mut child.all);
        files.sort_unstable();
        out.push(TreeRow::Folder {
            path: path.clone(),
            name: shown,
            depth,
            open,
            files,
        });
        if open {
            emit(child, &path, depth + 1, closed, out);
        }
    }
    node.files.sort();
    for (name, index) in node.files {
        out.push(TreeRow::File { index, name, depth });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Folder { name, depth, .. } => format!("{depth}:{name}/"),
                TreeRow::File { name, depth, .. } => format!("{depth}:{name}"),
            })
            .collect()
    }

    #[test]
    fn groups_by_folder_with_folders_first() {
        let paths = ["README.md", "src/a.rs", "src/view/b.rs", "src/view/c.rs"];
        let rows = rows(&paths, &HashSet::new());
        assert_eq!(
            names(&rows),
            [
                "0:src/",
                "1:view/",
                "2:b.rs",
                "2:c.rs",
                "1:a.rs",
                "0:README.md"
            ]
        );
    }

    #[test]
    fn single_child_chains_share_a_row() {
        let paths = ["crates/core/src/a.rs", "crates/core/src/b.rs"];
        let rows = rows(&paths, &HashSet::new());
        assert_eq!(names(&rows), ["0:crates/core/src/", "1:a.rs", "1:b.rs"]);
    }

    #[test]
    fn a_closed_folder_hides_what_is_below_but_keeps_its_files() {
        let closed = HashSet::from(["src".to_owned()]);
        let rows = rows(&["src/a.rs", "src/b.rs", "z.rs"], &closed);
        assert_eq!(names(&rows), ["0:src/", "0:z.rs"]);
        let TreeRow::Folder { open, files, .. } = &rows[0] else {
            panic!("a folder");
        };
        assert!(!open);
        assert_eq!(files, &[0, 1]);
    }

    #[test]
    fn rows_point_at_the_given_positions() {
        let rows = rows(&["b/x.rs", "a/y.rs"], &HashSet::new());
        let TreeRow::File { index, .. } = &rows[1] else {
            panic!("a file");
        };
        assert_eq!(*index, 1);
    }

    #[test]
    fn view_round_trips_and_unknown_is_the_list() {
        for view in FileView::ALL {
            assert_eq!(FileView::from_key(view.key()), view);
        }
        assert_eq!(FileView::from_key("grid"), FileView::List);
    }
}
