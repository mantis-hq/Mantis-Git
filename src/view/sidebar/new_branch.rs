//! The form at the top of the sidebar that names a new branch. The Branches heading and
//! the menus open it; creating the branch also checks it out.

use crate::view::GraphView;
use crate::view::parts::section_label;
use gitclient_core::repo::branch;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// The form that asks for a new branch's name.
pub(in crate::view) struct NewBranch {
    /// Where the branch starts, or the checked-out commit when `None`.
    start: Option<String>,
    /// What to call that place in the form.
    label: String,
    /// Made on the next render, which is the first time there is a window to make it in.
    input: Option<Entity<InputState>>,
    _events: Option<Subscription>,
}

impl GraphView {
    /// Opens the form for a new branch, showing the sidebar if it is closed.
    pub(in crate::view) fn begin_new_branch(
        &mut self,
        start: Option<String>,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.open = Some(true);
        self.sidebar.new_branch = Some(NewBranch {
            start,
            label,
            input: None,
            _events: None,
        });
        cx.notify();
    }

    fn cancel_new_branch(&mut self, cx: &mut Context<Self>) {
        self.sidebar.new_branch = None;
        cx.notify();
    }

    /// Makes the input of a form that was just opened, and puts the cursor in it.
    pub(in crate::view) fn ensure_new_branch_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .sidebar
            .new_branch
            .as_ref()
            .is_some_and(|form| form.input.is_none())
        {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("feat/my-change"));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.create_new_branch(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        if let Some(form) = &mut self.sidebar.new_branch {
            form.input = Some(input);
            form._events = Some(events);
        }
    }

    fn new_branch_name(&self, cx: &App) -> String {
        self.sidebar
            .new_branch
            .as_ref()
            .and_then(|form| form.input.as_ref())
            .map_or_else(String::new, |input| {
                input.read(cx).value().trim().to_owned()
            })
    }

    fn create_new_branch(&mut self, cx: &mut Context<Self>) {
        let name = self.new_branch_name(cx);
        if name.is_empty() || self.busy.is_some() {
            return;
        }
        let Some(form) = self.sidebar.new_branch.take() else {
            return;
        };
        self.run_operation(
            "Creating a branch",
            move |repo| branch::create_branch(repo, &name, form.start.as_deref()),
            cx,
        );
    }

    pub(super) fn render_new_branch(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.sidebar.new_branch.as_ref()?;
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let name = self.new_branch_name(cx);
        let disabled = name.is_empty() || self.busy.is_some();
        Some(
            div()
                .v_flex()
                .gap_2()
                .px_2()
                .pb_3()
                .mb_2()
                .border_b_1()
                .border_color(border)
                .child(section_label(muted).child(branch::new_branch_prompt(&form.label)))
                .children(form.input.as_ref().map(Input::new))
                .child(
                    div()
                        .h_flex()
                        .justify_end()
                        .gap_1()
                        .child(
                            Button::new("new-branch-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_new_branch(cx))),
                        )
                        .child(
                            Button::new("new-branch-create")
                                .ghost()
                                .small()
                                .label(branch::new_branch_label(&name))
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, cx| this.create_new_branch(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}
