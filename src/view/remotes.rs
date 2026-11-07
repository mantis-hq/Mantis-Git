//! Adding and editing remotes. The sidebar's "+" and a remote's menu open one form under
//! the header, with a name and an address. What a remote is and how it is saved lives in
//! `gitclient_core::remote`.

use super::GraphView;
use super::parts::{TEXT_BODY, TEXT_SMALL};
use crate::theme::tokens;
use gitclient_core::menu;
use gitclient_core::remote;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::Path;

/// The open form. Its inputs are made on the next draw, which is the first time there
/// is a window to make them in.
pub(super) struct RemoteForm {
    /// The remote being edited, or `None` while adding one.
    original: Option<String>,
    name: String,
    url: String,
    /// Why the last attempt to save was turned down.
    problem: Option<&'static str>,
    inputs: Option<Inputs>,
}

struct Inputs {
    name: Entity<InputState>,
    url: Entity<InputState>,
    _submit: Vec<Subscription>,
}

impl RemoteForm {
    /// The name and address the form started with.
    #[cfg(test)]
    pub(super) fn prefill(&self) -> (&str, &str) {
        (&self.name, &self.url)
    }
}

impl GraphView {
    /// Opens the form to add a remote, or to edit the one called `original`.
    pub(super) fn open_remote_form(&mut self, original: Option<String>, cx: &mut Context<Self>) {
        let (name, url) = match &original {
            Some(original) => self
                .refs
                .remote_configs
                .iter()
                .find(|remote| &remote.name == original)
                .map(|remote| (remote.name.clone(), remote.url.clone()))
                .unwrap_or_default(),
            None => Default::default(),
        };
        self.menu = None;
        self.remote_form = Some(RemoteForm {
            original,
            name,
            url,
            problem: None,
            inputs: None,
        });
        cx.notify();
    }

    pub(super) fn open_remote_menu(
        &mut self,
        name: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let lines = menu::remote_menu(&self.refs, name);
        self.show_menu(position, lines, cx);
    }

    /// Whether `name` is a remote of the repository, as opposed to a folder of branches.
    pub(super) fn is_remote(&self, name: &str) -> bool {
        self.refs
            .remote_configs
            .iter()
            .any(|remote| remote.name == name)
    }

    /// Escape: closes the menu if one is open, otherwise the form.
    pub(super) fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.close_menu(cx);
        } else if self.remote_form.take().is_some() {
            cx.notify();
        }
    }

    /// Makes the form's inputs the first time it is drawn, and puts the cursor in the first.
    pub(super) fn ensure_remote_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.remote_form else {
            return;
        };
        if form.inputs.is_some() {
            return;
        }
        let (name, url) = (form.name.clone(), form.url.clone());
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("origin")
                .default_value(name)
        });
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://example.com/you/repo.git")
                .default_value(url)
        });
        let submit = [&name, &url]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_remote_form(cx);
                    }
                })
            })
            .collect();
        window.focus(&name.focus_handle(cx), cx);
        if let Some(form) = &mut self.remote_form {
            form.inputs = Some(Inputs {
                name,
                url,
                _submit: submit,
            });
        }
    }

    fn save_remote_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.remote_form else {
            return;
        };
        let Some(inputs) = &form.inputs else {
            return;
        };
        let name = inputs.name.read(cx).value().trim().to_owned();
        let url = inputs.url.read(cx).value().trim().to_owned();
        let problem = remote::why_bad_name(&name).or(url
            .is_empty()
            .then_some("Give the remote an address to fetch from."));
        if problem.is_some() {
            form.problem = problem;
            cx.notify();
            return;
        }
        let original = form.original.clone();
        let label = if original.is_some() {
            "Saving the remote"
        } else {
            "Adding the remote"
        };
        self.remote_form = None;
        self.run_operation(
            label,
            move |repo: &Path| remote::save(repo, original.as_deref(), &name, &url),
            cx,
        );
    }

    pub(super) fn render_remote_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.remote_form.as_ref()?;
        let inputs = form.inputs.as_ref()?;
        let theme = cx.theme();
        let (border, header, muted) = (theme.border, theme.title_bar, theme.muted_foreground);
        let peach = tokens(theme.is_dark()).peach;
        let (title, action) = match &form.original {
            Some(original) => (format!("Edit {original}"), format!("Save {original}")),
            None => ("Add a remote".to_owned(), "Add remote".to_owned()),
        };
        let label = |text: &'static str| {
            div()
                .flex_none()
                .text_size(TEXT_SMALL)
                .font_medium()
                .text_color(muted)
                .child(text)
        };
        Some(
            div()
                .id("remote-form")
                .v_flex()
                .flex_none()
                .gap_2()
                .px_4()
                .py_2()
                .text_size(TEXT_BODY)
                .bg(header)
                .border_b_1()
                .border_color(border)
                .child(
                    div()
                        .h_flex()
                        .gap_3()
                        .child(div().flex_1().font_semibold().child(title))
                        .child(
                            Button::new("remote-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                        )
                        .child(
                            Button::new("remote-save")
                                .ghost()
                                .small()
                                .label(action)
                                .on_click(cx.listener(|this, _, _, cx| this.save_remote_form(cx))),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(label("Name"))
                        .child(
                            div()
                                .w(px(180.))
                                .flex_none()
                                .child(Input::new(&inputs.name)),
                        )
                        .child(label("Address"))
                        .child(div().flex_1().min_w_0().child(Input::new(&inputs.url))),
                )
                .children(
                    form.problem
                        .map(|problem| div().text_color(peach).child(problem)),
                )
                .into_any_element(),
        )
    }
}
