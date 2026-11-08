//! Popups centred over the window that wait for an answer: the question before something
//! is removed, and the forms that ask for a name. They share one frame and one set of
//! keys. Enter answers, Escape or a click outside cancels, and a key held down never
//! answers, so nothing happens that the user hasn't read.

mod ask;
mod confirm;
mod form;

pub(super) use ask::AskDialog;
pub(super) use confirm::Confirm;
pub(super) use form::FormDialog;

use super::GraphView;
use super::parts::TEXT_BODY;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const WIDTH: Pixels = px(420.);

/// The popup on show. There is at most one, and it takes the keys while it is open.
pub(super) enum Dialog {
    Confirm(Confirm),
    Form(FormDialog),
    Ask(AskDialog),
}

/// What a popup puts in the shared frame, above the line naming the keys and the buttons.
struct Frame {
    content: Vec<AnyElement>,
    hint: SharedString,
    /// Muted, or red when the next press acts at once on something that can't be undone.
    hint_color: Hsla,
    /// The button that answers, named for its result.
    action: SharedString,
}

impl GraphView {
    /// Enter: answers the popup, or else chooses the menu line.
    pub(super) fn press_enter(&mut self, cx: &mut Context<Self>) {
        match &self.dialog {
            None => self.activate_menu(cx),
            // A key held down repeats, which must never answer a question the user hasn't
            // read, nor send a form they haven't finished.
            Some(_) if self.key_held => {}
            Some(Dialog::Confirm(_)) => self.press_enter_to_confirm(cx),
            Some(Dialog::Form(_)) => self.submit_form(cx),
            Some(Dialog::Ask(_)) => self.submit_ask(cx),
        }
    }

    /// Escape: closes the popup, or else whatever is open over the graph.
    pub(super) fn press_escape(&mut self, cx: &mut Context<Self>) {
        if self.dialog.take().is_some() {
            cx.notify();
        } else {
            self.dismiss(cx);
        }
    }

    /// Closes the menu if one is open, otherwise the diff or the resolver.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_menu(cx);
        } else if self.diff_is_open() {
            self.close_diff(cx);
        } else if self.conflict.open {
            self.close_resolver(cx);
        }
    }

    /// The answering button. A click is never a held key, and it is deliberate enough not to
    /// need the second press that Enter does for the riskiest deletions.
    fn answer_dialog(&mut self, cx: &mut Context<Self>) {
        match &self.dialog {
            Some(Dialog::Confirm(_)) => self.run_confirmed(cx),
            Some(Dialog::Form(_)) => self.submit_form(cx),
            Some(Dialog::Ask(_)) => self.submit_ask(cx),
            None => {}
        }
    }

    pub(super) fn render_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let frame = match self.dialog.as_ref()? {
            Dialog::Confirm(confirm) => {
                // Enter and Escape are bound on this view, and a click elsewhere (a menu
                // line, a terminal) leaves the focus somewhere they don't reach.
                if !self.focus.is_focused(window) {
                    window.focus(&self.focus, cx);
                }
                confirm.frame(cx)
            }
            Dialog::Form(_) => {
                self.ensure_form_inputs(window, cx);
                let Some(Dialog::Form(form)) = &self.dialog else {
                    return None;
                };
                form.keep_focus(&self.focus, window, cx);
                form.frame(cx)
            }
            Dialog::Ask(_) => {
                self.ensure_ask_input(window, cx);
                let Some(Dialog::Ask(ask)) = &self.dialog else {
                    return None;
                };
                ask.keep_focus(&self.focus, window, cx);
                ask.frame(cx)
            }
        };
        Some(self.render_frame(frame, cx))
    }

    /// A card on Mantle in the middle of the window, over a backdrop that cancels on a click.
    fn render_frame(&self, frame: Frame, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (panel, border) = (theme.title_bar, theme.border);
        let cancel = |this: &mut Self,
                      _: &MouseDownEvent,
                      _: &mut Window,
                      cx: &mut Context<Self>| { this.press_escape(cx) };
        let card = div()
            .id("dialog")
            .v_flex()
            .w(WIDTH)
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
            .children(frame.content)
            .child(div().text_color(frame.hint_color).child(frame.hint))
            .child(
                div()
                    .h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("dialog-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.press_escape(cx))),
                    )
                    .child(
                        Button::new("dialog-answer")
                            .ghost()
                            .small()
                            .label(frame.action)
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.answer_dialog(cx))),
                    ),
            );

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
        .into_any_element()
    }
}
