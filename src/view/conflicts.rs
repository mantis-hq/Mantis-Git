//! A paused rebase, cherry-pick, revert or merge. The inspector lists the files that need a
//! decision. A file opens in the resolver over the graph: its conflicts side by side, to take
//! whole or line by line, and the file's result in an editor, saved with one button.

use super::notice::Confirm;
use super::parts::{
    TEXT_CHIP, TEXT_SMALL, checkbox, expand_tabs, file_path, panel_row, pill, section_label,
};
use super::{GraphView, Notice};
use crate::theme::{lane_color, tokens};
use gitclient_core::conflict::{self, Decision, Progress, Side};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    progress: Option<Arc<Progress>>,
    /// Whether the resolver is open over the graph.
    pub(super) open: bool,
    /// The file on show in the resolver.
    shown: Option<String>,
    /// Each file's result so far. It starts as the file with its markers, and is edited
    /// from there, by picking from the sides above it or by hand.
    drafts: HashMap<String, String>,
    /// The side picked for a file that can only go one way as a whole, such as a binary one.
    sides: HashMap<String, Side>,
    /// The lines picked in the conflicts of the file on show: conflict, side, line.
    lines: HashSet<(usize, Side, usize)>,
    /// Files saved at this stop. They stay listed until the operation goes on.
    resolved: Vec<String>,
    /// The file being saved, so that once it is, the next one opens.
    saving: Option<String>,
    editor: Option<Entity<TextareaState>>,
    /// The file whose result is in the editor.
    editing: Option<String>,
    /// Set while continuing, so that the check afterwards can tell finished from cancelled.
    continuing: Option<Arc<Progress>>,
    rebase_target: Option<String>,
    _editor_changes: Option<Subscription>,
}

impl State {
    /// Forgets everything about the stop that was open.
    fn reset(&mut self) {
        self.open = false;
        self.shown = None;
        self.drafts.clear();
        self.sides.clear();
        self.lines.clear();
        self.resolved.clear();
        self.saving = None;
        self.editing = None;
    }
}

/// What a conflict's button puts in the result.
#[derive(Clone, Copy)]
pub(super) enum Pick {
    Mine,
    Other,
    Lines,
}

impl GraphView {
    pub fn paused_operation(&self) -> Option<&Arc<Progress>> {
        self.conflict.progress.as_ref()
    }

    /// Whether a conflicted file is open over the graph.
    pub fn resolver_is_open(&self) -> bool {
        self.conflict.open && self.conflict.progress.is_some()
    }

    /// Looks for a paused operation, which can appear or go away outside the app too.
    pub(super) fn check_paused_operation(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        Self::spawn_blocking(
            move || (conflict::detect(&repo), conflict::rebase_target(&repo)),
            Self::paused_operation_read,
            cx,
        );
    }

    fn paused_operation_read(
        &mut self,
        (progress, target): (Option<Progress>, Option<String>),
        cx: &mut Context<Self>,
    ) {
        self.conflict.rebase_target = target;
        let continuing = self.conflict.continuing.take();
        let failed = matches!(self.notice, Some(Notice::Error(_)));
        match progress {
            Some(new) => {
                // Saving a file changes the decisions but not where the operation stands.
                let same =
                    (self.conflict.progress.as_deref()).is_some_and(|old| old.is_same_stop(&new));
                let new = Arc::new(new);
                if same {
                    self.conflict.progress = Some(new.clone());
                    self.after_save(&new, cx);
                } else {
                    self.conflict.reset();
                    self.conflict.progress = Some(new);
                    self.focus_conflicts(cx);
                }
            }
            None => {
                self.conflict.reset();
                self.conflict.progress = None;
                if let Some(finished) = continuing.filter(|_| !failed) {
                    self.notice = Some(Notice::Info(finished.finished_sentence().into()));
                }
            }
        }
        cx.notify();
    }

    /// Once a file is saved it is listed as resolved, and the resolver moves on to the next
    /// file that needs one, or closes when there is none.
    fn after_save(&mut self, progress: &Progress, cx: &mut Context<Self>) {
        let Some(saved) = self.conflict.saving.take() else {
            return;
        };
        let files = progress.files();
        if files.iter().any(|file| file.path == saved) {
            // It didn't save, and the band under the header says why.
            return;
        }
        self.conflict.drafts.remove(&saved);
        self.conflict.sides.remove(&saved);
        if !self.conflict.resolved.contains(&saved) {
            self.conflict.resolved.push(saved.clone());
        }
        if self.conflict.shown.as_deref() != Some(saved.as_str()) {
            return;
        }
        self.conflict.lines.clear();
        match files.first() {
            Some(next) => self.show_conflict(&next.path.clone(), cx),
            None => {
                self.conflict.open = false;
                self.notice = Some(Notice::Info(progress.resolved_sentence().into()));
            }
        }
    }

    /// Shows the working changes, where the conflicts are listed, so that a paused
    /// operation is never out of sight.
    pub(super) fn focus_conflicts(&mut self, cx: &mut Context<Self>) {
        if self.conflict.progress.is_none() || self.working_row_is_selected() {
            return;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.working) {
            self.select(index, cx);
        }
    }

    pub(super) fn render_rebase_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.conflict.rebase_target.clone()?;
        Some(
            Button::new("rebase")
                .ghost()
                .small()
                .label(format!("Rebase onto {target}"))
                .disabled(self.busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let target = target.clone();
                    this.confirm = Some(Confirm::banner(
                        conflict::rebase_warning(&target),
                        "Rebase",
                        move |this, cx| {
                            let target = target.clone();
                            this.run_operation(
                                "Rebasing",
                                move |repo| conflict::rebase_onto(repo, &target),
                                cx,
                            );
                        },
                    ));
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The conflicts of a file that has lines to merge, in the order they come.
    fn file_decisions(&self, path: &str) -> Vec<Decision> {
        let decisions = self.conflict.progress.iter().flat_map(|p| &p.decisions);
        decisions
            .filter(|decision| decision.path == path && !decision.whole_file)
            .cloned()
            .collect()
    }

    /// Whether a file has no lines to merge, such as a binary one, and is one choice.
    fn is_whole_file(&self, path: &str) -> bool {
        let mut decisions = self.conflict.progress.iter().flat_map(|p| &p.decisions);
        decisions.any(|decision| decision.path == path && decision.whole_file)
    }

    /// How many conflicts a file still has in its result.
    fn conflicts_left(&self, path: &str) -> usize {
        let Some(progress) = &self.conflict.progress else {
            return 0;
        };
        if self.is_whole_file(path) {
            return usize::from(!self.conflict.sides.contains_key(path));
        }
        match self.conflict.drafts.get(path) {
            Some(draft) => progress.open_chunks(path, draft).iter().flatten().count(),
            None => self.file_decisions(path).len(),
        }
    }

    /// Opens the resolver on a file, reading its result from the file as it stands.
    pub fn show_conflict(&mut self, path: &str, cx: &mut Context<Self>) {
        self.conflict.shown = Some(path.to_owned());
        self.conflict.open = true;
        self.conflict.lines.clear();
        self.conflict.editing = None;
        // Only one thing covers the graph at a time.
        self.close_diff(cx);
        cx.notify();
        if self.is_whole_file(path) || self.conflict.drafts.contains_key(path) {
            return;
        }
        let Some(repo) = self.workdir() else {
            return;
        };
        let path = path.to_owned();
        let wanted = path.clone();
        Self::spawn_blocking(
            move || conflict::read_text(&repo, &path),
            move |this, text, cx| {
                match text {
                    Ok(text) => {
                        this.conflict.drafts.entry(wanted).or_insert(text);
                    }
                    Err(message) => this.notice = Some(Notice::Error(message.into())),
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(super) fn close_resolver(&mut self, cx: &mut Context<Self>) {
        self.conflict.open = false;
        cx.notify();
    }

    /// Takes one side for every conflict in a file and saves it.
    pub(super) fn use_side_for_file(&mut self, path: &str, side: Side, cx: &mut Context<Self>) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        self.conflict.saving = Some(path.to_owned());
        let path = path.to_owned();
        self.run_operation(
            "Resolving",
            move |repo| conflict::resolve_with_side(repo, &progress, &path, side).map(|()| None),
            cx,
        );
    }

    pub(super) fn toggle_line(&mut self, key: (usize, Side, usize), cx: &mut Context<Self>) {
        if !self.conflict.lines.remove(&key) {
            self.conflict.lines.insert(key);
        }
        cx.notify();
    }

    /// The lines picked in one conflict: those of the first side, then the second's.
    fn picked_lines(&self, ordinal: usize, decision: &Decision) -> String {
        let mut picked = String::new();
        for side in [Side::Mine, Side::Other] {
            for (at, line) in conflict::lines_of(decision.side(side)).iter().enumerate() {
                if self.conflict.lines.contains(&(ordinal, side, at)) {
                    picked.push_str(line);
                    if !line.ends_with('\n') {
                        picked.push('\n');
                    }
                }
            }
        }
        picked
    }

    /// Puts a side, or the picked lines, of one conflict in the result in place of its
    /// markers.
    pub(super) fn use_chunk(&mut self, ordinal: usize, pick: Pick, cx: &mut Context<Self>) {
        let (Some(progress), Some(path)) =
            (self.conflict.progress.clone(), self.conflict.shown.clone())
        else {
            return;
        };
        let Some(draft) = self.conflict.drafts.get(&path).cloned() else {
            return;
        };
        let decisions = self.file_decisions(&path);
        let open = progress.open_chunks(&path, &draft);
        let (Some(decision), Some(Some(block))) = (decisions.get(ordinal), open.get(ordinal))
        else {
            return;
        };
        let chosen = match pick {
            Pick::Mine => decision.mine.clone(),
            Pick::Other => decision.other.clone(),
            Pick::Lines => self.picked_lines(ordinal, decision),
        };
        let text = conflict::replace_block(&draft, block, &chosen);
        self.conflict.drafts.insert(path, text);
        // So that the editor reads the new result the next time it is drawn.
        self.conflict.editing = None;
        self.conflict.lines.retain(|(at, ..)| *at != ordinal);
        cx.notify();
    }

    fn editor_changed(&mut self, editor: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let Some(path) = self.conflict.editing.clone() else {
            return;
        };
        let value = editor.read(cx).value().to_string();
        self.conflict.drafts.insert(path, value);
        cx.notify();
    }

    /// Keeps the editor on the result of the file on show, making it the first time it is
    /// needed.
    pub(super) fn sync_conflict_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.conflict.open {
            return;
        }
        let Some(path) = self.conflict.shown.clone() else {
            return;
        };
        let Some(draft) = self.conflict.drafts.get(&path).cloned() else {
            return;
        };
        if self.is_whole_file(&path) {
            return;
        }
        let editor = match &self.conflict.editor {
            Some(editor) => editor.clone(),
            None => {
                let editor = cx.new(|cx| {
                    TextareaState::new(window, cx).placeholder("The file as it will be saved.")
                });
                let changes = cx.subscribe(&editor, |this, editor, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.editor_changed(&editor, cx);
                    }
                });
                self.conflict._editor_changes = Some(changes);
                self.conflict.editor = Some(editor.clone());
                editor
            }
        };
        if self.conflict.editing.as_deref() != Some(path.as_str()) {
            // Setting the text is a change too, which isn't the user's.
            self.conflict.editing = None;
            editor.update(cx, |editor, cx| editor.set_value(draft, window, cx));
            self.conflict.editing = Some(path);
        }
    }

    /// Saves the result of the file on show and stages it, which takes the conflict off it.
    pub(super) fn save_conflict(&mut self, cx: &mut Context<Self>) {
        let (Some(progress), Some(path)) =
            (self.conflict.progress.clone(), self.conflict.shown.clone())
        else {
            return;
        };
        if self.conflicts_left(&path) > 0 {
            return;
        }
        self.conflict.saving = Some(path.clone());
        if self.is_whole_file(&path) {
            let Some(side) = self.conflict.sides.get(&path).copied() else {
                return;
            };
            self.run_operation(
                "Saving",
                move |repo| {
                    conflict::resolve_with_side(repo, &progress, &path, side).map(|()| None)
                },
                cx,
            );
            return;
        }
        let Some(result) = self.conflict.drafts.get(&path).cloned() else {
            return;
        };
        self.run_operation(
            "Saving",
            move |repo| conflict::save_file(repo, &path, &result).map(|()| None),
            cx,
        );
    }

    /// Moves the operation on once every file is resolved.
    pub(super) fn continue_operation(&mut self, cx: &mut Context<Self>) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        if !progress.decisions.is_empty() {
            return;
        }
        let kind = progress.kind;
        self.conflict.continuing = Some(progress.clone());
        let label = format!("Continuing {}", kind.title().to_lowercase());
        self.run_operation(
            label,
            move |repo| conflict::carry_on(repo, kind).map(|_| None),
            cx,
        );
    }

    fn cancel_operation(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.conflict.progress.as_ref().map(|p| p.kind) else {
            return;
        };
        self.run_operation("Cancelling", move |repo| conflict::cancel(repo, kind), cx);
    }

    /// The inspector while an operation is paused: what happened, the files that need a
    /// decision, and the way out either side.
    pub(super) fn render_conflict_panel(&self, progress: &Progress, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let colors = tokens(theme.is_dark());
        let (border, muted, success) = (theme.border, theme.muted_foreground, theme.success);
        let kind = progress.kind.title().to_lowercase();
        let ready = progress.decisions.is_empty();
        let busy = self.busy.is_some();
        let files = progress.files();
        let shown_path = self.conflict.shown.clone().filter(|_| self.conflict.open);

        let mut rows: Vec<Stateful<Div>> = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let left = self.conflicts_left(&file.path);
            let (path, menu_path) = (file.path.clone(), file.path.clone());
            rows.push(
                panel_row(
                    ("conflict-file", index),
                    shown_path.as_deref() == Some(file.path.as_str()),
                    cx,
                )
                .h(px(28.))
                .on_click(cx.listener(move |this, _, _, cx| this.show_conflict(&path, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_conflict_menu(&menu_path, event.position, cx);
                    }),
                )
                .child(
                    div()
                        .w(px(12.))
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .font_semibold()
                        .text_color(colors.peach)
                        .child("!"),
                )
                .child(file_path(&file.path, muted))
                .child(
                    div()
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child(file.note(left)),
                ),
            );
        }
        // Saved files stay in the list, ticked, until the operation goes on.
        for (index, path) in self.conflict.resolved.iter().enumerate() {
            rows.push(
                panel_row(("resolved-file", index), false, cx)
                    .h(px(28.))
                    .cursor_default()
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .font_semibold()
                            .text_color(success)
                            .child("✓"),
                    )
                    .child(file_path(path, muted))
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child("Resolved"),
                    ),
            );
        }

        div()
            .v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .child(
                div()
                    .v_flex()
                    .flex_none()
                    .gap_2()
                    .child(div().h_flex().gap_2().child(pill(
                        &format!("{} paused", progress.kind.title()),
                        colors.peach,
                        cx,
                    )))
                    .child(div().text_color(colors.subtext0).child(progress.sentence())),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.))
                    .mx(px(-8.))
                    .child(
                        section_label(muted)
                            .h(px(24.))
                            .px_2()
                            .h_flex()
                            .child(progress.list_heading(self.conflict.resolved.len())),
                    )
                    .child(
                        div()
                            .id("conflict-files")
                            .v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .flex_none()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .text_color(muted)
                            .child(progress.cancel_note()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("cancel-operation")
                                    .ghost()
                                    .label(format!("Cancel {kind}"))
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_operation(cx)),
                                    ),
                            )
                            .child(
                                Button::new("continue-operation")
                                    .primary()
                                    .label(format!("Continue {kind}"))
                                    .disabled(busy || !ready)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.continue_operation(cx)),
                                    ),
                            ),
                    ),
            )
    }

    /// The conflicted file on show, over the graph.
    pub(super) fn render_resolver(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.conflict.open {
            return None;
        }
        let progress = self.conflict.progress.clone()?;
        let path = self.conflict.shown.clone()?;
        Some(
            self.render_file_resolver(&progress, &path, cx)
                .into_any_element(),
        )
    }

    fn render_file_resolver(&self, progress: &Progress, path: &str, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let (success, peach) = (theme.success, tokens(is_dark).peach);
        let whole = self.is_whole_file(path);
        let loaded = whole || self.conflict.drafts.contains_key(path);
        let left = self.conflicts_left(path);
        let ready = loaded && left == 0;
        let hint = if whole {
            if ready {
                "Ready to save".to_owned()
            } else {
                "Choose which version to keep".to_owned()
            }
        } else {
            conflict::result_hint(left)
        };
        let busy = self.busy.is_some();

        let header = div()
            .h_flex()
            .h(px(40.))
            .flex_none()
            .px_4()
            .gap_3()
            .bg(panel)
            .border_b_1()
            .border_color(border)
            .text_size(TEXT_SMALL)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_flex()
                    .gap_3()
                    .child(file_path(path, muted))
                    .child(
                        div()
                            .flex_none()
                            .text_color(if ready { success } else { peach })
                            .child(hint),
                    ),
            )
            .child(
                Button::new("close-resolver")
                    .ghost()
                    .xsmall()
                    .label("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close_resolver(cx))),
            );

        let body = if whole {
            self.render_whole_file_choice(progress, path, cx)
        } else {
            self.render_conflicts_and_result(progress, path, cx)
        };

        let save = div()
            .flex_none()
            .p_4()
            .bg(panel)
            .border_t_1()
            .border_color(border)
            .child(
                Button::new("save-conflict")
                    .primary()
                    .large()
                    .w_full()
                    .label("Save and mark as resolved")
                    .disabled(!ready || busy)
                    .on_click(cx.listener(|this, _, _, cx| this.save_conflict(cx))),
            );

        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(body)
            .child(save)
    }

    /// A file with nothing to merge line by line: one version or the other, as a whole.
    fn render_whole_file_choice(
        &self,
        progress: &Progress,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (background, border, foreground) = (theme.background, theme.border, theme.foreground);
        let (on_color, subtext) = (theme.primary_foreground, tokens(is_dark).subtext0);
        let picked = self.conflict.sides.get(path).copied();
        let decision = progress
            .decisions
            .iter()
            .find(|decision| decision.path == path);
        let other = lane_color(self.refs.branch_color(&progress.other), is_dark);
        let sides = [
            (Side::Mine, &progress.mine, theme.primary),
            (Side::Other, &progress.other, other),
        ];
        let cards = sides.map(|(side, branch, color)| {
            let on = picked == Some(side);
            let (what, why) = decision.map_or(("", ""), |decision| match side {
                Side::Mine => (decision.mine.as_str(), decision.mine_why.as_str()),
                Side::Other => (decision.other.as_str(), decision.other_why.as_str()),
            });
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .p_4()
                .gap_2()
                .rounded(px(8.))
                .bg(background)
                .border(px(if on { 2. } else { 1. }))
                .border_color(if on { color } else { border })
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .font_semibold()
                        .text_color(color)
                        .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                        .child(div().min_w_0().truncate().child(branch.clone())),
                )
                .child(div().text_color(subtext).child(why.to_owned()))
                .child(div().child(what.to_owned()))
                .child(
                    div()
                        .id(match side {
                            Side::Mine => "keep-mine",
                            Side::Other => "keep-other",
                        })
                        .mt_auto()
                        .h_flex()
                        .h(px(34.))
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .font_semibold()
                        .bg(if on { color } else { border })
                        .text_color(if on { on_color } else { foreground })
                        .child(div().min_w_0().truncate().child(format!("Use {branch}")))
                        .on_click({
                            let path = path.to_owned();
                            cx.listener(move |this, _, _, cx| {
                                this.conflict.sides.insert(path.clone(), side);
                                cx.notify();
                            })
                        }),
                )
        });
        div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_4()
            .children(cards)
    }

    /// The file's conflicts side by side, over its result in an editor.
    fn render_conflicts_and_result(
        &self,
        progress: &Progress,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let subtext = tokens(is_dark).subtext0;
        let draft = self.conflict.drafts.get(path);
        let decisions = self.file_decisions(path);
        let open: Vec<bool> = match draft {
            Some(draft) => progress
                .open_chunks(path, draft)
                .iter()
                .map(Option::is_some)
                .collect(),
            None => vec![true; decisions.len()],
        };
        let any_open = open.iter().any(|open| *open);
        let total = decisions.len();
        let cards: Vec<Div> = decisions
            .iter()
            .enumerate()
            .map(|(ordinal, decision)| {
                if open[ordinal] {
                    self.render_chunk(progress, ordinal, total, decision, cx)
                } else {
                    div()
                        .h_flex()
                        .flex_none()
                        .h(px(30.))
                        .px_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(border)
                        .text_size(TEXT_SMALL)
                        .text_color(muted)
                        .child(format!(
                            "Conflict {} of {total} · line {} · settled in the result",
                            ordinal + 1,
                            decision.line
                        ))
                }
            })
            .collect();

        let editor = match (&self.conflict.editor, draft) {
            (Some(editor), Some(_)) if self.conflict.editing.as_deref() == Some(path) => div()
                .flex_1()
                .min_h_0()
                .child(Textarea::new(editor).h_full()),
            _ => div()
                .flex_1()
                .min_h_0()
                .text_color(muted)
                .child("Reading the file…"),
        };

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_4()
            .child(
                div()
                    .id("conflict-chunks")
                    .v_flex()
                    .gap_3()
                    .when(any_open, |list| list.flex_1().min_h(px(160.)))
                    .when(!any_open, |list| list.flex_none())
                    .overflow_y_scroll()
                    .children(cards),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h(px(200.))
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .gap_3()
                            .child(div().font_semibold().child("Result"))
                            .child(
                                div()
                                    .text_size(TEXT_SMALL)
                                    .text_color(subtext)
                                    .child("The whole file. Pick above, or edit it by hand."),
                            ),
                    )
                    .child(editor),
            )
    }

    /// One conflict: both sides next to each other, with the lines to pick from and the
    /// buttons that put them in the result.
    fn render_chunk(
        &self,
        progress: &Progress,
        ordinal: usize,
        total: usize,
        decision: &Decision,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let other = lane_color(self.refs.branch_color(&progress.other), is_dark);
        let picked = self
            .conflict
            .lines
            .iter()
            .filter(|(at, ..)| *at == ordinal)
            .count();

        let button = |id: &'static str, label: String, pick: Pick| {
            Button::new((id, ordinal))
                .ghost()
                .small()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.use_chunk(ordinal, pick, cx)))
        };
        let header = div()
            .h_flex()
            .h(px(38.))
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .text_size(TEXT_SMALL)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(muted)
                    .child(format!(
                        "Conflict {} of {total} · line {}",
                        ordinal + 1,
                        decision.line
                    )),
            )
            .child(button(
                "use-mine",
                format!("Use {}", progress.mine),
                Pick::Mine,
            ))
            .child(button(
                "use-other",
                format!("Use {}", progress.other),
                Pick::Other,
            ))
            .when(picked > 0, |header| {
                header.child(button(
                    "use-lines",
                    conflict::picked_lines_label(picked),
                    Pick::Lines,
                ))
            });

        div()
            .v_flex()
            .flex_none()
            .rounded(px(8.))
            .overflow_hidden()
            .bg(panel)
            .border_1()
            .border_color(border)
            .child(header)
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .child(self.render_chunk_side(
                        ordinal,
                        Side::Mine,
                        &progress.mine,
                        cx.theme().primary,
                        decision,
                        cx,
                    ))
                    .child(div().w(px(1.)).flex_none().bg(border))
                    .child(self.render_chunk_side(
                        ordinal,
                        Side::Other,
                        &progress.other,
                        other,
                        decision,
                        cx,
                    )),
            )
    }

    /// One side of a conflict. Each line can be ticked to pick it for the result.
    fn render_chunk_side(
        &self,
        ordinal: usize,
        side: Side,
        branch: &str,
        color: Hsla,
        decision: &Decision,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let foreground = theme.foreground;
        let subtext = tokens(theme.is_dark()).subtext0;
        let (text, why) = match side {
            Side::Mine => (&decision.mine, &decision.mine_why),
            Side::Other => (&decision.other, &decision.other_why),
        };
        let (row_id, box_id) = match side {
            Side::Mine => ("pick-mine-line", "pick-mine-box"),
            Side::Other => ("pick-other-line", "pick-other-box"),
        };
        let lines = conflict::lines_of(text);
        let rows: Vec<Stateful<Div>> = lines
            .iter()
            .enumerate()
            .map(|(at, line)| {
                let key = (ordinal, side, at);
                let on = self.conflict.lines.contains(&key);
                let id = ordinal * 10_000 + at;
                div()
                    .id((row_id, id))
                    .h_flex()
                    .h(px(22.))
                    .px_3()
                    .gap_2()
                    .cursor_pointer()
                    .text_size(TEXT_SMALL)
                    .text_color(foreground)
                    .when(on, |row| row.bg(color.opacity(0.14)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_line(key, cx)))
                    .child(checkbox((box_id, id), on, false, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(expand_tabs(line.trim_end_matches('\n'))),
                    )
            })
            .collect();

        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .v_flex()
                    .px_3()
                    .py_2()
                    .gap(px(2.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .font_semibold()
                            .text_size(TEXT_SMALL)
                            .text_color(color)
                            .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                            .child(div().min_w_0().truncate().child(branch.to_owned())),
                    )
                    .when(!why.is_empty(), |heading| {
                        heading.child(
                            div()
                                .text_size(TEXT_CHIP)
                                .text_color(subtext)
                                .child(why.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .v_flex()
                    .py_1()
                    .children(rows)
                    .when(lines.is_empty(), |body| {
                        body.child(
                            div()
                                .h(px(22.))
                                .px_3()
                                .text_size(TEXT_SMALL)
                                .text_color(muted)
                                .child("Nothing on this side"),
                        )
                    }),
            )
    }
}
