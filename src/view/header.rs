//! The header: where you are in a sentence, and the one thing to do next.

use super::notice::Confirm;
use super::parts::{header_bar, header_divider};
use super::{GraphView, Status, repository_name};
use gitclient_core::status::{self, NextAction, Span};
use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

impl GraphView {
    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let name = self.repo.as_deref().map(repository_name);
        let loading = self.status == Status::Loading || self.busy.is_some();
        let has_repo = self.status == Status::Loaded;
        let action = self.refs.status.next_action();

        header_bar(cx)
            .child(
                Button::new("switch-repository")
                    .ghost()
                    .label(name.unwrap_or_else(|| "Open repository".to_owned()))
                    .tooltip("Open another repository in a new tab")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(
                            super::workspace::OpenRepositoryInTab.boxed_clone(),
                            cx,
                        )
                    }),
            )
            .child(header_divider(cx))
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
            .child(
                Button::new("settings")
                    .ghost()
                    .label("Settings")
                    .tooltip("Edit your git config (Ctrl ,)")
                    .on_click(
                        cx.listener(|this, _, _, cx| super::settings::open(this.repo.clone(), cx)),
                    ),
            )
    }

    fn render_sentence(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let current = theme.primary;
        let other =
            crate::theme::lane_color(gitclient_core::graph::LaneColor::Branch(0), theme.is_dark());
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
            self.confirm = Some(Confirm {
                message: self.refs.status.force_push_warning(replaced).into(),
                action: "Force push".into(),
                run: Rc::new(move |this, cx| this.run_action(action.clone(), cx)),
            });
            cx.notify();
            return;
        }
        self.run_action(action, cx);
    }

    fn run_action(&mut self, action: NextAction, cx: &mut Context<Self>) {
        self.run_operation(
            action.progress_label(),
            move |repo| status::run_next_action(repo, &action),
            cx,
        );
    }
}
