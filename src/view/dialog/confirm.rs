//! The question asked before a risky step. Rewrites ask in the band under the header;
//! deletions ask in a popup, and those that lose work nothing else keeps need Enter twice.

use super::{Dialog, Frame};
use crate::view::GraphView;
use gitclient_core::menu::command::Weight;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How soon the second Enter must follow the first when a deletion needs two.
const DOUBLE_PRESS: Duration = Duration::from_millis(600);

type ConfirmAction = Rc<dyn Fn(&mut GraphView, &mut Context<GraphView>)>;

/// A risky step waiting for confirmation. The message says how to undo it.
#[derive(Clone)]
pub(in crate::view) struct Confirm {
    pub message: SharedString,
    pub action: SharedString,
    pub run: ConfirmAction,
    pub weight: Weight,
    /// When the first of two Enter presses landed, while the second is still welcome.
    pub armed: Option<Instant>,
}

impl Confirm {
    pub fn new(
        message: impl Into<SharedString>,
        action: impl Into<SharedString>,
        weight: Weight,
        run: impl Fn(&mut GraphView, &mut Context<GraphView>) + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            action: action.into(),
            run: Rc::new(run),
            weight,
            armed: None,
        }
    }

    /// A question in the band under the header, for a step that rewrites history.
    pub fn banner(
        message: impl Into<SharedString>,
        action: impl Into<SharedString>,
        run: impl Fn(&mut GraphView, &mut Context<GraphView>) + 'static,
    ) -> Self {
        Self::new(message, action, Weight::Banner, run)
    }

    pub(super) fn frame(&self, cx: &App) -> Frame {
        let theme = cx.theme();
        let armed = self.armed.is_some();
        Frame {
            content: vec![
                div()
                    .text_color(theme.foreground)
                    .child(self.message.clone())
                    .into_any_element(),
            ],
            hint: self.weight.hint(&self.action, armed).into(),
            hint_color: if armed {
                theme.danger
            } else {
                theme.muted_foreground
            },
            action: self.action.clone(),
        }
    }
}

impl GraphView {
    /// Asks before `confirm` runs: in the band under the header, or in a popup.
    pub(in crate::view) fn ask(&mut self, confirm: Confirm, cx: &mut Context<Self>) {
        if confirm.weight == Weight::Banner {
            self.confirm = Some(confirm);
        } else {
            self.dialog = Some(Dialog::Confirm(confirm));
        }
        cx.notify();
    }

    /// Whether a risky step is waiting for confirmation, in the band or in a popup.
    pub fn is_confirming(&self) -> bool {
        self.confirm.is_some() || matches!(self.dialog, Some(Dialog::Confirm(_)))
    }

    pub(super) fn press_enter_to_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Confirm(confirm)) = &mut self.dialog else {
            return;
        };
        let armed = confirm.armed.is_some_and(|at| at.elapsed() <= DOUBLE_PRESS);
        if confirm.weight != Weight::DoublePress || armed {
            self.run_confirmed(cx);
            return;
        }
        let at = Instant::now();
        confirm.armed = Some(at);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DOUBLE_PRESS).await;
            this.update(cx, |this, cx| {
                if let Some(Dialog::Confirm(confirm)) = &mut this.dialog
                    && confirm.armed == Some(at)
                {
                    confirm.armed = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn run_confirmed(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.dialog, Some(Dialog::Confirm(_))) {
            return;
        }
        if let Some(Dialog::Confirm(confirm)) = self.dialog.take() {
            (confirm.run)(self, cx);
        }
    }
}
