//! Context menus and drag and drop: drawing the lines core asks for, and running the one
//! that is chosen. What a menu offers, and what each line does, is decided in
//! `mantisgit_core::menu`.

use super::dialog::Confirm;
use super::notice::Notice;
use super::parts::{TEXT_BODY, TEXT_CHIP, TEXT_SMALL};
use super::{GraphView, OpenWorktreeInTab, SwitchWorktree};
use crate::icons::Icon;
use crate::theme::{selected_row, tokens};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mantisgit_core::history::refs::RefKind;
use mantisgit_core::menu::command::{Command, Weight};
use mantisgit_core::menu::{self, FileSource, Glyph, Line};
use mantisgit_core::repo::branch::{self, Dragged, Relation};
use mantisgit_core::repo::forge;
use mantisgit_core::working::Side;
use std::path::Path;

const MENU_WIDTH: Pixels = px(300.);
const MENU_MAX_WIDTH: Pixels = px(460.);
const ITEM_HEIGHT: Pixels = px(28.);
/// Keeps the menu off the window's edge.
const EDGE_MARGIN: Pixels = px(8.);

pub(super) struct Menu {
    position: Point<Pixels>,
    lines: Vec<Line>,
    /// The line the keyboard is on.
    highlighted: Option<usize>,
    /// The submenu that is out beside its line, if one is.
    sub: Option<Sub>,
}

/// A menu that pops out to the right of the line that opens it.
struct Sub {
    /// The line of the main menu that opened it.
    at: usize,
    lines: Vec<Line>,
    highlighted: Option<usize>,
    /// Whether the keyboard is in it. One the pointer opened leaves the keys on the main menu.
    keys: bool,
}

impl Menu {
    pub(super) fn new(position: Point<Pixels>, lines: Vec<Line>) -> Self {
        Self {
            position,
            lines,
            highlighted: None,
            sub: None,
        }
    }

    /// Pops the submenu of line `at` out, if that line has one. `keys` puts the keyboard in it.
    fn open_sub(&mut self, at: usize, keys: bool) {
        if self.sub.as_ref().is_some_and(|sub| sub.at == at) {
            if let Some(sub) = &mut self.sub {
                sub.keys |= keys;
            }
        } else {
            self.sub = self
                .lines
                .get(at)
                .and_then(Line::submenu_lines)
                .map(|lines| Sub {
                    at,
                    lines: lines.to_vec(),
                    highlighted: None,
                    keys,
                });
        }
        if let Some(sub) = self.sub.as_mut().filter(|sub| sub.keys) {
            self.highlighted = Some(sub.at);
            if sub.highlighted.is_none() {
                sub.highlighted = first_choice(&sub.lines);
            }
        }
    }

    /// Takes the submenu back in. False when none was out.
    fn close_sub(&mut self) -> bool {
        self.sub.take().is_some()
    }

    /// Moves the keyboard highlight to the next or previous line with a command, wrapping.
    fn step(&mut self, delta: isize) {
        if let Some(sub) = self.sub.as_mut().filter(|sub| sub.keys) {
            step_in(&sub.lines, &mut sub.highlighted, delta);
        } else {
            self.sub = None;
            step_in(&self.lines, &mut self.highlighted, delta);
        }
    }

    /// The line a key press would choose.
    fn current(&self) -> Option<&Line> {
        match self.sub.as_ref().filter(|sub| sub.keys) {
            Some(sub) => sub.lines.get(sub.highlighted?),
            None => self.lines.get(self.highlighted?),
        }
    }

    #[cfg(test)]
    pub(super) fn lines(&self) -> &[Line] {
        &self.lines
    }

    #[cfg(test)]
    pub(super) fn highlighted_label(&self) -> Option<String> {
        match self.current()? {
            Line::Item { label, .. } => Some(label.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn has_submenu_out(&self) -> bool {
        self.sub.is_some()
    }
}

/// The first line that does something.
fn first_choice(lines: &[Line]) -> Option<usize> {
    lines.iter().position(|line| line.command().is_some())
}

/// Moves `highlighted` to the next or previous line that does something, wrapping.
fn step_in(lines: &[Line], highlighted: &mut Option<usize>, delta: isize) {
    let len = lines.len() as isize;
    let mut at = highlighted.map_or(if delta > 0 { -1 } else { len }, |index| index as isize);
    for _ in 0..len {
        at = (at + delta).rem_euclid(len);
        if lines[at as usize].command().is_some() {
            *highlighted = Some(at as usize);
            return;
        }
    }
}

/// What the cursor carries while a branch or tag is being dragged.
struct DragGhost {
    name: SharedString,
    color: Hsla,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The chip is translucent, so it sits on a solid fill to stay readable over rows.
        div().rounded(px(4.)).bg(cx.theme().title_bar).child(
            div()
                .h(px(20.))
                .h_flex()
                .px_1p5()
                .rounded(px(4.))
                .text_size(TEXT_CHIP)
                .font_semibold()
                .text_color(self.color)
                .bg(self.color.opacity(0.14))
                .child(self.name.clone()),
        )
    }
}

/// The picture a line starts with. A forge's mark takes the line's own colour, and the
/// question takes Peach, the colour of what needs a decision.
fn glyph_icon(glyph: Glyph, color: Hsla, peach: Hsla) -> impl IntoElement {
    let (icon, color) = match glyph {
        Glyph::GitHub => (Icon::GitHub, color),
        Glyph::GitLab => (Icon::GitLab, color),
        Glyph::Gitea => (Icon::Gitea, color),
        Glyph::Unknown => (Icon::Question, peach),
    };
    svg()
        .path(icon.path())
        .size(px(14.))
        .flex_none()
        .text_color(color)
}

/// Makes a chip or sidebar row of a ref something to right-click and drag, and, for a local
/// branch, something to drop on. A chip in the graph passes the `row` of its commit, so
/// right-clicking it opens that commit's menu, the same as right-clicking the row.
pub(super) fn wire_ref(
    element: Stateful<Div>,
    target: Dragged,
    color: Hsla,
    row: Option<usize>,
    cx: &mut Context<GraphView>,
) -> Stateful<Div> {
    let menu_target = target.clone();
    let element = element
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                // A row under a chip would otherwise open its own menu on top.
                cx.stop_propagation();
                match row {
                    Some(row) => this.open_commit_menu(row, event.position, cx),
                    None => this.open_ref_menu(menu_target.clone(), event.position, cx),
                }
            }),
        )
        .on_drag(target.clone(), move |dragged, _, _, cx| {
            let name = SharedString::from(dragged.name.clone());
            cx.new(|_| DragGhost { name, color })
        });
    // A branch takes anything. A remote branch takes another remote branch, since what is
    // proposed is a request on the forge, which only two remote branches can make.
    if !matches!(target.kind, RefKind::Branch | RefKind::Remote) {
        return element;
    }
    element
        .can_drop({
            let target = target.clone();
            move |dragged, _, _| {
                dragged.downcast_ref::<Dragged>().is_some_and(|source| {
                    source.name != target.name
                        && (target.kind == RefKind::Branch
                            || matches!(source.kind, RefKind::Remote | RefKind::Branch))
                })
            }
        })
        .drag_over::<Dragged>(|style, _, _, cx| style.bg(cx.theme().primary.opacity(0.14)))
        .on_drop(cx.listener(move |this, source: &Dragged, window, cx| {
            this.drop_ref(source.clone(), target.clone(), window.mouse_position(), cx);
        }))
}

impl GraphView {
    /// Escape in a menu: take a submenu back in, else close the menu.
    pub(super) fn back_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.as_mut().is_some_and(Menu::close_sub) {
            cx.notify();
        } else {
            self.close_menu(cx);
        }
    }

    /// Right: pops out the submenu of the line the keyboard is on. Returns whether it took
    /// the key.
    pub(super) fn open_menu_line(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &mut self.menu else {
            return false;
        };
        if let Some(at) = menu
            .highlighted
            .filter(|&at| menu.lines[at].opens_submenu())
        {
            menu.open_sub(at, true);
            cx.notify();
        }
        true
    }

    /// A click on a line that has a submenu pops it out, with the keys in it.
    fn open_menu_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.open_sub(index, true);
            cx.notify();
        }
    }

    /// Left: takes a submenu back in without closing the menu. Returns whether it took the key.
    pub(super) fn close_menu_sub(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &mut self.menu else {
            return false;
        };
        if menu.close_sub() {
            cx.notify();
        }
        true
    }

    /// The pointer is on line `index`: its submenu pops out, and another line's goes in.
    fn hover_menu_line(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        if menu.lines[index].opens_submenu() {
            menu.open_sub(index, false);
        } else if !menu.close_sub() {
            return;
        }
        cx.notify();
    }

    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    /// Up and down while a menu is open move through it. Returns whether it took the key.
    pub(super) fn step_menu(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &mut self.menu else {
            return false;
        };
        menu.step(delta);
        cx.notify();
        true
    }

    pub(super) fn activate_menu(&mut self, cx: &mut Context<Self>) {
        // Enter on a line with a submenu pops it out instead.
        if self.menu.as_ref().is_some_and(|menu| {
            menu.sub.as_ref().is_none_or(|sub| !sub.keys)
                && menu
                    .highlighted
                    .is_some_and(|at| menu.lines[at].opens_submenu())
        }) {
            self.open_menu_line(cx);
            return;
        }
        let command = self
            .menu
            .as_ref()
            .and_then(|menu| menu.current()?.command().cloned());
        if let Some(command) = command {
            self.choose(command, cx);
        }
    }

    pub(super) fn show_menu(
        &mut self,
        position: Point<Pixels>,
        lines: Vec<Line>,
        cx: &mut Context<Self>,
    ) {
        if !lines.is_empty() {
            self.hovered_refs = None;
            self.popover_hovered = false;
            self.menu = Some(Menu::new(position, lines));
            cx.notify();
        }
    }

    /// Closes the menu and does what the chosen line says, asking first when it must.
    pub(super) fn choose(&mut self, command: Command, cx: &mut Context<Self>) {
        self.menu = None;
        if let Command::SwitchWorktree { path } = &command {
            cx.emit(SwitchWorktree(path.clone()));
        } else if let Command::OpenWorktreeInTab { path } = &command {
            cx.emit(OpenWorktreeInTab(path.clone()));
        } else if let Command::OpenSubmoduleInTab { path } = &command {
            cx.emit(OpenWorktreeInTab(path.clone()));
        } else if let Command::Form(form) = &command {
            self.open_form(form.clone(), cx);
        } else if let Command::ShowConflict { path } = &command {
            self.show_conflict(path, cx);
        } else if let Command::UseSide { path, side } = &command {
            self.use_side_for_file(path, *side, cx);
        } else if let Command::ShowFile { index } = &command {
            self.show_file(*index, cx);
        } else if let Command::StageFiles { paths, unstage } = &command {
            self.stage_paths(paths.clone(), *unstage, cx);
        } else if let Some(question) = command.forge_question() {
            // The forge isn't known, which only the repository's settings can put right.
            let confirm = Confirm::new(
                question.to_owned(),
                "Open settings",
                Weight::Popup,
                |this, cx| super::settings::open_repository_page(this.repo.clone(), cx),
            );
            self.ask(confirm, cx);
        } else if let Some(path) = command.reveal_path() {
            // A deleted file isn't there to show, so show the folder it was in.
            if let Some(repo) = &self.repo {
                let mut target = repo.join(path);
                while !target.exists() && target.pop() {}
                cx.reveal_path(&target);
            }
        } else if let Some(url) = command.url() {
            cx.open_url(url);
            self.notice = command
                .opened_notice()
                .map(|text| Notice::Info(text.into()));
            cx.notify();
        } else if let Some(text) = copied_text(&command) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.notice = command
                .copied_notice()
                .map(|text| Notice::Info(text.into()));
            cx.notify();
        } else if let Some(confirmation) = command.confirmation() {
            let confirm = Confirm::new(
                confirmation.message,
                confirmation.action,
                confirmation.weight,
                move |this, cx| this.run_command(command.clone(), cx),
            );
            self.ask(confirm, cx);
        } else {
            self.run_command(command, cx);
        }
    }

    /// A double-click on a branch switches to it. The checked-out branch has nowhere to go.
    pub(super) fn open_on_double_click(
        &mut self,
        target: &Dragged,
        event: &ClickEvent,
        cx: &mut Context<Self>,
    ) {
        let switchable = matches!(target.kind, RefKind::Branch | RefKind::Remote)
            && !(target.kind == RefKind::Branch
                && self.refs.head_branch.as_deref() == Some(target.name.as_str()));
        if event.click_count() < 2 || !switchable || self.busy.is_some() {
            return;
        }
        self.run_command(
            Command::CheckOutRef {
                kind: target.kind,
                name: target.name.clone(),
            },
            cx,
        );
    }

    pub(super) fn run_command(&mut self, command: Command, cx: &mut Context<Self>) {
        let label = command.progress();
        if !matches!(command, Command::PushBranch { .. } | Command::PushTo { .. }) {
            self.run_operation(label, move |repo| command.run(repo), cx);
            return;
        }
        // A push git refuses because the remote has commits the branch lacks can be forced,
        // which is asked about in its own popup.
        let Some(workdir) = self.workdir() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
        self.notice = None;
        cx.notify();
        Self::spawn_blocking(
            move || {
                let result = command.run(&workdir);
                let force = result
                    .as_ref()
                    .err()
                    .and_then(|error| command.force_after_rejection(&workdir, error));
                (result, force)
            },
            |this, (result, force), cx| match force {
                Some(force) => {
                    this.busy = None;
                    this.ask_to_force(force, cx);
                }
                None => this.finish_operation(result, cx),
            },
            cx,
        );
    }

    /// Asks before a force push, which takes Enter twice.
    fn ask_to_force(&mut self, force: Command, cx: &mut Context<Self>) {
        if let Some(confirmation) = force.confirmation() {
            let confirm = Confirm::new(
                confirmation.message,
                confirmation.action,
                confirmation.weight,
                move |this, cx| this.run_command(force.clone(), cx),
            );
            self.ask(confirm, cx);
        }
    }

    pub(super) fn open_ref_menu(
        &mut self,
        target: Dragged,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        // A right-click on one of several picked refs acts on all of them.
        let picks = self.sidebar.picks.items();
        let lines = if picks.len() > 1 && picks.contains(&target) {
            menu::refs_menu(&self.refs, picks)
        } else {
            menu::ref_menu(&self.refs, &target)
        };
        self.show_menu(position, lines, cx);
    }

    /// The menu for a file in the inspector. `index` is its place in the details shown.
    pub(super) fn open_file_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(details) = self.shown_details() else {
            return;
        };
        let Some(file) = details.files.get(index) else {
            return;
        };
        let source = if self.working_row_is_selected() {
            self.working_file(index)
                .map(|(side, _)| FileSource::Working(side))
        } else {
            Some(FileSource::Commit {
                oid: &details.oid,
                old_path: file.old_path.as_deref(),
            })
        };
        let lines = menu::file_menu(&file.path, index, source);
        self.show_menu(position, lines, cx);
    }

    /// The menu for a conflicted file: show both sides, or take one for the whole file.
    pub(super) fn open_conflict_menu(
        &mut self,
        path: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(progress) = self.paused_operation().cloned() else {
            return;
        };
        self.show_menu(position, menu::conflict_menu(&progress, path), cx);
    }

    /// The menu for a folder of the inspector's tree, acting on the files under it.
    pub(super) fn open_folder_menu(
        &mut self,
        path: &str,
        files: Vec<String>,
        side: Option<Side>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.show_menu(position, menu::folder_menu(path, files, side), cx);
    }

    pub(super) fn open_worktree_menu(
        &mut self,
        path: &Path,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let lines = menu::worktree_menu(&self.refs, path);
        self.show_menu(position, lines, cx);
    }

    pub(super) fn open_submodule_menu(
        &mut self,
        path: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let lines = menu::submodule_menu(&self.refs, path);
        self.show_menu(position, lines, cx);
    }

    /// Works out what dropping `source` on `target` can mean, then asks. A local branch
    /// can take it in; a remote branch is the base of a request, or where a
    /// local branch is pushed.
    pub fn drop_ref(
        &mut self,
        source: Dragged,
        target: Dragged,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(workdir) = self.workdir() else {
            return;
        };
        let (from, onto) = (source.name.clone(), target.name.clone());
        let (remotes, chosen) = (self.refs.remote_configs.clone(), self.refs.forge);
        let proposing = target.kind == RefKind::Remote && source.kind == RefKind::Remote;
        Self::spawn_blocking(
            move || {
                // A remote branch is proposed on its forge, which a self-hosted server may
                // have to be asked about before the menu can say. It is asked once.
                if proposing {
                    forge::warm(&remotes, &onto, chosen);
                }
                branch::relation(&workdir, &from, &onto)
            },
            move |this, relation: Result<Relation, String>, cx| {
                let lines = match target.kind {
                    RefKind::Remote if source.kind == RefKind::Branch => {
                        menu::push_drop_menu(&this.refs, &source, &target, relation)
                    }
                    RefKind::Remote => {
                        menu::request_drop_menu(&this.refs, &source, &target, relation)
                    }
                    _ => menu::drop_menu(&source, &target.name, relation),
                };
                this.show_menu(position, lines, cx);
            },
            cx,
        );
    }

    pub(super) fn open_commit_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.entries.get(index).is_none_or(|entry| entry.working) {
            return;
        }
        // A stash has the stash menu, as in the sidebar.
        if let Some(stash) = self.refs.stash_at(&self.entries[index].commit.oid) {
            self.select(index, cx);
            return self.open_stash_menu(stash, position, cx);
        }
        // A click inside the selection acts on all of it, one outside starts a new one.
        if !self.selection.contains(&self.entries, self.selected, index) {
            self.select(index, cx);
        }
        let commits = self.selection.commits(&self.entries, self.selected);
        let lines = menu::commit_menu(&self.refs, &commits);
        self.show_menu(position, lines, cx);
    }

    /// The card of one menu: its lines, and for the main menu the submenu beside its line.
    fn menu_card(
        &self,
        lines: &[Line],
        highlighted: Option<usize>,
        sub: Option<&Sub>,
        main: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (panel, border, text, muted) = (
            theme.title_bar,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let (hover, active, danger) = (theme.list_hover, selected_row(theme), theme.danger);
        let peach = tokens(theme.is_dark()).peach;
        // Rows of the main menu and of a submenu both count from 0.
        let scope = if main { "menu-item" } else { "menu-sub-item" };

        let mut items = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            items.push(match line {
                Line::Separator => div().h(px(1.)).my_1().mx_1().bg(border).into_any_element(),
                Line::Heading(label) => div()
                    .h(px(24.))
                    .px_2()
                    .h_flex()
                    .text_size(TEXT_SMALL)
                    .font_medium()
                    .text_color(muted)
                    .child(div().min_w_0().truncate().child(label.clone()))
                    .into_any_element(),
                Line::Item {
                    label,
                    command,
                    danger: is_danger,
                    glyph,
                } => {
                    let highlighted = highlighted == Some(index);
                    let color = match (command, is_danger) {
                        (None, _) => muted,
                        (_, true) => danger,
                        _ => text,
                    };
                    // A line that does something is one row, cut short if it must be. One
                    // that only explains is read in full, so it takes as many rows as it needs.
                    let explains = command.is_none();
                    let opens = line.opens_submenu();
                    let row = div()
                        .id((scope, index))
                        .relative()
                        .h_flex()
                        .gap_2()
                        .when(explains, |row| row.min_h(ITEM_HEIGHT).py_1())
                        .when(!explains, |row| row.h(ITEM_HEIGHT))
                        .px_2()
                        .rounded(px(4.))
                        .text_size(TEXT_BODY)
                        .text_color(color)
                        .when(highlighted, |row| row.bg(active))
                        .children(glyph.map(|glyph| glyph_icon(glyph, color, peach)))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .when(!explains, |label| label.truncate())
                                .child(label.clone()),
                        )
                        .when(opens, |row| row.child(div().child("›")));
                    // The submenu pops out on the right of the line that opens it.
                    let row = match sub.filter(|sub| sub.at == index) {
                        Some(open) => {
                            let inside = open.keys.then_some(open.highlighted).flatten();
                            row.child(
                                self.menu_card(&open.lines, inside, None, false, cx)
                                    .absolute()
                                    .top(px(-5.))
                                    .left(relative(1.))
                                    .min_w(px(240.)),
                            )
                        }
                        None => row,
                    };
                    match command.clone() {
                        Some(command) => row
                            .cursor_pointer()
                            .when(!highlighted, |row| row.hover(|style| style.bg(hover)))
                            .when(main, |row| {
                                row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered {
                                        this.hover_menu_line(index, cx)
                                    }
                                }))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if opens {
                                    this.open_menu_at(index, cx)
                                } else {
                                    this.choose(command.clone(), cx)
                                }
                            }))
                            .into_any_element(),
                        None => row.into_any_element(),
                    }
                }
            });
        }

        div()
            .v_flex()
            // As wide as its longest line, so a line names its whole result, up to a limit.
            .min_w(MENU_WIDTH)
            .max_w(MENU_MAX_WIDTH)
            .p_1()
            .rounded(px(8.))
            .bg(panel)
            .border_1()
            .border_color(border)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(items)
    }

    pub(super) fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let close = |this: &mut Self,
                     _: &MouseDownEvent,
                     _: &mut Window,
                     cx: &mut Context<Self>| { this.close_menu(cx) };
        let panel = self.menu_card(&menu.lines, menu.highlighted, menu.sub.as_ref(), true, cx);

        Some(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(close))
                    .on_mouse_down(MouseButton::Right, cx.listener(close))
                    .child(
                        anchored()
                            .position(menu.position)
                            .snap_to_window_with_margin(EDGE_MARGIN)
                            .child(panel),
                    ),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }
}

fn copied_text(command: &Command) -> Option<String> {
    match command {
        Command::Copy { text, .. } => Some(text.clone()),
        _ => None,
    }
}
