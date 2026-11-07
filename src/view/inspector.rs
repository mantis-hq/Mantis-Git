//! The panel beside the graph that shows the selected commit.

use super::{GraphView, Status};
use crate::details::{self, CommitDetails, FileChange};
use crate::git::{Commit, RefKind, split_subject};
use crate::theme::{selected_row, tokens};
use chrono::{Local, TimeZone};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;

const WIDTH: Pixels = px(380.);

#[derive(Default)]
pub(super) struct State {
    /// The selected commit's details, once read.
    details: Option<Arc<CommitDetails>>,
}

impl GraphView {
    pub(super) fn selected_details(&self) -> Option<&Arc<CommitDetails>> {
        let oid = &self.selected_commit()?.oid;
        self.inspector.details.as_ref().filter(|d| d.oid == *oid)
    }

    fn selected_commit(&self) -> Option<&Commit> {
        let entry = self.entries.get(self.selected?)?;
        (!entry.working).then_some(&entry.commit)
    }

    /// Whether the row selected is the working changes rather than a commit.
    pub(super) fn working_row_is_selected(&self) -> bool {
        self.selected
            .and_then(|index| self.entries.get(index))
            .is_some_and(|entry| entry.working)
    }

    /// Loads the new selection's details, keeping the old ones up until then so that
    /// moving through the list doesn't flash an empty panel.
    pub(super) fn selection_changed(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(commit)) = (self.workdir(), self.selected_commit()) else {
            self.inspector.details = None;
            return;
        };
        let oid = commit.oid.clone();
        cx.spawn(async move |this, cx| {
            let wanted = oid.clone();
            let details = cx
                .background_spawn(async move { details::read(&repo, &wanted) })
                .await;
            this.update(cx, |this, cx| {
                if this.selected_oid.as_ref() != Some(&oid) {
                    return;
                }
                this.inspector.details = details.ok().map(Arc::new);
                this.details_loaded(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Selects HEAD after a load if nothing else is, so the panel starts out useful.
    pub(super) fn select_head_if_unselected(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            return self.selection_changed(cx);
        }
        let head = self.refs.head.clone();
        let index = self
            .entries
            .iter()
            .position(|e| Some(&e.commit.oid) == head.as_ref());
        if let Some(index) = index {
            self.select(index, cx);
        }
    }

    fn select_oid(&mut self, oid: &str, cx: &mut Context<Self>) {
        if let Some(index) = self.entries.iter().position(|e| &*e.commit.oid == oid) {
            self.select(index, cx);
        }
    }

    /// The branch to switch to for a commit: the one at it, if it isn't already current.
    fn branch_at(&self, commit: &Commit) -> Option<String> {
        let refs = self.refs.by_commit.get(&commit.oid)?;
        refs.iter()
            .find(|r| r.kind == RefKind::Branch)
            .map(|r| r.name.clone())
    }

    pub(super) fn render_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.status == Status::NoRepository {
            return None;
        }
        let theme = cx.theme();
        let panel = div()
            .id("inspector")
            .w(WIDTH)
            .flex_none()
            .bg(theme.title_bar)
            .border_l_1()
            .border_color(theme.border);
        if self.working_row_is_selected() {
            let branch = self
                .refs
                .head_branch
                .clone()
                .unwrap_or_else(|| "HEAD".to_owned());
            let files = self.refs.working_files;
            let noun = if files == 1 { "file" } else { "files" };
            let subtext = tokens(theme.is_dark()).subtext0;
            return Some(
                panel
                    .child(
                        div()
                            .v_flex()
                            .p_4()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .line_height(px(24.))
                                    .font_semibold()
                                    .child("Working changes"),
                            )
                            .child(div().text_color(subtext).child(format!(
                                "{files} {noun} changed, not committed yet. These sit on top of \
                                 {branch}."
                            )))
                            .child(
                                Button::new("review-and-commit")
                                    .primary()
                                    .label("Review and commit")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.open_commit_screen(cx)),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }
        let Some(commit) = self.selected_commit() else {
            return Some(
                panel
                    .p_4()
                    .text_color(theme.muted_foreground)
                    .child("Select a commit to see what changed.")
                    .into_any_element(),
            );
        };
        let details = self.inspector.details.as_ref();
        let loaded = details.filter(|details| details.oid == commit.oid);

        Some(
            panel
                .child(
                    div()
                        .v_flex()
                        .size_full()
                        .p_4()
                        .gap_4()
                        .overflow_y_scrollbar()
                        .child(self.render_summary(commit, loaded, cx))
                        .child(self.render_facts(commit, cx))
                        .child(self.render_actions(commit, cx))
                        .when_some(loaded, |panel, details| {
                            panel.child(self.render_files(commit, details, cx))
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_summary(
        &self,
        commit: &Commit,
        details: Option<&Arc<CommitDetails>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let subtext = tokens(theme.is_dark()).subtext0;
        let (prefix, description) = split_subject(&commit.subject);
        let body = details.map(|d| d.body.clone()).filter(|b| !b.is_empty());

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(18.))
                    .line_height(px(24.))
                    .font_semibold()
                    .child(
                        StyledText::new(format!("{prefix}{description}")).with_highlights([(
                            0..prefix.len(),
                            HighlightStyle {
                                color: Some(muted),
                                font_weight: Some(FontWeight::MEDIUM),
                                ..Default::default()
                            },
                        )]),
                    ),
            )
            .when_some(body, |summary, body| {
                summary.child(div().text_color(subtext).child(body))
            })
    }

    fn render_facts(&self, commit: &Commit, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, link) = (theme.muted_foreground, theme.primary);
        let fact = |label: &str, value: AnyElement| {
            div()
                .h_flex()
                .items_start()
                .child(
                    div()
                        .w(px(72.))
                        .flex_none()
                        .text_color(muted)
                        .child(label.to_owned()),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let when = Local
            .timestamp_opt(commit.time, 0)
            .single()
            .map(|date| date.format("%b %-d %Y, %H:%M").to_string())
            .unwrap_or_default();
        let parents = commit.parents.iter().enumerate().map(|(index, parent)| {
            let oid = parent.clone();
            div()
                .id(("parent", index))
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(link)
                .child(parent[..parent.len().min(7)].to_owned())
                .on_click(cx.listener(move |this, _, _, cx| this.select_oid(&oid, cx)))
        });
        let label = if commit.parents.len() > 1 {
            "Parents"
        } else {
            "Parent"
        };

        div()
            .v_flex()
            .gap_1()
            .child(fact(
                "Author",
                format!("{}, {when}", commit.author).into_any_element(),
            ))
            .child(fact(
                "Commit",
                div()
                    .text_size(px(13.))
                    .child(SharedString::from(commit.oid.clone()))
                    .into_any_element(),
            ))
            .child(fact(
                label,
                div()
                    .h_flex()
                    .gap_2()
                    .children(parents)
                    .when(commit.parents.is_empty(), |list| {
                        list.text_size(px(13.)).text_color(muted).child("none")
                    })
                    .into_any_element(),
            ))
    }

    fn render_actions(&self, commit: &Commit, cx: &mut Context<Self>) -> impl IntoElement {
        let oid = commit.oid.clone();
        let branch = self.branch_at(commit);
        let is_merge = commit.parents.len() > 1;
        let is_head = self.refs.head.as_ref() == Some(&commit.oid);
        let busy = self.busy.is_some();

        let check_out = {
            let (oid, branch) = (oid.clone(), branch.clone());
            cx.listener(move |this, _, _, cx| {
                let (oid, branch) = (oid.clone(), branch.clone());
                this.run_operation(
                    "Checking out",
                    move |repo| details::check_out(repo, branch.as_deref(), &oid),
                    cx,
                );
            })
        };
        let cherry_pick = {
            let oid = oid.clone();
            cx.listener(move |this, _, _, cx| {
                let oid = oid.clone();
                this.run_operation(
                    "Cherry-picking",
                    move |repo| details::cherry_pick(repo, &oid),
                    cx,
                );
            })
        };
        let revert = cx.listener(move |this, _, _, cx| {
            let oid = oid.clone();
            this.run_operation(
                "Reverting",
                move |repo| details::revert(repo, &oid, is_merge),
                cx,
            );
        });

        div()
            .h_flex()
            .gap_1()
            .child(
                Button::new("check-out")
                    .ghost()
                    .small()
                    .label(match &branch {
                        Some(branch) => format!("Check out {branch}"),
                        None => "Check out".to_owned(),
                    })
                    .disabled(busy || is_head)
                    .on_click(check_out),
            )
            .when(!is_merge && !is_head, |actions| {
                actions.child(
                    Button::new("cherry-pick")
                        .ghost()
                        .small()
                        .label("Cherry-pick")
                        .disabled(busy)
                        .on_click(cherry_pick),
                )
            })
            .child(
                Button::new("revert")
                    .ghost()
                    .small()
                    .label("Revert")
                    .disabled(busy)
                    .on_click(revert),
            )
    }
}

pub(super) fn status_color(file: &FileChange, cx: &App) -> Hsla {
    let theme = cx.theme();
    match file.status {
        'A' => theme.success,
        'D' => theme.danger,
        _ => theme.warning,
    }
}

impl GraphView {
    fn render_files(
        &self,
        commit: &Commit,
        details: &CommitDetails,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, added, removed) = (theme.muted_foreground, theme.success, theme.danger);
        let count = details.files.len();
        let (additions, deletions) = details.totals();
        let is_merge = commit.parents.len() > 1;
        let merge_note = tokens(cx.theme().is_dark()).subtext0;
        let mut files = Vec::with_capacity(count);
        for (index, file) in details.files.iter().enumerate() {
            files.push(self.render_file(index, file, cx));
        }

        div()
            .v_flex()
            .gap(px(2.))
            .child(
                div()
                    .h_flex()
                    .h(px(24.))
                    .justify_between()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(muted)
                    .child(format!(
                        "{count} {} changed",
                        if count == 1 { "file" } else { "files" }
                    ))
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .text_xs()
                            .child(div().text_color(added).child(format!("+{additions}")))
                            .child(div().text_color(removed).child(format!("−{deletions}"))),
                    ),
            )
            .children(files)
            .when(is_merge && count == 0, |list| {
                list.child(
                    div()
                        .text_color(merge_note)
                        .child("This merge has no changes of its own."),
                )
            })
    }

    fn render_file(
        &self,
        index: usize,
        file: &FileChange,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, active, hover) = (
            theme.muted_foreground,
            selected_row(theme),
            theme.list_hover,
        );
        let shown = self.shown_file() == Some(index);
        let (folder, name) = file.split_path();
        let count = if file.binary {
            "binary".to_owned()
        } else {
            format!("+{} −{}", file.additions, file.deletions)
        };
        div()
            .id(("file", index))
            .h_flex()
            .h(px(28.))
            .mx(px(-8.))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .when(shown, |row| row.bg(active))
            .when(!shown, |row| row.hover(|style| style.bg(hover)))
            .on_click(cx.listener(move |this, _, _, cx| this.show_file(index, cx)))
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .text_xs()
                    .font_medium()
                    .text_color(status_color(file, cx))
                    .child(file.status.to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(div().flex_none().text_color(muted).child(folder.to_owned()))
                    .child(div().min_w_0().truncate().child(name.to_owned())),
            )
            .child(div().flex_none().text_xs().text_color(muted).child(count))
    }
}
