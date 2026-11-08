//! The band under the header, and the runner for git operations that report through it.
//!
//! Operations that change a repository run off the UI thread, one at a time. Whatever
//! happens is said here in words, and risky steps wait here for a second click.

use super::GraphView;
use super::parts::TEXT_BODY;
use crate::theme::tokens;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    Info(SharedString),
    Error(SharedString),
}

/// What a view does once an operation has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum After {
    /// Reads the whole repository again.
    Reload,
    /// A file in conflict was saved. That moves no branch and adds no untracked file, so
    /// only the paused operation and the tracked changes are read again, which stays quick
    /// however large the history and the working tree are.
    ConflictSaved,
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
        self.run_operation_then(label, operation, After::Reload, cx);
    }

    /// Runs a git operation like [`Self::run_operation`], and catches up as `after` says.
    pub(super) fn run_operation_then(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Path) -> Result<Option<String>, String> + Send + 'static,
        after: After,
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
        Self::spawn_blocking(
            move || operation(&workdir),
            move |this, result, cx| this.finish_operation_then(result, after, cx),
            cx,
        );
    }

    pub(super) fn finish_operation(
        &mut self,
        result: Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) {
        self.finish_operation_then(result, After::Reload, cx);
    }

    fn finish_operation_then(
        &mut self,
        result: Result<Option<String>, String>,
        after: After,
        cx: &mut Context<Self>,
    ) {
        self.busy = None;
        let saved = result.is_ok();
        self.notice = match result {
            Ok(message) => message.map(|message| Notice::Info(message.into())),
            Err(message) => Some(Notice::Error(message.into())),
        };
        match (after, saved) {
            (After::ConflictSaved, true) => self.conflict_written(cx),
            (After::ConflictSaved, false) => {
                // The screen went ahead of git, so it goes back, and git is read again.
                self.conflict_not_written(cx);
                self.reload(cx);
            }
            (After::Reload, _) => self.reload(cx),
        }
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

        if let Some(confirm) = &self.confirm {
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
