//! The task screen for a paused rebase, cherry-pick, revert or merge.

use super::notice::Confirm;
use super::parts::{Edge, expand_tabs, fit, grip, header_bar, on_resize, section_label, title};
use super::{GraphView, Notice, Screen};
use crate::theme::{lane_color, tokens};
use gitclient_core::conflict::{self, Decision, Progress, Side, Step, StepState};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;
use std::sync::Arc;

const STEPS_WIDTH: Pixels = px(340.);

#[derive(Default)]
pub(super) struct State {
    progress: Option<Arc<Progress>>,
    /// The decision on show.
    decision: usize,
    /// The width the user dragged the steps panel to.
    steps_width: Option<f32>,
    /// The result chosen for each decision so far.
    answers: Vec<Option<String>>,
    picked: Vec<Option<Side>>,
    editor: Option<Entity<TextareaState>>,
    /// Which decision the editor's text belongs to.
    editing: Option<usize>,
    /// Set while resolving, so that the check afterwards can tell finished from cancelled.
    continuing: Option<Arc<Progress>>,
    finished: Option<Arc<Progress>>,
    rebase_target: Option<String>,
    _editor_changes: Option<Subscription>,
}

impl GraphView {
    pub fn paused_operation(&self) -> Option<&Arc<Progress>> {
        self.conflict.progress.as_ref()
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
                let same = self.conflict.progress.as_deref() == Some(&progress);
                if !same {
                    let count = progress.decisions.len();
                    self.conflict.decision = 0;
                    self.conflict.answers = vec![None; count];
                    self.conflict.picked = vec![None; count];
                    self.conflict.editing = None;
                    self.conflict.progress = Some(Arc::new(progress));
                    self.conflict.finished = None;
                    self.screen = Screen::Conflict;
                }
            }
            None => {
                self.conflict.progress = None;
                match continuing {
                    Some(finished) if !failed => {
                        self.conflict.finished = Some(finished);
                        self.screen = Screen::Conflict;
                    }
                    _ if self.screen == Screen::Conflict && self.conflict.finished.is_none() => {
                        self.screen = Screen::Graph;
                    }
                    _ => {}
                }
            }
        }
        cx.notify();
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
                    this.confirm = Some(Confirm {
                        message: conflict::rebase_warning(&target).into(),
                        action: "Rebase".into(),
                        run: Rc::new(move |this, cx| {
                            let target = target.clone();
                            this.run_operation(
                                "Rebasing",
                                move |repo| conflict::rebase_onto(repo, &target),
                                cx,
                            );
                        }),
                    });
                    cx.notify();
                }))
                .into_any_element(),
        )
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

    /// Keeps the editor on the decision on show, making it the first time it's needed.
    fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.conflict.decision;
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

    fn continue_operation(&mut self, cx: &mut Context<Self>) {
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

    /// Leaves the screen with the operation still paused, keeping a way back.
    fn leave_for_graph(&mut self, cx: &mut Context<Self>) {
        if let Some(progress) = &self.conflict.progress {
            self.confirm = Some(Confirm {
                message: progress.sentence().into(),
                action: "Resolve".into(),
                run: Rc::new(|this, cx| this.show_screen(Screen::Conflict, cx)),
            });
        }
        self.conflict.finished = None;
        self.show_screen(Screen::Graph, cx);
    }

    pub(super) fn render_conflict_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let progress = self.conflict.progress.clone();
        let finished = self.conflict.finished.clone();
        let Some(shown) = progress.as_ref().or(finished.as_ref()).cloned() else {
            return Empty.into_any_element();
        };
        let decision = shown.decisions.get(self.conflict.decision).cloned();
        let needs_editor = decision.as_ref().is_some_and(|d| !d.whole_file);
        if progress.is_some() && needs_editor {
            self.sync_editor(window, cx);
        }

        let body = div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .child(self.render_steps(&shown, finished.is_some(), cx))
            .child(grip(Edge::Steps, true, cx));
        let body = on_resize(body, Edge::Steps, cx, |this, pointer, bounds| {
            let max = f32::from(bounds.size.width) - 360.;
            this.conflict.steps_width = Some(fit(pointer.x - bounds.left(), 240., max));
        });
        let body = match decision.filter(|_| finished.is_none()) {
            Some(decision) => body.child(self.render_decision(&shown, &decision, cx)),
            None => body.child(div().flex_1()),
        };
        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .child(self.render_conflict_header(&shown, finished.is_some(), cx))
            .child(body)
            .into_any_element()
    }

    fn render_conflict_header(
        &self,
        shown: &Progress,
        finished: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let colors = tokens(theme.is_dark());
        let success = theme.success;
        let on_pill = theme.primary_foreground;
        let (pill, pill_color, sentence) = if finished {
            (
                format!("{} finished", shown.kind.title()),
                success,
                shown.finished_sentence(),
            )
        } else {
            (
                format!("{} paused", shown.kind.title()),
                colors.peach,
                shown.sentence(),
            )
        };
        let ready = self.conflict.answers.iter().all(Option::is_some);
        let busy = self.busy.is_some();

        header_bar(cx)
            .child(
                div()
                    .h_flex()
                    .flex_none()
                    .h(px(22.))
                    .px_2()
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .font_semibold()
                    .bg(pill_color)
                    .text_color(on_pill)
                    .child(pill),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .text_color(colors.subtext0)
                    .child(sentence),
            )
            .when(finished, |bar| {
                bar.child(
                    Button::new("back-to-graph")
                        .primary()
                        .label("Back to the graph")
                        .on_click(cx.listener(|this, _, _, cx| this.leave_for_graph(cx))),
                )
            })
            .when(!finished, |bar| {
                let kind = shown.kind.title().to_lowercase();
                bar.child(
                    Button::new("graph")
                        .ghost()
                        .label("Graph")
                        .on_click(cx.listener(|this, _, _, cx| this.leave_for_graph(cx))),
                )
                .child(
                    Button::new("cancel-operation")
                        .ghost()
                        .label(format!("Cancel {kind}"))
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_operation(cx))),
                )
                .child(
                    Button::new("continue-operation")
                        .primary()
                        .label(format!("Continue {kind}"))
                        .disabled(busy || !ready)
                        .on_click(cx.listener(|this, _, _, cx| this.continue_operation(cx))),
                )
            })
    }

    fn render_steps(
        &self,
        shown: &Progress,
        finished: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (border, panel, muted) = (theme.border, theme.title_bar, theme.muted_foreground);
        let heading = shown.steps_heading();
        let steps: Vec<_> = shown
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let last = index + 1 == shown.steps.len();
                render_step(step, finished, last, cx)
            })
            .collect();

        div()
            .v_flex()
            .w(self.conflict.steps_width.map_or(STEPS_WIDTH, px))
            .flex_none()
            .p_4()
            .gap_3()
            .bg(panel)
            .border_r_1()
            .border_color(border)
            .child(section_label(muted).child(heading))
            .child(div().v_flex().flex_1().min_h_0().children(steps))
            .when(!finished, |column| {
                column.child(
                    div()
                        .text_size(px(12.))
                        .text_color(muted)
                        .child(shown.cancel_note()),
                )
            })
    }

    fn render_decision(
        &self,
        shown: &Progress,
        decision: &Decision,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (head, subtext) = (theme.primary, tokens(is_dark).subtext0);
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
            .p_4()
            .gap_4()
            .overflow_y_scrollbar()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(title().child(headline))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(subtext)
                            .child(location),
                    )
                    .when(count > 1, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
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
                    }),
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
        let (panel, border, foreground) = (theme.title_bar, theme.border, theme.foreground);
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
            .bg(panel)
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
                    .text_size(px(12.))
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
                    .child(div().text_size(px(12.)).text_color(hint_color).child(hint)),
            )
            .children(
                self.conflict
                    .editor
                    .as_ref()
                    .map(|editor| div().h(px(160.)).child(Textarea::new(editor).h_full())),
            )
    }
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

fn render_step(step: &Step, finished: bool, last: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let colors = tokens(theme.is_dark());
    let (success, foreground, muted) = (theme.success, theme.foreground, theme.muted_foreground);
    let line = theme.input;
    let on_green = theme.primary_foreground;
    let state = if finished {
        StepState::Applied
    } else {
        step.state
    };
    let (note, fill, ring, ring_width, text) = match state {
        StepState::Applied => ("Applied", Some(success), success, 0., foreground),
        StepState::Paused => ("Paused here", None, colors.peach, 3., foreground),
        StepState::Waiting => ("Waiting", None, colors.overlay1, 1.5, colors.subtext0),
    };

    div()
        .h_flex()
        .items_start()
        .min_h(px(44.))
        .gap(px(10.))
        .child(
            div()
                .v_flex()
                .items_center()
                .self_stretch()
                .w(px(16.))
                .flex_none()
                .child(
                    div()
                        .h_flex()
                        .justify_center()
                        .size(px(16.))
                        .mt(px(1.))
                        .flex_none()
                        .rounded_full()
                        .when_some(fill, |dot, fill| dot.bg(fill))
                        .border(px(ring_width))
                        .border_color(ring)
                        .when(state == StepState::Applied, |dot| {
                            dot.text_color(on_green)
                                .text_size(px(10.))
                                .font_semibold()
                                .child("✓")
                        }),
                )
                .when(!last, |column| {
                    column.child(div().flex_1().w(px(2.)).min_h(px(8.)).bg(line))
                }),
        )
        .child(
            div()
                .v_flex()
                .min_w_0()
                .pb(px(10.))
                .child(
                    div()
                        .text_color(text)
                        .when(state == StepState::Paused, |title| title.font_semibold())
                        .child(step.subject.clone()),
                )
                .child(div().text_size(px(12.)).text_color(muted).child(note)),
        )
}
