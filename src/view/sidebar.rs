//! The branch sidebar. Clicking a branch spotlights its history in the graph.

use super::menu::wire_ref;
use super::parts::{TEXT_CHIP, TEXT_SMALL};
use super::parts::{panel_row, section_label};
use super::{GraphView, SwitchWorktree};
use crate::theme::{lane_color, tokens};
use gitclient_core::branch;
use gitclient_core::branch::Dragged;
use gitclient_core::git::RefKind;
use gitclient_core::picks::{Click, Picks, tip_of};
use gitclient_core::spotlight::Spotlight;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::path::PathBuf;
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

/// The lists of refs, which group `feat/x` under a `feat` folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefList {
    Branches,
    Remotes,
    Tags,
}

impl RefList {
    /// Names the list in element ids and in the keys of closed folders.
    fn key(self) -> &'static str {
        match self {
            Self::Branches => "branches",
            Self::Remotes => "Remotes",
            Self::Tags => "Tags",
        }
    }
}

/// One row of the sidebar, as data. The sidebar is a single flat list of these, so that
/// only the rows in view become elements however many refs the repository has.
enum Item {
    /// The "Branches" label and, while refs are picked, "Show all".
    Heading,
    /// Space between the branches and the other sections.
    Gap,
    Header {
        section: Section,
        title: &'static str,
        count: usize,
    },
    Folder {
        list: RefList,
        folder: Folder,
    },
    Ref {
        list: RefList,
        /// Index into the list that was grouped.
        index: usize,
        /// What to call it, which leaves out the folders it is in.
        label: String,
        depth: usize,
    },
    Worktree(usize),
    Stash(usize),
}

/// Every row is this tall: a 30 px row and the 2 px gap under it.
const ITEM_HEIGHT: Pixels = px(32.);

/// What a ref row needs to be drawn.
struct RefRow {
    id: ElementId,
    name: String,
    target: Dragged,
    list: RefList,
    color: Hsla,
    detail: String,
    sync: String,
    on: bool,
    strong: bool,
    depth: usize,
}

/// The form that asks for a new branch's name.
pub(super) struct NewBranch {
    /// Where the branch starts, or the checked-out commit when `None`.
    start: Option<String>,
    /// What to call that place in the form.
    label: String,
    /// Made on the next render, which is the first time there is a window to make it in.
    input: Option<Entity<InputState>>,
    _events: Option<Subscription>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) new_branch: Option<NewBranch>,
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
    scroll: UniformListScrollHandle,
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

    /// Opens the form for a new branch, showing the sidebar if it is closed.
    pub(super) fn begin_new_branch(
        &mut self,
        start: Option<String>,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.open = Some(true);
        self.sidebar.new_branch = Some(NewBranch {
            start,
            label,
            input: None,
            _events: None,
        });
        cx.notify();
    }

    fn cancel_new_branch(&mut self, cx: &mut Context<Self>) {
        self.sidebar.new_branch = None;
        cx.notify();
    }

    /// Makes the input of a form that was just opened, and puts the cursor in it.
    pub(super) fn ensure_new_branch_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .sidebar
            .new_branch
            .as_ref()
            .is_some_and(|form| form.input.is_none())
        {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("feat/my-change"));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.create_new_branch(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        if let Some(form) = &mut self.sidebar.new_branch {
            form.input = Some(input);
            form._events = Some(events);
        }
    }

    fn new_branch_name(&self, cx: &App) -> String {
        self.sidebar
            .new_branch
            .as_ref()
            .and_then(|form| form.input.as_ref())
            .map_or_else(String::new, |input| {
                input.read(cx).value().trim().to_owned()
            })
    }

    fn create_new_branch(&mut self, cx: &mut Context<Self>) {
        let name = self.new_branch_name(cx);
        if name.is_empty() || self.busy.is_some() {
            return;
        }
        let Some(form) = self.sidebar.new_branch.take() else {
            return;
        };
        self.run_operation(
            "Creating a branch",
            move |repo| branch::create_branch(repo, &name, form.start.as_deref()),
            cx,
        );
    }

    fn render_new_branch(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.sidebar.new_branch.as_ref()?;
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let name = self.new_branch_name(cx);
        let disabled = name.is_empty() || self.busy.is_some();
        Some(
            div()
                .v_flex()
                .gap_2()
                .px_2()
                .pb_3()
                .mb_2()
                .border_b_1()
                .border_color(border)
                .child(section_label(muted).child(branch::new_branch_prompt(&form.label)))
                .children(form.input.as_ref().map(Input::new))
                .child(
                    div()
                        .h_flex()
                        .justify_end()
                        .gap_1()
                        .child(
                            Button::new("new-branch-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_new_branch(cx))),
                        )
                        .child(
                            Button::new("new-branch-create")
                                .ghost()
                                .small()
                                .label(branch::new_branch_label(&name))
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, cx| this.create_new_branch(cx))),
                        ),
                )
                .into_any_element(),
        )
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

    /// Adds a list of refs to `items`, with `feat/x` inside a `feat` folder.
    fn push_tree<'a>(
        &self,
        list: RefList,
        names: impl Iterator<Item = &'a str>,
        items: &mut Vec<Item>,
    ) {
        let rows = tree_rows(names, |path| {
            let key = format!("{}/{path}", list.key());
            self.sidebar.closed_folders.contains(&key)
        });
        items.extend(rows.into_iter().map(|row| match row {
            TreeRow::Folder(folder) => Item::Folder { list, folder },
            TreeRow::Ref {
                index,
                label,
                depth,
            } => Item::Ref {
                list,
                index,
                label,
                depth,
            },
        }));
    }

    /// Every row of the sidebar, top to bottom.
    fn sidebar_items(&self) -> Vec<Item> {
        let mut items = vec![Item::Heading];
        let branches = self.refs.branches.iter().map(|b| &*b.name);
        self.push_tree(RefList::Branches, branches, &mut items);
        items.push(Item::Gap);

        let trees = self.refs.worktrees.len();
        if trees >= 2 {
            let section = Section::Worktrees;
            items.push(Item::Header {
                section,
                title: "Worktrees",
                count: trees,
            });
            if self.sidebar.expanded.contains(&section) {
                items.extend((0..trees).map(Item::Worktree));
            }
        }

        let sections = [
            (
                Section::Remotes,
                "Remotes",
                RefList::Remotes,
                &self.refs.remotes,
            ),
            (Section::Tags, "Tags", RefList::Tags, &self.refs.tags),
        ];
        for (section, title, list, refs) in sections {
            items.push(Item::Header {
                section,
                title,
                count: refs.len(),
            });
            if self.sidebar.expanded.contains(&section) {
                self.push_tree(list, refs.iter().map(|named| &*named.name), &mut items);
            }
        }

        let stashes = self.refs.stashes.len();
        items.push(Item::Header {
            section: Section::Stashes,
            title: "Stashes",
            count: stashes,
        });
        if self.sidebar.expanded.contains(&Section::Stashes) {
            items.extend((0..stashes).map(Item::Stash));
        }
        items
    }

    /// The ref at `index` in a list.
    fn ref_target(&self, list: RefList, index: usize) -> Option<Dragged> {
        let (kind, name) = match list {
            RefList::Branches => (RefKind::Branch, &self.refs.branches.get(index)?.name),
            RefList::Remotes => (RefKind::Remote, &self.refs.remotes.get(index)?.name),
            RefList::Tags => (RefKind::Tag, &self.refs.tags.get(index)?.name),
        };
        Some(Dragged {
            kind,
            name: name.to_string(),
        })
    }

    /// Every ref row shown in a list, which a shift-click runs along.
    fn visible_refs(&self, list: RefList) -> Vec<Dragged> {
        self.sidebar_items()
            .iter()
            .filter_map(|item| match item {
                Item::Ref {
                    list: of, index, ..
                } if *of == list => self.ref_target(list, *index),
                _ => None,
            })
            .collect()
    }

    fn render_item(&self, item: &Item, cx: &mut Context<Self>) -> Div {
        let slot = div().h(ITEM_HEIGHT).flex_none();
        match item {
            Item::Heading => slot.child(self.render_heading(cx)),
            Item::Gap => slot,
            Item::Header {
                section,
                title,
                count,
            } => slot.child(self.render_section_header(*section, title, *count, cx)),
            Item::Folder { list, folder } => {
                let folder = Folder {
                    path: folder.path.clone(),
                    label: folder.label.clone(),
                    ..*folder
                };
                slot.child(self.render_folder_row(list.key(), folder, cx))
            }
            Item::Ref {
                list,
                index,
                label,
                depth,
            } => match self.ref_row(*list, *index, label, *depth, cx) {
                Some(row) => slot.child(self.render_ref_row(row, cx)),
                None => slot,
            },
            Item::Worktree(index) => match self.render_worktree_row(*index, cx) {
                Some(row) => slot.child(row),
                None => slot,
            },
            Item::Stash(index) => {
                let subtext = tokens(cx.theme().is_dark()).subtext0;
                let stash = self.refs.stashes.get(*index).cloned().unwrap_or_default();
                slot.child(
                    div()
                        .h_flex()
                        .h(px(30.))
                        .px_2()
                        .text_color(subtext)
                        .child(div().min_w_0().truncate().child(stash)),
                )
            }
        }
    }

    fn ref_row(
        &self,
        list: RefList,
        index: usize,
        label: &str,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> Option<RefRow> {
        let target = self.ref_target(list, index)?;
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (color, detail, sync, strong) = match list {
            RefList::Branches => {
                let branch = self.refs.branches.get(index)?;
                (
                    lane_color(branch.color, is_dark),
                    branch.delta(self.refs.default_branch.as_deref()),
                    branch.remote_delta(),
                    branch.is_head,
                )
            }
            RefList::Remotes => (
                lane_color(self.refs.remotes.get(index)?.color, is_dark),
                String::new(),
                String::new(),
                false,
            ),
            RefList::Tags => (theme.warning, String::new(), String::new(), false),
        };
        Some(RefRow {
            id: (list.key(), index).into(),
            name: label.to_owned(),
            on: self.sidebar.picks.contains(&target),
            target,
            list,
            color,
            detail,
            sync,
            strong,
            depth,
        })
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
        let (panel, border) = (theme.title_bar, theme.border);
        let items = self.sidebar_items();
        let form = self.render_new_branch(cx);

        Some(
            div()
                .v_flex()
                .w(self.sidebar.width.map_or(WIDTH, px))
                .flex_none()
                .bg(panel)
                .border_r_1()
                .border_color(border)
                .child(div().v_flex().px_2().pt_3().children(form))
                .child(
                    div()
                        .id("sidebar")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .px_2()
                        .pb_3()
                        .child(
                            div()
                                .relative()
                                .size_full()
                                .child(
                                    uniform_list(
                                        "sidebar-rows",
                                        items.len(),
                                        cx.processor(move |this, range: Range<usize>, _, cx| {
                                            range
                                                .filter_map(|index| items.get(index))
                                                .map(|item| this.render_item(item, cx))
                                                .collect::<Vec<_>>()
                                        }),
                                    )
                                    .track_scroll(&self.sidebar.scroll)
                                    .size_full(),
                                )
                                .scrollbar(&self.sidebar.scroll, ScrollbarAxis::Vertical),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_heading(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (muted, link) = (theme.muted_foreground, theme.primary);
        div()
            .h_flex()
            .h(px(24.))
            .px_2()
            .child(section_label(muted).h_flex().flex_1().child("Branches"))
            .child(
                div()
                    .id("new-branch")
                    .cursor_pointer()
                    .text_size(TEXT_SMALL)
                    .font_medium()
                    .text_color(link)
                    .child("New branch")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let label = this.refs.head_name().to_owned();
                        this.begin_new_branch(None, label, cx)
                    })),
            )
            .when(self.sidebar.spotlight.is_some(), |heading| {
                heading.child(
                    div()
                        .id("show-all")
                        .cursor_pointer()
                        .text_size(TEXT_SMALL)
                        .font_medium()
                        .text_color(link)
                        .child("Show all")
                        .on_click(cx.listener(|this, _, _, cx| this.clear_picks(cx))),
                )
            })
    }

    fn render_ref_row(&self, row: RefRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let RefRow {
            id,
            name,
            target,
            list,
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
                    let visible = this.visible_refs(list);
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
                    .child(name),
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

    /// One of the repository's working trees. Picking another opens it, in the tab
    /// already on it or a new one.
    fn render_worktree_row(&self, index: usize, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let tree = self.refs.worktrees.get(index)?;
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (muted, text, strong) = (
            theme.muted_foreground,
            theme.secondary_foreground,
            theme.foreground,
        );
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
        let row = panel_row(("worktree", index), tree.is_current, cx)
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
                row.on_click(
                    cx.listener(move |this, _, _, cx| this.switch_worktree(path.clone(), cx)),
                )
            });
        Some(
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
                ),
        )
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
