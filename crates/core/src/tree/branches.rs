//! How the sidebar groups refs into folders by their `/` segments, so `feat/x` sits under
//! `feat`. Unlike the file tree, it keeps the order the list came in, so HEAD's branch stays
//! first, and a branch and a folder can share a name.

/// One visible row of a ref list that groups `feat/x` under a `feat` folder.
#[derive(Debug, PartialEq, Eq)]
pub enum Row {
    Folder(Folder),
    Ref {
        /// Index into the list that was grouped.
        index: usize,
        label: String,
        depth: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Folder {
    /// Full path, such as `feat/ui`.
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub count: usize,
    pub open: bool,
}

#[derive(Default)]
struct Node {
    label: String,
    path: String,
    leaf: Option<usize>,
    children: Vec<Node>,
}

impl Node {
    fn count(&self) -> usize {
        self.children
            .iter()
            .map(|c| usize::from(c.leaf.is_some()) + c.count())
            .sum()
    }
}

/// Groups names by their `/` segments. Folders and refs keep the order in which they
/// first appear, so HEAD's branch stays where the list put it. `closed` holds folder paths.
pub fn rows<'a>(names: impl Iterator<Item = &'a str>, closed: impl Fn(&str) -> bool) -> Vec<Row> {
    let mut root = Node::default();
    for (index, name) in names.enumerate() {
        let mut node = &mut root;
        let mut parts = name.split('/').peekable();
        let mut path = String::new();
        while let Some(part) = parts.next() {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            let last = parts.peek().is_none();
            let found = node
                .children
                .iter()
                .position(|c| c.label == part && c.leaf.is_some() == last);
            let at = found.unwrap_or_else(|| {
                node.children.push(Node {
                    label: part.to_owned(),
                    path: path.clone(),
                    leaf: last.then_some(index),
                    children: Vec::new(),
                });
                node.children.len() - 1
            });
            node = &mut node.children[at];
        }
    }
    fn walk(node: &Node, depth: usize, closed: &dyn Fn(&str) -> bool, out: &mut Vec<Row>) {
        for child in &node.children {
            match child.leaf {
                Some(index) => out.push(Row::Ref {
                    index,
                    label: child.label.clone(),
                    depth,
                }),
                None => {
                    let open = !closed(&child.path);
                    out.push(Row::Folder(Folder {
                        path: child.path.clone(),
                        label: child.label.clone(),
                        depth,
                        count: child.count(),
                        open,
                    }));
                    if open {
                        walk(child, depth + 1, closed, out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&root, 0, &closed, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str], closed: &[&str]) -> Vec<String> {
        super::rows(names.iter().copied(), |p| closed.contains(&p))
            .into_iter()
            .map(|row| match row {
                Row::Folder(Folder {
                    label,
                    depth,
                    count,
                    open,
                    ..
                }) => {
                    format!(
                        "{}{label}/ {count} {}",
                        " ".repeat(depth),
                        if open { "open" } else { "closed" }
                    )
                }
                Row::Ref { label, depth, .. } => format!("{}{label}", " ".repeat(depth)),
            })
            .collect()
    }

    #[test]
    fn slashes_become_folders() {
        let got = rows(&["main", "feat/a", "feat/ui/b", "fix/c", "feat/d"], &[]);
        assert_eq!(
            got,
            [
                "main",
                "feat/ 3 open",
                " a",
                " ui/ 1 open",
                "  b",
                " d",
                "fix/ 1 open",
                " c",
            ]
        );
    }

    #[test]
    fn a_closed_folder_hides_its_contents() {
        let got = rows(&["feat/a", "feat/ui/b"], &["feat"]);
        assert_eq!(got, ["feat/ 2 closed"]);
    }

    #[test]
    fn a_branch_and_a_folder_can_share_a_name() {
        let got = rows(&["feat", "feat/a"], &[]);
        assert_eq!(got, ["feat", "feat/ 1 open", " a"]);
    }
}
