//! A paused rebase, cherry-pick, revert or merge. The inspector lists the files that need
//! a decision, and the resolver opens over the graph with both sides next to each other.

use super::notice::Confirm;
use super::parts::{TEXT_CHIP, TEXT_SMALL, expand_tabs, file_path, panel_row, section_label};
use super::parts::{pill, title};
use super::{GraphView, Notice};
use crate::theme::{lane_color, tokens};
use gitclient_core::conflict::{self, Decision, Progress, Side};
use gitclient_core::text::count;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    progress: Option<Arc<Progress>>,
    /// The decision on show.
    decision: usize,
    /// Whether the resolver is open over the graph.
    pub(super) open: bool,
    /// The result chosen for each decision so far.
    answers: Vec<Option<String>>,
    picked: Vec<Option<Side>>,
    editor: Option<Entity<TextareaState>>,
    /// Which decision the editor's text belongs to.
    editing: Option<usize>,
    /// Set while resolving, so that the check afterwards can tell finished from cancelled.
    continuing: Option<Arc<Progress>>,
    rebase_target: Option<String>,
    _editor_changes: Option<Subscription>,
}

impl GraphView {
    pub fn paused_operation(&self) -> Option<&Arc<Progress>> {
        self.conflict.progress.as_ref()
    }

    /// Whether the two sides of a conflict are open over the graph.
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
            Some(progress) => {
                if self.conflict.progress.as_deref() != Some(&progress) {
                    let count = progress.decisions.len();
                    self.conflict.decision = 0;
                    self.conflict.answers = vec![None; count];
                    self.conflict.picked = vec![None; count];
                    self.conflict.editing = None;
                    self.conflict.open = false;
                    self.conflict.progress = Some(Arc::new(progress));
                    self.focus_conflicts(cx);
                }
            }
            None => {
                self.conflict.progress = None;
                self.conflict.open = false;
                if let Some(finished) = continuing.filter(|_| !failed) {
                    self.notice = Some(Notice::Info(finished.finished_sentence().into()));
                }
            }
        }
        cx.notify();
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

    /// Opens the resolver on the first decision of a file.
    pub fn show_conflict(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(first) = self
            .conflict
            .progress
            .as_ref()
            .and_then(|progress| progress.decisions.iter().position(|d| d.path == path))
        else {
            return;
        };
        self.conflict.decision = first;
        self.conflict.open = true;
        // Only one thing covers the graph at a time.
        self.close_diff(cx);
        cx.notify();
    }

    pub(super) fn close_resolver(&mut self, cx: &mut Context<Self>) {
        self.conflict.open = false;
        cx.notify();
    }

    /// Takes one side for every decision in a file.
    pub(super) fn use_side_for_file(&mut self, path: &str, side: Side, cx: &mut Context<Self>) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        for (index, decision) in progress.decisions.iter().enumerate() {
            if decision.path == path {
                self.conflict.answers[index] = Some(decision.side(side).to_owned());
                self.conflict.picked[index] = Some(side);
            }
        }
        // So that the editor reads the answer again the next time it is drawn.
        self.conflict.editing = None;
        cx.notify();
    }

    /// Fills the decision on show from one side.
    fn pick_side(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        let index = self.conflict.decision;
        let Some(decision) = progress.decisions.get(index) else {
            return;
        };
        let text = decision.side(side).to_owned();
        self.conflict.picked[index] = Some(side);
        if let Some(editor) = &self.conflict.editor {
            let shown = text.strip_suffix('\n').unwrap_or(&text).to_owned();
            editor.update(cx, |editor, cx| editor.set_value(shown, window, cx));
        }
        self.conflict.answers[index] = Some(text);
        cx.notify();
    }

    fn show_decision(&mut self, index: usize, cx: &mut Context<Self>) {
        self.conflict.decision = index;
        cx.notify();
    }

    fn editor_changed(&mut self, editor: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let Some(index) = self.conflict.editing else {
            return;
        };
        let value = editor.read(cx).value();
        let trimmed = value.trim();
        let answer = self.conflict.answers.get_mut(index);
        let picked = self.conflict.picked.get(index).copied().flatten();
        // Picking a side that is empty leaves the editor blank but is still an answer.
        if let Some(answer) = answer
            && (picked.is_none() || !trimmed.is_empty())
        {
            *answer = (!trimmed.is_empty()).then(|| value.to_string());
        }
        cx.notify();
    }

    /// Keeps the editor on the decision on show while the resolver is open, making it the
    /// first time it's needed.
    pub(super) fn sync_conflict_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(progress) = self
            .conflict
            .progress
            .clone()
            .filter(|_| self.conflict.open)
        else {
            return;
        };
        let index = self.conflict.decision;
        if progress.decisions.get(index).is_none_or(|d| d.whole_file) {
            return;
        }
        let editor = match &self.conflict.editor {
            Some(editor) => editor.clone(),
            None => {
                let editor = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .placeholder("Pick a side above, or type the code you want here.")
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
        if self.conflict.editing != Some(index) {
            let text = self.conflict.answers.get(index).cloned().flatten();
            let shown = text.unwrap_or_default();
            let shown = shown.strip_suffix('\n').unwrap_or(&shown).to_owned();
            self.conflict.editing = None;
            editor.update(cx, |editor, cx| editor.set_value(shown, window, cx));
            self.conflict.editing = Some(index);
        }
    }

    pub(super) fn continue_operation(&mut self, cx: &mut Context<Self>) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        let answers: Option<Vec<String>> = self.conflict.answers.iter().cloned().collect();
        let Some(answers) = answers else {
            return;
        };
        self.conflict.continuing = Some(progress.clone());
        let label = format!("Continuing {}", progress.kind.title().to_lowercase());
        self.run_operation(
            label,
            move |repo| conflict::finish(repo, &progress, &answers).map(|()| None),
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
        let (border, muted) = (theme.border, theme.muted_foreground);
        let kind = progress.kind.title().to_lowercase();
        let ready = self.conflict.answers.iter().all(Option::is_some);
        let busy = self.busy.is_some();
        let files = progress.files();
        let shown_path = self
            .conflict
            .open
            .then(|| progress.decisions.get(self.conflict.decision))
            .flatten()
            .map(|decision| decision.path.clone());

        let rows: Vec<_> = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let answered = file
                    .decisions
                    .iter()
                    .filter(|at| self.conflict.answers.get(**at).is_some_and(Option::is_some))
                    .count();
                let decided = answered >= file.decisions.len();
                let (mark, mark_color) = if decided {
                    ("✓", theme_success(cx))
                } else {
                    ("!", colors.peach)
                };
                let path = file.path.clone();
                let menu_path = file.path.clone();
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
                        .text_color(mark_color)
                        .child(mark),
                )
                .child(file_path(&file.path, muted))
                .child(
                    div()
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child(file.note(answered)),
                )
            })
            .collect();

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
                    .child(section_label(muted).h(px(24.)).px_2().h_flex().child(
                        if files.is_empty() {
                            "Nothing to decide".to_owned()
                        } else {
                            format!("Conflicts · {}", count(files.len(), "file"))
                        },
                    ))
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

    /// The two sides of a conflict next to each other, over the graph.
    pub(super) fn render_resolver(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.conflict.open {
            return None;
        }
        let progress = self.conflict.progress.clone()?;
        let decision = progress.decisions.get(self.conflict.decision)?.clone();
        Some(
            self.render_decision(&progress, &decision, cx)
                .into_any_element(),
        )
    }

    fn render_decision(
        &self,
        shown: &Progress,
        decision: &Decision,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (head, subtext, panel) = (theme.primary, tokens(is_dark).subtext0, theme.title_bar);
        let other = lane_color(self.refs.branch_color(&shown.other), is_dark);
        let index = self.conflict.decision;
        let count = shown.decisions.len();
        let picked = self.conflict.picked.get(index).copied().flatten();
        let location = if decision.whole_file {
            decision.path.clone()
        } else {
            format!("{}:{}", decision.path, decision.line)
        };
        let headline = if decision.whole_file {
            "Both branches changed this file"
        } else {
            "Both branches changed this code"
        };
        let mine = SideView {
            side: Side::Mine,
            branch: shown.mine.clone(),
            color: head,
            code: &decision.mine,
            why: &decision.mine_why,
            picked: picked == Some(Side::Mine),
        };
        let theirs = SideView {
            side: Side::Other,
            branch: shown.other.clone(),
            color: other,
            code: &decision.other,
            why: &decision.other_why,
            picked: picked == Some(Side::Other),
        };
        let cards = [self.render_side(mine, cx), self.render_side(theirs, cx)];
        let result = (!decision.whole_file).then(|| self.render_result(index, cx));

        div()
            .id("decision")
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p_4()
            .gap_4()
            .bg(panel)
            .overflow_y_scrollbar()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(title().child(headline))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_SMALL)
                            .text_color(subtext)
                            .child(location),
                    )
                    .when(count > 1, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(TEXT_SMALL)
                                .text_color(subtext)
                                .child(format!("{} of {count}", index + 1)),
                        )
                        .child(
                            Button::new("previous-decision")
                                .ghost()
                                .small()
                                .label("Previous")
                                .disabled(index == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.show_decision(index.saturating_sub(1), cx);
                                })),
                        )
                        .child(
                            Button::new("next-decision")
                                .ghost()
                                .small()
                                .label("Next")
                                .disabled(index + 1 == count)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.show_decision(index + 1, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("close-resolver")
                            .ghost()
                            .small()
                            .label("Close")
                            .on_click(cx.listener(|this, _, _, cx| this.close_resolver(cx))),
                    ),
            )
            .child(div().h_flex().items_stretch().gap_3().children(cards))
            .children(result)
    }

    fn render_side(&self, view: SideView, cx: &mut Context<Self>) -> Div {
        let SideView {
            side,
            branch,
            color,
            code,
            why,
            picked,
        } = view;
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (background, border, foreground) = (theme.background, theme.border, theme.foreground);
        let on_color = theme.primary_foreground;
        let subtext = tokens(is_dark).subtext0;
        let lines = code.lines().map(|line| {
            div()
                .h(px(22.))
                .whitespace_nowrap()
                .child(expand_tabs(line))
        });
        let id = match side {
            Side::Mine => "use-mine",
            Side::Other => "use-other",
        };

        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .rounded(px(8.))
            .overflow_hidden()
            .bg(background)
            .border(px(if picked { 2. } else { 1. }))
            .border_color(if picked { color } else { border })
            .child(
                div()
                    .v_flex()
                    .p_3()
                    .gap(px(2.))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .font_semibold()
                            .text_color(color)
                            .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                            .child(div().min_w_0().truncate().child(branch.clone())),
                    )
                    .when(!why.is_empty(), |heading| {
                        heading.child(div().text_color(subtext).child(why.to_owned()))
                    }),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .px_3()
                    .py_2()
                    .overflow_hidden()
                    .text_size(TEXT_SMALL)
                    .text_color(foreground)
                    .border_t_1()
                    .border_color(border)
                    .children(lines),
            )
            .child(
                div().p_2().border_t_1().border_color(border).child(
                    div()
                        .id(id)
                        .h_flex()
                        .h(px(30.))
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .font_semibold()
                        .bg(if picked { color } else { border })
                        .text_color(if picked { on_color } else { foreground })
                        .child(div().min_w_0().truncate().child(format!("Use {branch}")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_side(side, window, cx);
                        })),
                ),
            )
    }

    fn render_result(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (success, peach) = (theme.success, tokens(theme.is_dark()).peach);
        let answered = self
            .conflict
            .answers
            .get(index)
            .is_some_and(Option::is_some);
        let (hint, hint_color) = if answered {
            ("Ready. Edit it here if you need both.", success)
        } else {
            ("Choose what the code should be to continue", peach)
        };
        div()
            .v_flex()
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
                            .text_color(hint_color)
                            .child(hint),
                    ),
            )
            .children(
                self.conflict
                    .editor
                    .as_ref()
                    .map(|editor| div().h(px(160.)).child(Textarea::new(editor).h_full())),
            )
    }
}

fn theme_success(cx: &App) -> Hsla {
    cx.theme().success
}

/// One side of a decision, as a card.
struct SideView<'a> {
    side: Side,
    branch: String,
    color: Hsla,
    code: &'a str,
    why: &'a str,
    picked: bool,
}
