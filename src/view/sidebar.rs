//! The branch sidebar. Clicking a branch spotlights its history in the graph.

use super::GraphView;
use super::menu::wire_ref;
use super::parts::{panel_row, section_label};
use crate::theme::{lane_color, tokens};
use gitclient_core::branch::Dragged;
use gitclient_core::git::{NamedRef, RefKind};
use gitclient_core::spotlight::Spotlight;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;

const WIDTH: Pixels = px(300.);
/// Below this the sidebar starts closed, so the graph keeps some room.
pub(super) const WIDE_WINDOW: Pixels = px(1500.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Remotes,
    Tags,
    Stashes,
}

/// A ref to place in a tree of folders.
struct Leaf<'a> {
    kind: RefKind,
    name: &'a str,
    tip: Arc<str>,
    color: Hsla,
    detail: String,
    strong: bool,
}

/// One clickable row: a branch, remote branch or tag.
struct RefRow<'a> {
    id: ElementId,
    /// What to call it in the list, which leaves out the folders it is in.
    name: &'a str,
    target: Dragged,
    tip: Arc<str>,
    color: Hsla,
    detail: String,
    on: bool,
    strong: bool,
    depth: usize,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) window_is_wide: bool,
    /// What the user chose, which outranks the window's width.
    open: Option<bool>,
    pub(super) spotlight: Option<Arc<Spotlight>>,
    expanded: Vec<Section>,
    /// Folders the user closed, as `<list>/<path>`. Folders start open.
    closed_folders: Vec<String>,
}

/// One visible row of a ref list that groups `feat/x` under a `feat` folder.
#[derive(Debug, PartialEq, Eq)]
enum TreeRow {
    Folder(Folder),
    Ref {
        /// Index into the list that was grouped.
        index: usize,
        label: String,
        depth: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct Folder {
    /// Full path, such as `feat/ui`.
    path: String,
    label: String,
    depth: usize,
    count: usize,
    open: bool,
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
fn tree_rows<'a>(
    names: impl Iterator<Item = &'a str>,
    closed: impl Fn(&str) -> bool,
) -> Vec<TreeRow> {
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
    fn walk(node: &Node, depth: usize, closed: &dyn Fn(&str) -> bool, out: &mut Vec<TreeRow>) {
        for child in &node.children {
            match child.leaf {
                Some(index) => out.push(TreeRow::Ref {
                    index,
                    label: child.label.clone(),
                    depth,
                }),
                None => {
                    let open = !closed(&child.path);
                    out.push(TreeRow::Folder(Folder {
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

/// How far in a row of the tree starts.
fn indent(depth: usize) -> Pixels {
    px(8. + 14. * depth as f32)
}

/// Adds `item` to the list, or takes it out if it was there.
fn toggle<T: PartialEq>(list: &mut Vec<T>, item: T) {
    match list.iter().position(|existing| *existing == item) {
        Some(index) => {
            list.remove(index);
        }
        None => list.push(item),
    }
}

impl GraphView {
    pub(super) fn sidebar_is_open(&self) -> bool {
        self.sidebar.open.unwrap_or(self.sidebar.window_is_wide)
    }

    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar.open = Some(!self.sidebar_is_open());
        cx.notify();
    }

    /// Fades everything outside a branch's history, or turns that off again.
    pub(super) fn spotlight(&mut self, tip: Arc<str>, cx: &mut Context<Self>) {
        let same = self
            .sidebar
            .spotlight
            .as_ref()
            .is_some_and(|s| s.tip == tip);
        self.sidebar.spotlight =
            (!same).then(|| Arc::new(Spotlight::new(tip.clone(), &self.entries)));
        if !same && let Some(row) = self.row_of(&tip) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// History changed, so the same branch's lit commits need working out again.
    pub(super) fn refresh_spotlight(&mut self) {
        if let Some(spot) = &self.sidebar.spotlight {
            let spot = Spotlight::new(spot.tip.clone(), &self.entries);
            self.sidebar.spotlight = Some(Arc::new(spot));
        }
    }

    fn toggle_folder(&mut self, key: String, cx: &mut Context<Self>) {
        toggle(&mut self.sidebar.closed_folders, key);
        cx.notify();
    }

    fn render_folder_row(
        &self,
        list: &'static str,
        folder: Folder,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Folder {
            path,
            label,
            depth,
            count,
            open,
        } = folder;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let text = theme.secondary_foreground;
        let key = format!("{list}/{path}");
        panel_row(SharedString::from(format!("folder-{key}")), false, cx)
            .h(px(30.))
            .pl(indent(depth))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(key.clone(), cx)))
            .child(
                div()
                    .w(px(8.))
                    .flex_none()
                    .text_color(muted)
                    .child(if open { "▾" } else { "▸" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(text)
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(count.to_string()),
            )
    }

    /// Rows for a list of refs, with `feat/x` shown inside a `feat` folder.
    fn render_tree(
        &self,
        list: &'static str,
        leaves: &[Leaf],
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let spotlit = self.sidebar.spotlight.clone();
        let rows = tree_rows(leaves.iter().map(|l| l.name), |path| {
            let key = format!("{list}/{path}");
            self.sidebar.closed_folders.contains(&key)
        });
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            match row {
                TreeRow::Folder(folder) => {
                    out.push(self.render_folder_row(list, folder, cx));
                }
                TreeRow::Ref {
                    index,
                    label,
                    depth,
                } => {
                    let leaf = &leaves[index];
                    let on = spotlit.as_ref().is_some_and(|s| s.tip == leaf.tip);
                    out.push(self.render_ref_row(
                        RefRow {
                            id: (list, index).into(),
                            name: &label,
                            target: Dragged {
                                kind: leaf.kind,
                                name: leaf.name.to_owned(),
                            },
                            tip: leaf.tip.clone(),
                            color: leaf.color,
                            detail: leaf.detail.clone(),
                            on,
                            strong: leaf.strong,
                            depth,
                        },
                        cx,
                    ));
                }
            }
        }
        out
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        toggle(&mut self.sidebar.expanded, section);
        cx.notify();
    }

    pub(super) fn render_sidebar_toggle(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.refs.branches.first()?;
        Some(
            Button::new("toggle-sidebar")
                .ghost()
                .small()
                .label(if self.sidebar_is_open() {
                    "Hide branches"
                } else {
                    "Branches"
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
                .into_any_element(),
        )
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.sidebar_is_open() {
            return None;
        }
        let theme = cx.theme();
        let (panel, border, muted, link) = (
            theme.title_bar,
            theme.border,
            theme.muted_foreground,
            theme.primary,
        );
        let spotlit = self.sidebar.spotlight.clone();

        let is_dark = cx.theme().is_dark();
        let default_branch = self.refs.default_branch.as_deref();
        let leaves: Vec<Leaf> = self
            .refs
            .branches
            .iter()
            .map(|branch| Leaf {
                kind: RefKind::Branch,
                name: &branch.name,
                tip: branch.tip.clone(),
                color: lane_color(branch.color, is_dark),
                detail: branch.delta(default_branch),
                strong: branch.is_head,
            })
            .collect();
        let branches = self.render_tree("branches", &leaves, cx);
        let remotes = self.render_section(Section::Remotes, "Remotes", &self.refs.remotes, cx);
        let tags = self.render_section(Section::Tags, "Tags", &self.refs.tags, cx);
        let stashes = self.render_stashes(cx);

        Some(
            div()
                .w(WIDTH)
                .flex_none()
                .bg(panel)
                .border_r_1()
                .border_color(border)
                .child(
                    div()
                        .id("sidebar")
                        .v_flex()
                        .size_full()
                        .px_2()
                        .py_3()
                        .gap_4()
                        .overflow_y_scrollbar()
                        .child(
                            div()
                                .v_flex()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .h_flex()
                                        .h(px(24.))
                                        .px_2()
                                        .child(
                                            section_label(muted)
                                                .h_flex()
                                                .flex_1()
                                                .child("Branches"),
                                        )
                                        .when(spotlit.is_some(), |heading| {
                                            let tip = spotlit.as_ref().map(|s| s.tip.clone());
                                            heading.child(
                                                div()
                                                    .id("show-all")
                                                    .cursor_pointer()
                                                    .text_size(px(12.))
                                                    .font_medium()
                                                    .text_color(link)
                                                    .child("Show all")
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            if let Some(tip) = tip.clone() {
                                                                this.spotlight(tip, cx);
                                                            }
                                                        },
                                                    )),
                                            )
                                        }),
                                )
                                .children(branches)
                                .child(
                                    div()
                                        .mt_1()
                                        .px_2()
                                        .text_size(px(12.))
                                        .text_color(muted)
                                        .child("Click a branch to trace its history."),
                                ),
                        )
                        .child(
                            div()
                                .v_flex()
                                .gap(px(2.))
                                .child(remotes)
                                .child(tags)
                                .child(stashes),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_ref_row(&self, row: RefRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let RefRow {
            id,
            name,
            target,
            tip,
            color,
            detail,
            on,
            strong,
            depth,
        } = row;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let text = if strong || on {
            theme.foreground
        } else {
            theme.secondary_foreground
        };
        let row = panel_row(id, on, cx)
            .h(px(30.))
            .pl(indent(depth))
            .on_click(cx.listener(move |this, _, _, cx| this.spotlight(tip.clone(), cx)));
        wire_ref(row, target, color, cx)
            .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(text)
                    .when(strong, |name| name.font_semibold())
                    .child(name.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(detail),
            )
    }

    fn render_section(
        &self,
        section: Section,
        title: &'static str,
        refs: &[NamedRef],
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.sidebar.expanded.contains(&section);
        let header = self.render_section_header(section, title, refs.len(), cx);
        let is_dark = cx.theme().is_dark();
        let warning = cx.theme().warning;
        let rows = if open {
            let leaves: Vec<Leaf> = refs
                .iter()
                .map(|named| Leaf {
                    kind: match section {
                        Section::Tags => RefKind::Tag,
                        _ => RefKind::Remote,
                    },
                    name: &named.name,
                    tip: named.tip.clone(),
                    color: match section {
                        Section::Tags => warning,
                        _ => lane_color(named.color, is_dark),
                    },
                    detail: String::new(),
                    strong: false,
                })
                .collect();
            self.render_tree(title, &leaves, cx)
        } else {
            Vec::new()
        };
        div().v_flex().gap(px(2.)).child(header).children(rows)
    }

    fn render_stashes(&self, cx: &mut Context<Self>) -> Div {
        let count = self.refs.stashes.len();
        let open = self.sidebar.expanded.contains(&Section::Stashes);
        let header = self.render_section_header(Section::Stashes, "Stashes", count, cx);
        let subtext = tokens(cx.theme().is_dark()).subtext0;
        let rows = self.refs.stashes.iter().filter(|_| open).map(|stash| {
            div()
                .h_flex()
                .h(px(30.))
                .px_2()
                .text_color(subtext)
                .child(div().min_w_0().truncate().child(stash.clone()))
        });
        div().v_flex().gap(px(2.)).child(header).children(rows)
    }

    fn render_section_header(
        &self,
        section: Section,
        title: &'static str,
        count: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        panel_row(title, false, cx)
            .h(px(30.))
            .justify_between()
            .text_color(tokens(theme.is_dark()).subtext0)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(title)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(count.to_string()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Folder, TreeRow, tree_rows};

    fn rows(names: &[&str], closed: &[&str]) -> Vec<String> {
        tree_rows(names.iter().copied(), |p| closed.contains(&p))
            .into_iter()
            .map(|row| match row {
                TreeRow::Folder(Folder {
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
                TreeRow::Ref { label, depth, .. } => format!("{}{label}", " ".repeat(depth)),
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
