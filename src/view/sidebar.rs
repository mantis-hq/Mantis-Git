//! The branch sidebar. Clicking a branch spotlights its history in the graph.

use super::menu::wire_ref;
use super::parts::{TEXT_CHIP, TEXT_SMALL};
use super::parts::{panel_row, section_label};
use super::{GraphView, SwitchWorktree};
use crate::theme::{lane_color, tokens};
use gitclient_core::branch::Dragged;
use gitclient_core::git::{NamedRef, RefKind};
use gitclient_core::picks::{Click, Picks, tip_of};
use gitclient_core::spotlight::Spotlight;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

const WIDTH: Pixels = px(300.);
/// Below this the sidebar starts closed, so the graph keeps some room.
pub(super) const WIDE_WINDOW: Pixels = px(1500.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Worktrees,
    Remotes,
    Tags,
    Stashes,
}

/// A ref to place in a tree of folders.
struct Leaf<'a> {
    kind: RefKind,
    name: &'a str,
    color: Hsla,
    detail: String,
    /// How it stands against its remote branch, such as `↑1 ↓2`.
    sync: String,
    strong: bool,
}

/// One clickable row: a branch, remote branch or tag.
struct RefRow<'a> {
    id: ElementId,
    /// What to call it in the list, which leaves out the folders it is in.
    name: &'a str,
    target: Dragged,
    /// Every ref row shown in this list, which a shift-click runs along.
    visible: Rc<[Dragged]>,
    color: Hsla,
    detail: String,
    sync: String,
    on: bool,
    strong: bool,
    depth: usize,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) window_is_wide: bool,
    /// What the user chose, which outranks the window's width.
    open: Option<bool>,
    /// The width the user dragged the panel to.
    pub(super) width: Option<f32>,
    pub(super) spotlight: Option<Arc<Spotlight>>,
    /// The refs picked in the list, which `spotlight` is worked out from.
    pub(super) picks: Picks,
    expanded: Vec<Section>,
    /// Folders the user closed, as `<list>/<path>`. Folders start open.
    closed_folders: Vec<String>,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            // Worktrees are few and each is a place to go, so they start shown.
            expanded: vec![Section::Worktrees],
            ..Self::default()
        }
    }
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

    /// Switches to another working tree, which the window's tabs take care of.
    fn switch_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(SwitchWorktree(path));
    }

    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar.open = Some(!self.sidebar_is_open());
        cx.notify();
    }

    /// A click on a branch, tag or remote branch. Plain picks it alone, `Ctrl` adds or
    /// removes it and `Shift` picks everything down from the last one. The graph fades
    /// everything outside the picked histories.
    pub(super) fn pick(
        &mut self,
        target: &Dragged,
        modifiers: Modifiers,
        visible: &[Dragged],
        cx: &mut Context<Self>,
    ) {
        let click = if modifiers.shift {
            Click::Range
        } else if modifiers.secondary(cx) {
            Click::Add
        } else {
            Click::Plain
        };
        self.sidebar.picks.click(target, click, visible);
        self.update_spotlight();
        let tip = self
            .sidebar
            .picks
            .contains(target)
            .then(|| tip_of(&self.refs, target));
        if let Some(Some(tip)) = tip
            && let Some(row) = self.row_of(&tip)
        {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// Lights what the picked refs point at now.
    fn update_spotlight(&mut self) {
        let tips = self.sidebar.picks.tips(&self.refs);
        self.sidebar.spotlight =
            (!tips.is_empty()).then(|| Arc::new(Spotlight::new(tips, &self.entries)));
    }

    /// Lets go of every pick, as "Show all" does.
    fn clear_picks(&mut self, cx: &mut Context<Self>) {
        self.sidebar.picks.clear();
        self.sidebar.spotlight = None;
        cx.notify();
    }

    /// Refs or history changed, so the picks and their lit commits need working out again.
    pub(super) fn refresh_spotlight(&mut self) {
        self.sidebar.picks.retain_existing(&self.refs);
        self.update_spotlight();
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
            .gap_1()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(key.clone(), cx)))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .text_size(px(15.))
                    .line_height(px(16.))
                    .text_center()
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
                    .text_size(TEXT_CHIP)
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
        let rows = tree_rows(leaves.iter().map(|l| l.name), |path| {
            let key = format!("{list}/{path}");
            self.sidebar.closed_folders.contains(&key)
        });
        let visible: Rc<[Dragged]> = rows
            .iter()
            .filter_map(|row| match row {
                TreeRow::Ref { index, .. } => Some(Dragged {
                    kind: leaves[*index].kind,
                    name: leaves[*index].name.to_owned(),
                }),
                TreeRow::Folder(_) => None,
            })
            .collect();
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
                    let target = Dragged {
                        kind: leaf.kind,
                        name: leaf.name.to_owned(),
                    };
                    let on = self.sidebar.picks.contains(&target);
                    out.push(self.render_ref_row(
                        RefRow {
                            id: (list, index).into(),
                            name: &label,
                            target,
                            visible: visible.clone(),
                            color: leaf.color,
                            detail: leaf.detail.clone(),
                            sync: leaf.sync.clone(),
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
                color: lane_color(branch.color, is_dark),
                detail: branch.delta(default_branch),
                sync: branch.remote_delta(),
                strong: branch.is_head,
            })
            .collect();
        let branches = self.render_tree("branches", &leaves, cx);
        let worktrees = self.render_worktrees(cx);
        let remotes = self.render_section(Section::Remotes, "Remotes", &self.refs.remotes, cx);
        let tags = self.render_section(Section::Tags, "Tags", &self.refs.tags, cx);
        let stashes = self.render_stashes(cx);

        Some(
            div()
                .w(self.sidebar.width.map_or(WIDTH, px))
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
                                            heading.child(
                                                div()
                                                    .id("show-all")
                                                    .cursor_pointer()
                                                    .text_size(TEXT_SMALL)
                                                    .font_medium()
                                                    .text_color(link)
                                                    .child("Show all")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.clear_picks(cx)
                                                    })),
                                            )
                                        }),
                                )
                                .children(branches),
                        )
                        .child(
                            div()
                                .v_flex()
                                .gap(px(2.))
                                .child(worktrees)
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
            visible,
            color,
            detail,
            sync,
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
            .on_click({
                let target = target.clone();
                cx.listener(move |this, event: &ClickEvent, _, cx| {
                    this.pick(&target, event.modifiers(), &visible, cx);
                    this.open_on_double_click(&target, event, cx);
                })
            });
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
                    .text_size(TEXT_CHIP)
                    .text_color(muted)
                    .child(detail),
            )
            .when(!sync.is_empty(), |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child(sync),
                )
            })
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
                    color: match section {
                        Section::Tags => warning,
                        _ => lane_color(named.color, is_dark),
                    },
                    detail: String::new(),
                    sync: String::new(),
                    strong: false,
                })
                .collect();
            self.render_tree(title, &leaves, cx)
        } else {
            Vec::new()
        };
        div().v_flex().gap(px(2.)).child(header).children(rows)
    }

    /// The repository's working trees, when it has more than the one. Picking another
    /// opens it, in the tab already on it or a new one.
    fn render_worktrees(&self, cx: &mut Context<Self>) -> Div {
        let trees = &self.refs.worktrees;
        if trees.len() < 2 {
            return div();
        }
        let open = self.sidebar.expanded.contains(&Section::Worktrees);
        let header = self.render_section_header(Section::Worktrees, "Worktrees", trees.len(), cx);
        let is_dark = cx.theme().is_dark();
        let theme = cx.theme();
        let (muted, text, strong) = (
            theme.muted_foreground,
            theme.secondary_foreground,
            theme.foreground,
        );
        let rows = trees
            .iter()
            .enumerate()
            .filter(|_| open)
            .map(|(index, tree)| {
                let color = match &tree.branch {
                    Some(branch) => lane_color(self.refs.branch_color(branch), is_dark),
                    None => muted,
                };
                let unavailable = tree.unavailable();
                let path = tree.path.clone();
                let mut detail = tree.detail();
                if tree.locked {
                    detail.push_str(" · locked");
                }
                let menu_path = tree.path.clone();
                let row =
                    panel_row(("worktree", index), tree.is_current, cx)
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_worktree_menu(&menu_path, event.position, cx);
                            }),
                        )
                        .h(px(30.))
                        .pl(indent(0))
                        .when(unavailable.is_some(), |row| {
                            row.opacity(0.5).cursor_default()
                        })
                        .when(!tree.is_current && unavailable.is_none(), |row| {
                            row.on_click(cx.listener(move |this, _, _, cx| {
                                this.switch_worktree(path.clone(), cx)
                            }))
                        });
                row.child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if tree.is_current { strong } else { text })
                            .when(tree.is_current, |name| name.font_semibold())
                            .child(tree.label()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(unavailable.map_or(detail, str::to_owned)),
                    )
            });
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
                    .text_size(TEXT_CHIP)
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
