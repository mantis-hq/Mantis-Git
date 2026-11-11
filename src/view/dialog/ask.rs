//! A question from git or ssh that waits for typing: a key's passphrase, an HTTPS login,
//! whether to trust a host. What it says lives in `mantisgit_core::askpass`; this draws
//! it in the shared frame, with the answer hidden when it is a secret. Dropping the popup
//! cancels the question, so Escape needs no code of its own.

use super::{Dialog, Frame};
use crate::view::GraphView;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mantisgit_core::askpass::Request;

/// An open question. Its input is made on the next draw, when there is a window.
pub(in crate::view) struct AskDialog {
    request: Option<Request>,
    input: Option<Entity<InputState>>,
}

impl AskDialog {
    pub(super) fn frame(&self, cx: &App) -> Frame {
        let theme = cx.theme();
        let request = self.request.as_ref();
        let mut content = vec![
            div()
                .text_color(theme.foreground)
                .child(request.map(Request::sentence).unwrap_or_default())
                .into_any_element(),
        ];
        content.extend(
            self.input
                .as_ref()
                .map(|input| Input::new(input).into_any_element()),
        );
        Frame {
            content,
            hint: request.map(Request::hint).unwrap_or_default().into(),
            hint_color: theme.muted_foreground,
            action: request.map_or("Answer", Request::action).into(),
        }
    }

    pub(super) fn keep_focus(&self, view: &FocusHandle, window: &mut Window, cx: &mut App) {
        if let Some(input) = &self.input
            && !input.focus_handle(cx).is_focused(window)
            && !view.is_focused(window)
        {
            window.focus(&input.focus_handle(cx), cx);
        }
    }
}

impl GraphView {
    /// Shows `request` over the window, in place of any other popup. The popup it replaces
    /// is closed, and a question it replaces is cancelled, which git reports as a failure.
    pub fn open_ask(&mut self, request: Request, cx: &mut Context<Self>) {
        self.menu = None;
        self.dialog = Some(Dialog::Ask(AskDialog {
            request: Some(request),
            input: None,
        }));
        cx.notify();
    }

    pub(super) fn ensure_ask_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Ask(dialog)) = &self.dialog else {
            return;
        };
        if dialog.input.is_some() {
            return;
        }
        let secret = dialog.request.as_ref().is_some_and(Request::is_secret);
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            if secret {
                state.set_masked(true, window, cx);
            }
            state
        });
        window.focus(&input.focus_handle(cx), cx);
        if let Some(Dialog::Ask(dialog)) = &mut self.dialog {
            dialog.input = Some(input);
        }
    }

    /// Enter or the button: sends what was typed to the waiting git and closes the popup.
    pub(super) fn submit_ask(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Ask(dialog)) = &mut self.dialog else {
            return;
        };
        let Some(input) = dialog.input.clone() else {
            return;
        };
        let value = input.read(cx).value().to_string();
        let request = dialog.request.take();
        self.dialog = None;
        if let Some(request) = request {
            request.answer(Some(value));
        }
        cx.notify();
    }
}
