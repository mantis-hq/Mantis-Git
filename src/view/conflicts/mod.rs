//! A paused rebase, cherry-pick, revert or merge. The inspector lists the files that need a
//! decision. A file opens in the resolver over the graph: its conflicts side by side, to take
//! whole or line by line, and the file's result in an editor, saved with one button.

mod resolver;

use resolver::FileView;

use super::dialog::Confirm;
use super::parts::{
    TEXT_CHIP, TEXT_SMALL, checkbox, expand_tabs, file_path, panel_row, pill, scroll_marks,
    section_label,
};
use super::{GraphView, Notice};
use crate::theme::{lane_color, tokens};
use gitclient_core::repo::conflict::{self, Decision, Piece, Progress, Side};
use gitclient_core::text::count;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

/// Rows are as tall as the diff's, so that a file reads the same in both.
const ROW_HEIGHT: Pixels = px(24.);
const NUMBER_WIDTH: Pixels = px(48.);
/// The lines of context around a conflict when only the changes are shown.
const CONTEXT_LINES: usize = 3;

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
    /// Each file laid out for its list, built in the background when it is opened.
    views: HashMap<String, Arc<FileView>>,
    /// For each file, which of its conflicts still stand in its result. Worked out when the
    /// result changes, so that drawing never has to read the file again.
    standing: HashMap<String, Vec<bool>>,
    scroll: UniformListScrollHandle,
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
        self.views.clear();
        self.standing.clear();
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
                    let confirm = Confirm::banner(
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
                    );
                    this.ask(confirm, cx);
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
        if self.is_whole_file(path) {
            return usize::from(!self.conflict.sides.contains_key(path));
        }
        match self.conflict.standing.get(path) {
            Some(open) => open.iter().filter(|open| **open).count(),
            None => self.file_decisions(path).len(),
        }
    }

    /// Works out which of a file's conflicts still stand in its result.
    fn refresh_open(&mut self, path: &str) {
        let (Some(progress), Some(draft)) =
            (&self.conflict.progress, self.conflict.drafts.get(path))
        else {
            return;
        };
        let open = progress
            .open_chunks(path, draft)
            .iter()
            .map(Option::is_some)
            .collect();
        self.conflict.standing.insert(path.to_owned(), open);
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
}
