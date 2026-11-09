//! A popup that asks for a few words, such as a new branch's name or a remote's address.
//! What each form asks and what it runs lives in `gitclient_core::menu::form`; this draws
//! any of them, one input per field, in the shared frame.

use super::{Dialog, Frame};
use crate::theme::tokens;
use crate::view::parts::section_label;
use crate::view::{GraphView, OpenWorktreeInTab};
use gitclient_core::menu::form::Form;
use gitclient_core::repo::clone;
use gitclient_core::settings::state::AppState;
use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

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

/// Where the cursor starts: the first field you type in, since a dropdown that has the
/// cursor takes Enter for itself and the form could not be answered from the keyboard.
fn start_at(controls: &[Control]) -> Option<&Control> {
    controls
        .iter()
        .find(|control| matches!(control, Control::Text(_)))
        .or(controls.first())
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
            && let Some(first) = start_at(&self.inputs)
        {
            window.focus(&first.focus_handle(cx), cx);
        }
    }

    /// The list that drops under a typed field while it has the cursor: existing things the
    /// field could be filled with, narrowed by what is typed. A click types one in, and
    /// typing something else is still allowed.
    fn dropdown(
        &self,
        index: usize,
        values: &[String],
        window: &Window,
        cx: &mut Context<GraphView>,
    ) -> Option<AnyElement> {
        let Some(Control::Text(input)) = self.inputs.get(index) else {
            return None;
        };
        let names = self.form.suggestions(index, values);
        if names.is_empty() || !input.focus_handle(cx).is_focused(window) {
            return None;
        }
        let theme = cx.theme();
        let (panel, border, hover) = (theme.title_bar, theme.border, theme.list_active);
        let rows = names.into_iter().enumerate().map(|(n, name)| {
            let fill = name.clone();
            div()
                .id(("suggestion", n))
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(div().truncate().child(name))
                // On press, not on release: the press is what takes the cursor out of the
                // field, and the list goes with it.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.fill_form_field(index, &fill, window, cx)
                    }),
                )
        });
        Some(
            deferred(
                div()
                    .absolute()
                    .top_full()
                    .left_0()
                    .right_0()
                    .mt_1()
                    .v_flex()
                    .p_1()
                    .rounded(px(6.))
                    .bg(panel)
                    .border_1()
                    .border_color(border)
                    .occlude()
                    .children(rows),
            )
            .with_priority(30)
            .into_any_element(),
        )
    }

    pub(super) fn frame(&self, window: &Window, cx: &mut Context<GraphView>) -> Frame {
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
        let mut fields = Vec::new();
        for (index, (field, control)) in self.form.fields().iter().zip(&self.inputs).enumerate() {
            fields.push(
                div()
                    .v_flex()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(section_label(muted).child(field.label))
                    .child(
                        div()
                            .relative()
                            .child(control.element())
                            .children(self.dropdown(index, &values, window, cx)),
                    ),
            );
        }
        match self.form.separator() {
            // The fields share a row, as the two halves of `origin/main` do.
            Some(separator) => {
                let mut row = div().h_flex().items_end().gap_2();
                for (i, field) in fields.into_iter().enumerate() {
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
            None => content.extend(fields.into_iter().map(IntoElement::into_any_element)),
        }
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
        if let Some(first) = start_at(&inputs) {
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
        let open = self.workdir();
        let Some(Dialog::Form(dialog)) = &mut self.dialog else {
            return;
        };
        // A clone is the one form that needs no repository to be open.
        let Some(workdir) =
            open.or_else(|| matches!(dialog.form, Form::Clone { .. }).then(PathBuf::new))
        else {
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
        let cloned_to = form.cloned_to(&values);
        Self::spawn_blocking(
            {
                let form = form.clone();
                move || form.submit(&workdir, &values)
            },
            move |this, result, cx| {
                let cloned = cloned_to.filter(|_| result.is_ok());
                this.finish_form(&form, result, cx);
                if let Some(path) = cloned {
                    this.cloned(path, cx);
                }
            },
            cx,
        );
    }

    /// Opens the form that clones a repository. The address starts as what is on the
    /// clipboard when that looks like one, so copying a link and pressing the key is
    /// enough, and the folder starts as the one last cloned into.
    pub fn open_clone_form(&mut self, cx: &mut Context<Self>) {
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        let address = copied
            .filter(|text| clone::looks_like_address(text))
            .map(|text| text.trim().to_owned())
            .unwrap_or_default();
        let remembered = self
            .state_file()
            .and_then(|file| AppState::load(file).clone_dir);
        let into = clone::default_parent(remembered.as_deref());
        self.open_form(Form::Clone { address, into }, cx);
    }

    /// A clone finished: remembers the folder it went into and asks for the repository
    /// to be shown, in this tab if it is empty and else in a new one.
    fn cloned(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let (Some(file), Some(parent)) = (self.state_file(), path.parent()) {
            let parent = parent.to_owned();
            AppState::change(file, |state| state.clone_dir = Some(parent)).ok();
        }
        cx.emit(OpenWorktreeInTab(path));
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
