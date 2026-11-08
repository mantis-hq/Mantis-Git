//! Context menus and drag and drop: drawing the lines core asks for, and running the one
//! that is chosen. What a menu offers, and what each line does, is decided in
//! `gitclient_core::menu`.

use super::dialog::Confirm;
use super::notice::Notice;
use super::parts::{TEXT_BODY, TEXT_CHIP, TEXT_SMALL};
use super::{GraphView, OpenWorktreeInTab, SwitchWorktree};
use crate::icons::Icon;
use crate::theme::{selected_row, tokens};
use gitclient_core::history::refs::RefKind;
use gitclient_core::menu::command::{Command, Weight};
use gitclient_core::menu::{self, FileSource, Glyph, Line};
use gitclient_core::repo::branch::{self, Dragged, Relation};
use gitclient_core::repo::forge;
use gitclient_core::working::Side;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
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
}

impl Menu {
    pub(super) fn new(position: Point<Pixels>, lines: Vec<Line>) -> Self {
        Self {
            position,
            lines,
            highlighted: None,
        }
    }

    /// Moves the keyboard highlight to the next or previous line with a command, wrapping.
    fn step(&mut self, delta: isize) {
        let len = self.lines.len() as isize;
        let mut at = self
            .highlighted
            .map_or(if delta > 0 { -1 } else { len }, |index| index as isize);
        for _ in 0..len {
            at = (at + delta).rem_euclid(len);
            if self.lines[at as usize].command().is_some() {
                self.highlighted = Some(at as usize);
                return;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn lines(&self) -> &[Line] {
        &self.lines
    }

    #[cfg(test)]
    pub(super) fn highlighted_label(&self) -> Option<String> {
        match self.lines.get(self.highlighted?)? {
            Line::Item { label, .. } => Some(label.clone()),
            _ => None,
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
/// branch, something to drop on.
pub(super) fn wire_ref(
    element: Stateful<Div>,
    target: Dragged,
    color: Hsla,
    cx: &mut Context<GraphView>,
) -> Stateful<Div> {
    let menu_target = target.clone();
    let element = element
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                // A row under a chip would otherwise open its own menu on top.
                cx.stop_propagation();
                this.open_ref_menu(menu_target.clone(), event.position, cx);
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
                        && (target.kind == RefKind::Branch || source.kind == RefKind::Remote)
                })
            }
        })
        .drag_over::<Dragged>(|style, _, _, cx| style.bg(cx.theme().primary.opacity(0.14)))
        .on_drop(cx.listener(move |this, source: &Dragged, window, cx| {
            this.drop_ref(source.clone(), target.clone(), window.mouse_position(), cx);
        }))
}

impl GraphView {
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
        let command = self.menu.as_ref().and_then(|menu| {
            let line = menu.lines.get(menu.highlighted?)?;
            line.command().cloned()
        });
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
        self.run_operation(label, move |repo| command.run(repo), cx);
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
    /// can take it in; a remote branch can only be the base of a request.
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
        let proposing = target.kind == RefKind::Remote;
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

    pub(super) fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let theme = cx.theme();
        let (panel, border, text, muted) = (
            theme.title_bar,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let (hover, active, danger) = (theme.list_hover, selected_row(theme), theme.danger);
        let peach = tokens(theme.is_dark()).peach;

        let mut items = Vec::with_capacity(menu.lines.len());
        for (index, line) in menu.lines.iter().enumerate() {
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
                    let highlighted = menu.highlighted == Some(index);
                    let color = match (command, is_danger) {
                        (None, _) => muted,
                        (_, true) => danger,
                        _ => text,
                    };
                    // A line that does something is one row, cut short if it must be. One
                    // that only explains is read in full, so it takes as many rows as it needs.
                    let explains = command.is_none();
                    let row = div()
                        .id(("menu-item", index))
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
                        );
                    match command.clone() {
                        Some(command) => row
                            .cursor_pointer()
                            .when(!highlighted, |row| row.hover(|style| style.bg(hover)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.choose(command.clone(), cx)),
                            )
                            .into_any_element(),
                        None => row.into_any_element(),
                    }
                }
            });
        }

        let close = |this: &mut Self,
                     _: &MouseDownEvent,
                     _: &mut Window,
                     cx: &mut Context<Self>| { this.close_menu(cx) };
        let panel = div()
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
            .children(items);

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
