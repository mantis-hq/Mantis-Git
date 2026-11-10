//! The toasts and cards over the window, and the runner for git operations that report through them.
//!
//! Operations that change a repository run off the UI thread, one at a time. Whatever
//! happens is said here in words.

use super::GraphView;
use super::parts::{card, card_actions};
use crate::theme::tokens;
use gitclient_core::repo::commit_graph;
use gitclient_core::text;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::Path;
use std::time::Duration;

/// How long a toast waits after the pointer leaves before it closes.
const HOVER_GRACE: Duration = Duration::from_secs(2);

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
                // The screen went ahead of git, so it goes back. Nothing changed in git.
                self.conflict_not_written(cx);
            }
            (After::Reload, _) => self.reload(cx),
        }
    }

    /// The card that says counting the branches was slow because there is no commit graph,
    /// with the button that writes one. It stays until one of its buttons is pressed, and
    /// is drawn only when both are true.
    pub(super) fn render_graph_hint(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let message = self.graph_hint.clone()?;
        let peach = tokens(cx.theme().is_dark()).peach;
        Some(
            card("graph-hint", cx)
                .child(div().text_color(peach).child(message))
                .child(
                    card_actions()
                        .child(
                            Button::new("graph-hint-dismiss")
                                .ghost()
                                .small()
                                .label("Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(repo) = this.repo.clone() {
                                        this.graph_hint_dismissed.insert(repo);
                                    }
                                    this.graph_hint = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("graph-hint-write")
                                .primary()
                                .small()
                                .label(commit_graph::WRITE_ACTION)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.write_commit_graph(cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Writes the commit graph the way Settings → Repository does, then reads the
    /// repository again so the counts come back quickly.
    pub(super) fn write_commit_graph(&mut self, cx: &mut Context<Self>) {
        self.graph_hint = None;
        self.run_operation(
            "Writing the commit graph…",
            |repo| commit_graph::write(repo).map(|report| Some(report.written_message())),
            cx,
        );
    }

    /// Starts the closing timer for a notice that has just appeared. Confirmations close
    /// themselves (core says after how long); failures wait for the ×. While the pointer is
    /// over the toast the timer starts over, so nothing closes under someone reading it.
    pub(super) fn sync_toast(&mut self, cx: &mut Context<Self>) {
        if self.toast_timed == self.notice {
            return;
        }
        self.toast_timed = self.notice.clone();
        self.toast_hovered = false;
        let Some(notice) = self.notice.clone() else {
            return;
        };
        let (message, is_error) = match &notice {
            Notice::Info(message) => (message, false),
            Notice::Error(message) => (message, true),
        };
        let Some(mut wait) = text::toast_lifetime(message, is_error) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(wait).await;
                let closed = this
                    .update(cx, |this, cx| {
                        if this.notice.as_ref() != Some(&notice) {
                            return true;
                        }
                        if this.toast_hovered {
                            return false;
                        }
                        this.notice = None;
                        cx.notify();
                        true
                    })
                    .unwrap_or(true);
                if closed {
                    break;
                }
                wait = HOVER_GRACE;
            }
        })
        .detach();
    }

    /// What just happened, as a card with a × to close it.
    fn render_toast(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let (message, color) = match self.notice.as_ref()? {
            Notice::Info(message) => (message.clone(), theme.secondary_foreground),
            Notice::Error(message) => (message.clone(), tokens(theme.is_dark()).peach),
        };
        Some(
            card("notice", cx)
                .on_hover(cx.listener(|this, hovered: &bool, _, _| this.toast_hovered = *hovered))
                .child(
                    div()
                        .h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().flex_1().min_w_0().text_color(color).child(message))
                        .child(
                            Button::new("notice-dismiss")
                                .ghost()
                                .compact()
                                .label("×")
                                .tooltip("Close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.notice = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// This tab's cards, what is offered first and what just happened last. The workspace
    /// draws them in the bottom right corner of the window, where the toast is nearest.
    pub(super) fn cards(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        [self.render_graph_hint(cx), self.render_toast(cx)]
            .into_iter()
            .flatten()
            .collect()
    }
}
