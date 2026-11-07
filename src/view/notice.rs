//! The band under the header, and the runner for git operations that report through it.
//!
//! Operations that change a repository run off the UI thread, one at a time. Whatever
//! happens is said here in words, and risky steps wait here for a second click.

use super::GraphView;
use super::parts::TEXT_BODY;
use crate::theme::tokens;
use gitclient_core::menu::Weight;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How soon the second Enter must follow the first when a deletion needs two.
const DOUBLE_PRESS: Duration = Duration::from_millis(600);
const POPUP_WIDTH: Pixels = px(420.);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    Info(SharedString),
    Error(SharedString),
}

type ConfirmAction = Rc<dyn Fn(&mut GraphView, &mut Context<GraphView>)>;

/// A risky step waiting for confirmation. The message says how to undo it.
#[derive(Clone)]
pub(super) struct Confirm {
    pub message: SharedString,
    pub action: SharedString,
    pub run: ConfirmAction,
    pub weight: Weight,
    /// When the first of two Enter presses landed, while the second is still welcome.
    pub armed: Option<Instant>,
}

impl Confirm {
    /// A question in the band under the header.
    pub fn banner(
        message: impl Into<SharedString>,
        action: impl Into<SharedString>,
        run: impl Fn(&mut GraphView, &mut Context<GraphView>) + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            action: action.into(),
            run: Rc::new(run),
            weight: Weight::Banner,
            armed: None,
        }
    }

    fn is_popup(&self) -> bool {
        self.weight != Weight::Banner
    }
}

impl GraphView {
    /// Runs a git operation in the background, then reloads. `operation` returns what to
    /// tell the user on success, if anything worth saying.
    pub fn run_operation(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Path) -> Result<Option<String>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(workdir) = self.workdir() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
        self.notice = None;
        self.confirm = None;
        cx.notify();
        Self::spawn_blocking(move || operation(&workdir), Self::finish_operation, cx);
    }

    pub(super) fn finish_operation(
        &mut self,
        result: Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) {
        self.busy = None;
        self.notice = match result {
            Ok(message) => message.map(|message| Notice::Info(message.into())),
            Err(message) => Some(Notice::Error(message.into())),
        };
        self.reload(cx);
    }

    pub(super) fn render_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let (border, header, text) = (theme.border, theme.title_bar, theme.secondary_foreground);
        let peach = tokens(theme.is_dark()).peach;
        let band = |id: &'static str| {
            div()
                .id(id)
                .h_flex()
                .flex_none()
                .min_h(px(36.))
                .px_4()
                .py_1()
                .gap_3()
                .text_size(TEXT_BODY)
                .bg(header)
                .border_b_1()
                .border_color(border)
        };

        if let Some(confirm) = self.confirm.as_ref().filter(|confirm| !confirm.is_popup()) {
            let run = confirm.run.clone();
            return Some(
                band("confirm")
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(peach)
                            .child(confirm.message.clone()),
                    )
                    .child(
                        Button::new("confirm-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("confirm-run")
                            .ghost()
                            .small()
                            .label(confirm.action.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm = None;
                                run(this, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }

        let (message, color) = match self.notice.as_ref()? {
            Notice::Info(message) => (message.clone(), text),
            Notice::Error(message) => (message.clone(), peach),
        };
        Some(
            band("notice")
                .child(div().flex_1().min_w_0().text_color(color).child(message))
                .child(
                    Button::new("notice-dismiss")
                        .ghost()
                        .small()
                        .label("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

impl GraphView {
    /// Enter: answers a confirmation popup, or else chooses the menu line.
    pub(super) fn press_enter(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.as_mut().filter(|confirm| confirm.is_popup()) else {
            self.activate_menu(cx);
            return;
        };
        // A key held down repeats, which must never answer a question the user hasn't read.
        if self.key_held {
            return;
        }
        let armed = confirm.armed.is_some_and(|at| at.elapsed() <= DOUBLE_PRESS);
        if confirm.weight == Weight::DoublePress && !armed {
            let at = Instant::now();
            confirm.armed = Some(at);
            cx.notify();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DOUBLE_PRESS).await;
                this.update(cx, |this, cx| {
                    if let Some(confirm) = &mut this.confirm
                        && confirm.armed == Some(at)
                    {
                        confirm.armed = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        self.run_confirmed(cx);
    }

    /// Escape: closes a confirmation popup, or else the menu, or else the remote form.
    pub(super) fn press_escape(&mut self, cx: &mut Context<Self>) {
        if self.confirm.as_ref().is_some_and(Confirm::is_popup) {
            self.confirm = None;
            cx.notify();
        } else {
            self.dismiss(cx);
        }
    }

    fn run_confirmed(&mut self, cx: &mut Context<Self>) {
        if let Some(confirm) = self.confirm.take() {
            (confirm.run)(self, cx);
        }
    }

    /// The small popup that asks before something is removed.
    pub(super) fn render_confirm_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm.as_ref().filter(|confirm| confirm.is_popup())?;
        let theme = cx.theme();
        let (panel, border, text, muted) = (
            theme.title_bar,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let armed = confirm.armed.is_some();
        let danger = theme.danger;
        let hint = confirm.weight.hint(&confirm.action, armed);

        let cancel = |this: &mut Self,
                      _: &MouseDownEvent,
                      _: &mut Window,
                      cx: &mut Context<Self>| { this.press_escape(cx) };
        let card = div()
            .id("confirm-popup")
            .v_flex()
            .w(POPUP_WIDTH)
            .max_w_full()
            .p_4()
            .gap_3()
            .rounded(px(8.))
            .bg(panel)
            .border_1()
            .border_color(border)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .text_size(TEXT_BODY)
            .child(div().text_color(text).child(confirm.message.clone()))
            .child(
                div()
                    .text_color(if armed { danger } else { muted })
                    .child(hint),
            )
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("confirm-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.press_escape(cx))),
                    )
                    .child(
                        Button::new("confirm-run")
                            .ghost()
                            .small()
                            .label(confirm.action.clone())
                            .on_click(cx.listener(|this, _, _, cx| this.run_confirmed(cx))),
                    ),
            );

        Some(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .h_flex()
                    .justify_center()
                    .items_center()
                    .px_4()
                    .on_mouse_down(MouseButton::Left, cx.listener(cancel))
                    .on_mouse_down(MouseButton::Right, cx.listener(cancel))
                    .child(card),
            )
            .with_priority(20)
            .into_any_element(),
        )
    }
}
