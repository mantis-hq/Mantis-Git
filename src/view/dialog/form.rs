//! A popup that asks for a few words, such as a new branch's name or a remote's address.
//! What each form asks and what it runs lives in `gitclient_core::menu::form`; this draws
//! any of them, one input per field, in the shared frame.

use super::{Dialog, Frame};
use crate::theme::tokens;
use crate::view::GraphView;
use crate::view::parts::section_label;
use gitclient_core::menu::form::Form;
use gpui_kit::component::IndexPath;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

type PickState = SelectState<SearchableVec<String>>;

/// What one field of a form is drawn as: a typed input, or a dropdown with a search box
/// for a field that only takes what the repository already has.
enum Control {
    Text(Entity<InputState>),
    Pick(Entity<PickState>),
}

impl Control {
    fn value(&self, cx: &App) -> String {
        match self {
            Self::Text(input) => input.read(cx).value().to_string(),
            Self::Pick(select) => select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Text(input) => input.focus_handle(cx),
            Self::Pick(select) => select.focus_handle(cx),
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            Self::Text(input) => Input::new(input).into_any_element(),
            Self::Pick(select) => Select::new(select).into_any_element(),
        }
    }
}

/// An open form. Its inputs are made on the next draw, which is the first time there is a
/// window to make them in.
pub(in crate::view) struct FormDialog {
    pub form: Form,
    /// What the inputs start with.
    initial: Vec<String>,
    inputs: Vec<Control>,
    /// Why the last attempt was turned down, by the form's own checks or by git. Typing
    /// clears it.
    problem: Option<SharedString>,
    _events: Vec<Subscription>,
}

impl FormDialog {
    /// What the inputs hold now, in the order of the form's fields.
    fn values(&self, cx: &App) -> Vec<String> {
        if self.inputs.is_empty() {
            return self.initial.clone();
        }
        self.inputs.iter().map(|input| input.value(cx)).collect()
    }

    /// The index of the input with the cursor in it.
    fn focused(&self, window: &Window, cx: &App) -> Option<usize> {
        self.inputs
            .iter()
            .position(|input| input.focus_handle(cx).is_focused(window))
    }

    /// Puts the cursor back in the first input when it has wandered out of the form, as
    /// after a click on the card. Enter and Escape would otherwise go nowhere.
    pub(super) fn keep_focus(&self, view: &FocusHandle, window: &mut Window, cx: &mut App) {
        if self.focused(window, cx).is_none()
            && !view.is_focused(window)
            && let Some(first) = self.inputs.first()
        {
            window.focus(&first.focus_handle(cx), cx);
        }
    }

    /// Existing things a typed field could be filled with, as quiet buttons under the
    /// fields. Clicking one types it in.
    fn suggestions(&self, values: &[String], cx: &mut Context<GraphView>) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let (index, names) = (0..values.len())
            .map(|index| (index, self.form.suggestions(index, values)))
            .find(|(_, names)| !names.is_empty())?;
        let buttons = names.into_iter().enumerate().map(|(n, name)| {
            let fill = name.clone();
            Button::new(("suggestion", n))
                .ghost()
                .small()
                .label(name)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.fill_form_field(index, &fill, window, cx)
                }))
        });
        Some(
            div()
                .v_flex()
                .gap_1()
                .child(section_label(muted).child("Already there"))
                .child(div().h_flex().flex_wrap().gap_1().children(buttons))
                .into_any_element(),
        )
    }

    pub(super) fn frame(&self, cx: &mut Context<GraphView>) -> Frame {
        let theme = cx.theme();
        let (text, muted) = (theme.foreground, theme.muted_foreground);
        let peach = tokens(theme.is_dark()).peach;
        let values = self.values(cx);
        let mut content = vec![
            div()
                .text_color(text)
                .child(self.form.title())
                .into_any_element(),
        ];
        let fields = self
            .form
            .fields()
            .iter()
            .zip(&self.inputs)
            .map(|(field, control)| {
                div()
                    .v_flex()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(section_label(muted).child(field.label))
                    .child(control.element())
            });
        match self.form.separator() {
            // The fields share a row, as the two halves of `origin/main` do.
            Some(separator) => {
                let mut row = div().h_flex().items_end().gap_2();
                for (i, field) in fields.enumerate() {
                    if i > 0 {
                        row = row.child(
                            div()
                                .h(px(32.))
                                .items_center()
                                .text_color(muted)
                                .child(separator),
                        );
                    }
                    row = row.child(field);
                }
                content.push(row.into_any_element());
            }
            None => content.extend(fields.map(IntoElement::into_any_element)),
        }
        content.extend(self.suggestions(&values, cx));
        content.extend(
            self.problem
                .clone()
                .map(|problem| div().text_color(peach).child(problem).into_any_element()),
        );
        Frame {
            content,
            hint: self.form.hint(&values).into(),
            hint_color: muted,
            action: self.form.action(&values).into(),
        }
    }
}

#[cfg(test)]
impl GraphView {
    /// What the open form's inputs hold, or `None` when no form is open.
    pub(in crate::view) fn form_values(&self, cx: &App) -> Option<Vec<String>> {
        match &self.dialog {
            Some(Dialog::Form(dialog)) => Some(dialog.values(cx)),
            _ => None,
        }
    }

    /// Why the open form's last attempt was turned down.
    pub(in crate::view) fn form_problem(&self) -> Option<SharedString> {
        match &self.dialog {
            Some(Dialog::Form(dialog)) => dialog.problem.clone(),
            _ => None,
        }
    }
}

impl GraphView {
    /// Opens `form` over the window, in place of any menu or other popup.
    pub fn open_form(&mut self, form: Form, cx: &mut Context<Self>) {
        self.menu = None;
        self.dialog = Some(Dialog::Form(FormDialog {
            initial: form.initial_values(&self.refs),
            form,
            inputs: Vec::new(),
            problem: None,
            _events: Vec::new(),
        }));
        cx.notify();
    }

    /// Makes the form's inputs the first time it is drawn, and puts the cursor in the first.
    pub(super) fn ensure_form_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Form(dialog)) = &self.dialog else {
            return;
        };
        if !dialog.inputs.is_empty() {
            return;
        }
        let (form, initial) = (dialog.form.clone(), dialog.initial.clone());
        let mut inputs = Vec::new();
        let mut events = Vec::new();
        for (index, (field, value)) in form.fields().iter().zip(&initial).enumerate() {
            // Enter isn't taken here: it goes on to this view's own binding, as it does
            // from anywhere else in the window, so a form answers to the same keys as a
            // question.
            if form.is_pick(index) {
                let options = form.options(index, &self.refs);
                let at = options.iter().position(|option| option == value);
                let select = cx.new(|cx| {
                    SelectState::new(
                        SearchableVec::new(options),
                        at.map(IndexPath::new),
                        window,
                        cx,
                    )
                });
                events.push(cx.subscribe(
                    &select,
                    |this, _, _: &SelectEvent<SearchableVec<String>>, cx| this.form_changed(cx),
                ));
                inputs.push(Control::Pick(select));
            } else {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(field.placeholder)
                        .default_value(value.clone())
                });
                events.push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.form_changed(cx);
                    }
                }));
                inputs.push(Control::Text(input));
            }
        }
        if let Some(first) = inputs.first() {
            window.focus(&first.focus_handle(cx), cx);
        }
        if let Some(Dialog::Form(dialog)) = &mut self.dialog {
            dialog.inputs = inputs;
            dialog._events = events;
        }
    }

    /// Tab and Shift Tab: moves the cursor to the form's next or previous input, round.
    pub(in crate::view) fn step_field(&mut self, step: isize, window: &mut Window, cx: &mut App) {
        let Some(Dialog::Form(dialog)) = &self.dialog else {
            return;
        };
        let count = dialog.inputs.len() as isize;
        if count == 0 {
            return;
        }
        let at = dialog.focused(window, cx).map_or(-step, |at| at as isize);
        let next = (at + step).rem_euclid(count) as usize;
        window.focus(&dialog.inputs[next].focus_handle(cx), cx);
    }

    /// What was typed changed, so the button's name may have, and the last problem no
    /// longer applies.
    fn form_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(Dialog::Form(dialog)) = &mut self.dialog {
            dialog.problem = None;
        }
        cx.notify();
    }

    /// A suggestion was clicked: types it into the field in place of what was there.
    fn fill_form_field(
        &mut self,
        index: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Dialog::Form(dialog)) = &self.dialog else {
            return;
        };
        if let Some(Control::Text(input)) = dialog.inputs.get(index) {
            input.update(cx, |input, cx| input.set_value(text, window, cx));
            window.focus(&input.focus_handle(cx), cx);
        }
    }

    /// Runs what the form asks for. It stays open, with the button off, until git
    /// answers: on success it closes and the band says what happened, otherwise it says
    /// why in the form, so the words can be put right.
    pub(super) fn submit_form(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(workdir) = self.workdir() else {
            return;
        };
        let Some(Dialog::Form(dialog)) = &mut self.dialog else {
            return;
        };
        let values = dialog.values(cx);
        if let Some(problem) = dialog.form.problem(&values) {
            dialog.problem = Some(problem.into());
            cx.notify();
            return;
        }
        let form = dialog.form.clone();
        self.busy = Some(form.progress().into());
        self.notice = None;
        cx.notify();
        Self::spawn_blocking(
            {
                let form = form.clone();
                move || form.submit(&workdir, &values)
            },
            move |this, result, cx| this.finish_form(&form, result, cx),
            cx,
        );
    }

    fn finish_form(
        &mut self,
        form: &Form,
        result: Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) {
        let still_open = matches!(&self.dialog, Some(Dialog::Form(dialog)) if &dialog.form == form);
        match result {
            Err(problem) if still_open => {
                self.busy = None;
                if let Some(Dialog::Form(dialog)) = &mut self.dialog {
                    dialog.problem = Some(problem.into());
                }
                cx.notify();
            }
            // Closed while git ran, so whatever it said goes in the band.
            result => {
                if still_open {
                    self.dialog = None;
                }
                self.finish_operation(result, cx);
            }
        }
    }
}
