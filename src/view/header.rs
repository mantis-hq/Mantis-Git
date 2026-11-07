//! The header: where you are in a sentence, and the one thing to do next.

use super::notice::Confirm;
use super::{GraphView, Status, repository_name};
use crate::status::{self, NextAction, Span};
use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

impl GraphView {
    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, header, muted) = (theme.border, theme.title_bar, theme.muted_foreground);
        let name = self.repo.as_deref().map(repository_name);
        let loading = self.status == Status::Loading || self.busy.is_some();
        let has_repo = self.status == Status::Loaded;
        let action = self.refs.status.next_action();

        div()
            .h_flex()
            .h(px(52.))
            .flex_none()
            .pl_4()
            .pr_3()
            .gap_4()
            .bg(header)
            .border_b_1()
            .border_color(border)
            .child(
                Button::new("switch-repository")
                    .ghost()
                    .label(name.unwrap_or_else(|| "Open repository".to_owned()))
                    .tooltip("Open another repository")
                    .on_click(cx.listener(|this, _, _, cx| this.prompt_for_repository(cx))),
            )
            .child(div().flex_none().w(px(1.)).h(px(20.)).bg(border))
            .child(self.render_sentence(cx))
            .when(loading, |bar| {
                bar.child(Spinner::new().small())
                    .when_some(self.busy.clone(), |bar, label| {
                        bar.child(div().flex_none().text_sm().text_color(muted).child(label))
                    })
            })
            .children(self.render_rebase_button(cx))
            .children(self.render_sidebar_toggle(cx))
            .when(has_repo && self.refs.status.remote.is_some(), |bar| {
                bar.child(
                    Button::new("fetch")
                        .ghost()
                        .label("Fetch")
                        .disabled(self.busy.is_some())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_operation("Fetching", status::fetch, cx);
                        })),
                )
            })
            .when_some(action.filter(|_| has_repo), |bar, action| {
                bar.child(self.render_action(action, cx))
            })
    }

    fn render_sentence(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let current = theme.primary;
        let other = crate::theme::lane_color(crate::graph::LaneColor::Branch(0), theme.is_dark());
        let spans = self.refs.status.sentence().into_iter().map(|span| {
            let (text, color) = match span {
                Span::Text(text) => (text, None),
                Span::Current(text) => (text, Some(current)),
                Span::Branch(text) => (text, Some(other)),
            };
            div()
                .flex_none()
                .whitespace_nowrap()
                .when_some(color, |span, color| span.text_color(color).font_semibold())
                .child(text)
        });

        div()
            .h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .text_size(px(15.))
            .text_color(crate::theme::tokens(theme.is_dark()).subtext0)
            .when(self.status == Status::Loaded, |line| line.children(spans))
    }

    fn render_action(&self, action: NextAction, cx: &mut Context<Self>) -> impl IntoElement {
        // Each screen has one filled button, and the inspector's is the next thing to do
        // while the working changes are selected.
        Button::new("next-action")
            .when(self.working_row_is_selected(), |button| button.ghost())
            .when(!self.working_row_is_selected(), |button| button.primary())
            .label(action.label())
            .disabled(self.busy.is_some())
            .on_click(cx.listener(move |this, _, _, cx| this.start_action(action.clone(), cx)))
    }

    /// Runs the header's action, stopping to explain first if it can lose work.
    pub fn start_action(&mut self, action: NextAction, cx: &mut Context<Self>) {
        if let NextAction::ForcePush { replaced, .. } = action {
            let upstream = self.refs.status.upstream.clone().unwrap_or_default();
            let branch = self.refs.status.branch.clone().unwrap_or_default();
            let commits = if replaced == 1 {
                "1 commit".to_owned()
            } else {
                format!("{replaced} commits")
            };
            self.confirm = Some(Confirm {
                message: format!(
                    "This replaces {commits} on {upstream}. To undo it, find them with \
                     `git reflog show {upstream}` and push {branch} back to that commit."
                )
                .into(),
                action: "Force push".into(),
                run: Rc::new(move |this, cx| this.run_action(action.clone(), cx)),
            });
            cx.notify();
            return;
        }
        self.run_action(action, cx);
    }

    fn run_action(&mut self, action: NextAction, cx: &mut Context<Self>) {
        let label = match action {
            NextAction::Pull { .. } => "Pulling",
            NextAction::Publish { .. } | NextAction::Push { .. } | NextAction::ForcePush { .. } => {
                "Pushing"
            }
        };
        self.run_operation(
            label,
            move |repo| status::run_next_action(repo, &action),
            cx,
        );
    }
}
