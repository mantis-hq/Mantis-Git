//! The branch sidebar. Clicking a branch spotlights its history in the graph.

pub(super) mod remotes;

use super::menu::wire_ref;
use super::parts::{TEXT_CHIP, TEXT_SMALL};
use super::parts::{panel_row, section_label};
use super::{GraphView, OpenWorktreeInTab, SwitchWorktree};
use crate::theme::{lane_color, tokens};
use gitclient_core::history::picks::{Click, Picks, tip_of};
use gitclient_core::history::refs::RefKind;
use gitclient_core::history::spotlight::Spotlight;
use gitclient_core::menu::form::Form;
use gitclient_core::repo::branch::Dragged;
use gitclient_core::repo::submodule::{self, State as SubmoduleState};
use gitclient_core::tree::branches::{self, Folder};
use gpui_kit::component::button::*;
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
    Submodules,
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
    /// The "Branches" label and the "New branch" link.
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
    /// A remote that has no remote branches yet, such as one just added. Index into
    /// `remote_configs`.
    Remote(usize),
    Stash(usize),
}

/// Every row is this tall: a 30 px row and the 2 px gap under it.
const ITEM_HEIGHT: Pixels = px(32.);
/// The pinned submodules show this many rows at most, and scroll past that, so the
/// branches above keep their room.
const PINNED_ROWS: usize = 5;

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
    scroll: UniformListScrollHandle,
    submodule_scroll: UniformListScrollHandle,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            // Worktrees and submodules are few and each is a place to go, so they start shown.
            expanded: vec![Section::Worktrees, Section::Submodules],
            ..Self::default()
        }
    }
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
    pub fn sidebar_is_open(&self) -> bool {
        self.sidebar.open.unwrap_or(self.sidebar.window_is_wide)
    }

    /// Switches to another working tree, which the window's tabs take care of.
    fn switch_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(SwitchWorktree(path));
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
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
        // The top folder of the remote branches is the remote itself.
        let remote = (list == RefList::Remotes.key() && depth == 0 && self.is_remote(&path))
            .then(|| path.clone());
        panel_row(SharedString::from(format!("folder-{key}")), false, cx)
            .when_some(remote, |row, name| {
                row.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_remote_menu(&name, event.position, cx);
                    }),
                )
            })
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
        let rows = branches::rows(names, |path| {
            let key = format!("{}/{path}", list.key());
            self.sidebar.closed_folders.contains(&key)
        });
        items.extend(rows.into_iter().map(|row| match row {
            branches::Row::Folder(folder) => Item::Folder { list, folder },
            branches::Row::Ref {
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

        self.push_ref_section(
            Section::Remotes,
            "Remotes",
            RefList::Remotes,
            &self.refs.remotes,
            &mut items,
        );

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

        self.push_ref_section(
            Section::Tags,
            "Tags",
            RefList::Tags,
            &self.refs.tags,
            &mut items,
        );

        let stashes = self.refs.stashes.len();
        items.push(Item::Header {
            section: Section::Stashes,
            title: "Stashes",
            count: stashes,
        });
        if self.sidebar.expanded.contains(&Section::Stashes) {
            items.extend((0..stashes).map(Item::Stash));
        }
        // Submodules are pinned to the foot of the panel, apart from this list.
        items
    }

    /// A header and, when open, the folder tree of the refs under it.
    fn push_ref_section(
        &self,
        section: Section,
        title: &'static str,
        list: RefList,
        refs: &[gitclient_core::history::refs::NamedRef],
        items: &mut Vec<Item>,
    ) {
        items.push(Item::Header {
            section,
            title,
            count: refs.len(),
        });
        if self.sidebar.expanded.contains(&section) {
            self.push_tree(list, refs.iter().map(|named| &*named.name), items);
            if section == Section::Remotes {
                items.extend(self.remotes_without_branches().map(Item::Remote));
            }
        }
    }

    /// The remotes that no remote branch shows, until the first fetch brings some.
    fn remotes_without_branches(&self) -> impl Iterator<Item = usize> + '_ {
        let has_branch = |name: &str| {
            let prefix = format!("{name}/");
            self.refs
                .remotes
                .iter()
                .any(|branch| branch.name.starts_with(&prefix))
        };
        self.refs
            .remote_configs
            .iter()
            .enumerate()
            .filter(move |(_, remote)| !has_branch(&remote.name))
            .map(|(index, _)| index)
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
            Item::Remote(index) => match self.render_remote_row(*index, cx) {
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

        Some(
            div()
                .v_flex()
                .w(self.sidebar.width.map_or(WIDTH, px))
                .flex_none()
                .bg(panel)
                .border_r_1()
                .border_color(border)
                .child(
                    div()
                        .id("sidebar")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .px_2()
                        .pt_3()
                        .pb_2()
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
                .child(self.render_submodules(cx))
                .into_any_element(),
        )
    }

    /// The submodules, pinned to the foot of the panel under a border so they stay in
    /// view however long the branches run. The header is always there, with "+ Add", and
    /// says so in a sentence when the repository has none.
    fn render_submodules(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let section = Section::Submodules;
        let modules = self.refs.submodules.len();
        let body = self.sidebar.expanded.contains(&section).then(|| {
            if modules == 0 {
                return div()
                    .h_flex()
                    .h(px(30.))
                    .px_2()
                    .text_color(muted)
                    .child(div().min_w_0().truncate().child(submodule::NONE))
                    .into_any_element();
            }
            let shown = modules.min(PINNED_ROWS);
            div()
                .relative()
                .w_full()
                .h(ITEM_HEIGHT * shown as f32)
                .child(
                    uniform_list(
                        "sidebar-submodules",
                        modules,
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let slot = div().h(ITEM_HEIGHT).flex_none();
                                    match this.render_submodule_row(index, cx) {
                                        Some(row) => slot.child(row),
                                        None => slot,
                                    }
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.sidebar.submodule_scroll)
                    .size_full(),
                )
                .when(modules > PINNED_ROWS, |list| {
                    list.scrollbar(&self.sidebar.submodule_scroll, ScrollbarAxis::Vertical)
                })
                .into_any_element()
        });
        div()
            .flex_none()
            .w_full()
            .px_2()
            .pt_2()
            .pb_3()
            .border_t_1()
            .border_color(border)
            .child(self.render_section_header(section, "Submodules", modules, cx))
            .children(body)
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
                        this.open_form(Form::new_branch_here(&this.refs), cx)
                    })),
            )
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

    /// One of the repository's submodules. Picking one opens it in a tab, when it has
    /// been downloaded.
    fn render_submodule_row(&self, index: usize, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let module = self.refs.submodules.get(index)?;
        let theme = cx.theme();
        let (muted, text) = (theme.muted_foreground, theme.secondary_foreground);
        // The words say the same as the dot, since a colour alone carries no meaning.
        let color = match module.state {
            SubmoduleState::Current => theme.success,
            SubmoduleState::Moved => theme.warning,
            SubmoduleState::Conflict => tokens(theme.is_dark()).peach,
            SubmoduleState::NotDownloaded => muted,
        };
        let (folder, menu_path) = (module.folder.clone(), module.path.clone());
        let row = panel_row(("submodule", index), false, cx)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_submodule_menu(&menu_path, event.position, cx);
                }),
            )
            .h(px(30.))
            .pl(indent(0))
            .when(!module.is_downloaded(), |row| row.cursor_default())
            .when(module.is_downloaded(), |row| {
                row.on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(OpenWorktreeInTab(folder.clone()));
                }))
            });
        Some(
            row.child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(text)
                        .child(module.label().to_owned()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child(module.detail()),
                ),
        )
    }

    /// A remote with nothing fetched from it yet. It is what there is to right-click.
    fn render_remote_row(&self, index: usize, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let remote = self.refs.remote_configs.get(index)?;
        let theme = cx.theme();
        let (muted, text) = (theme.muted_foreground, theme.secondary_foreground);
        let name = remote.name.clone();
        let menu_name = remote.name.clone();
        Some(
            panel_row(("remote", index), false, cx)
                .h(px(30.))
                .pl(indent(0))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_remote_menu(&menu_name, event.position, cx);
                    }),
                )
                .child(div().size(px(8.)).flex_none().rounded_full().bg(muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(text)
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child("no branches yet"),
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
        let link = theme.primary;
        let add = match section {
            Section::Remotes => Some(("add-remote", Form::Remote { original: None })),
            Section::Submodules => Some(("add-submodule", Form::Submodule { original: None })),
            _ => None,
        };
        panel_row(title, false, cx)
            .h(px(30.))
            .justify_between()
            .text_color(tokens(theme.is_dark()).subtext0)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(div().flex_1().child(title))
            .when_some(add, |row, (id, form)| {
                row.child(
                    div()
                        .id(id)
                        .flex_none()
                        .px_1()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(link)
                        .font_semibold()
                        .child("+ Add")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The header under it would otherwise fold the section.
                            cx.stop_propagation();
                            this.open_form(form.clone(), cx);
                        })),
                )
            })
            .child(
                div()
                    .text_size(TEXT_CHIP)
                    .text_color(theme.muted_foreground)
                    .child(count.to_string()),
            )
    }
}
