//! The panel beside the graph that shows the selected commit.

use super::parts::{TEXT_CHIP, TEXT_SMALL};
use super::parts::{file_path, panel_row, section_label, status_color, title};
use super::{GraphView, Status};
use crate::theme::tokens;
use chrono::{Local, TimeZone};
use gitclient_core::details::{self, CommitDetails, FileChange};
use gitclient_core::git::{Commit, Entry, split_subject};
use gitclient_core::ops::Cancel;
use gitclient_core::text::{count, short_oid};
use gitclient_core::working::{self, WorkingFile};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::sync::Arc;

const WIDTH: Pixels = px(380.);

#[derive(Default)]
pub(super) struct State {
    /// The selected commit's details, once read.
    details: Option<Arc<CommitDetails>>,
    /// The uncommitted changes, read in full so a file's diff needs no further git call.
    working: Option<Working>,
    /// The width the user dragged the panel to.
    pub(super) width: Option<f32>,
    /// Scroll of the file list, which draws only the rows in view so that a commit with
    /// thousands of files costs no more than one with a few.
    files_scroll: UniformListScrollHandle,
    /// The read of the selected commit's details, while it runs. Selecting something else
    /// sets it, so that git stops work nobody will look at.
    loading: Option<Cancel>,
}

/// What the working changes row shows: each file's patch, and the same list a commit has.
pub(super) struct Working {
    pub(super) files: Vec<WorkingFile>,
    pub(super) details: Arc<CommitDetails>,
}

/// Stands in for a commit id in the details of the working changes.
pub(super) const WORKING_ID: &str = "working";

impl GraphView {
    pub(super) fn selected_details(&self) -> Option<&Arc<CommitDetails>> {
        let oid = &self.selected_commit()?.oid;
        self.inspector.details.as_ref().filter(|d| d.oid == *oid)
    }

    /// The files on show in the inspector: a commit's, or the working changes'.
    pub(super) fn shown_details(&self) -> Option<&Arc<CommitDetails>> {
        if self.working_row_is_selected() {
            return self
                .inspector
                .working
                .as_ref()
                .map(|working| &working.details);
        }
        self.selected_details()
    }

    pub(super) fn working_patch(&self, index: usize) -> Option<&WorkingFile> {
        self.inspector.working.as_ref()?.files.get(index)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected?)
    }

    fn selected_commit(&self) -> Option<&Commit> {
        let entry = self.selected_entry()?;
        (!entry.working).then_some(&entry.commit)
    }

    /// Whether the row selected is the working changes rather than a commit.
    pub(super) fn working_row_is_selected(&self) -> bool {
        self.selected_entry().is_some_and(|entry| entry.working)
    }

    /// Loads the new selection's details, keeping the old ones up until then so that
    /// moving through the list doesn't flash an empty panel.
    pub(super) fn selection_changed(&mut self, cx: &mut Context<Self>) {
        self.stop_loading_details();
        if self.working_row_is_selected() {
            return self.reload_working(cx);
        }
        let (Some(repo), Some(commit)) = (self.workdir(), self.selected_commit()) else {
            self.inspector.details = None;
            return;
        };
        let oid = commit.oid.clone();
        let wanted = oid.clone();
        let cancel = Cancel::new();
        self.inspector.loading = Some(cancel.clone());
        let finished = cancel.clone();
        Self::spawn_blocking(
            move || details::read(&repo, &wanted, &cancel),
            move |this, details, cx| {
                // Only the newest read has anything to say: an older one was cancelled.
                if finished.is_cancelled() || this.selected_oid.as_ref() != Some(&oid) {
                    return;
                }
                this.inspector.loading = None;
                this.inspector.details = details.ok().map(Arc::new);
                this.inspector
                    .files_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                this.details_loaded(cx);
                cx.notify();
            },
            cx,
        );
    }

    /// Cancels the read of the previous selection's details, if it is still running.
    fn stop_loading_details(&mut self) {
        if let Some(cancel) = self.inspector.loading.take() {
            cancel.cancel();
        }
    }

    /// Reads the uncommitted changes. Also run when the window is used again, because
    /// they change under the app as files are edited.
    pub(super) fn reload_working(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        Self::spawn_blocking(
            move || working::read(&repo),
            move |this, files, cx| {
                if !this.working_row_is_selected() {
                    return;
                }
                let Ok(files) = files else {
                    this.inspector.working = None;
                    return cx.notify();
                };
                let details = CommitDetails {
                    oid: WORKING_ID.into(),
                    body: String::new(),
                    files: files.iter().map(WorkingFile::change).collect(),
                };
                this.inspector.working = Some(Working {
                    files,
                    details: Arc::new(details),
                });
                this.working_loaded(cx);
                cx.notify();
            },
            cx,
        );
    }

    /// Selects HEAD after a load if nothing else is, so the panel starts out useful.
    pub(super) fn select_head_if_unselected(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() {
            return self.selection_changed(cx);
        }
        if let Some(head) = self.refs.head.clone() {
            self.select_oid(&head, cx);
        }
    }

    fn select_oid(&mut self, oid: &str, cx: &mut Context<Self>) {
        if let Some(index) = self.row_of(oid) {
            self.select(index, cx);
        }
    }

    pub(super) fn render_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.status == Status::NoRepository {
            return None;
        }
        let theme = cx.theme();
        let panel = div()
            .id("inspector")
            .w(self.inspector.width.map_or(WIDTH, px))
            .flex_none()
            .bg(theme.title_bar)
            .border_l_1()
            .border_color(theme.border);
        if self.working_row_is_selected() {
            let sentence = self.refs.working_sentence();
            let subtext = tokens(theme.is_dark()).subtext0;
            let files = self.shown_details().cloned();
            return Some(
                panel
                    .child(
                        div()
                            .v_flex()
                            .size_full()
                            .p_4()
                            .gap_4()
                            .child(
                                div()
                                    .v_flex()
                                    .flex_none()
                                    .gap_3()
                                    .child(title().child("Working changes"))
                                    .child(div().text_color(subtext).child(sentence))
                                    .child(
                                        Button::new("review-and-commit")
                                            .primary()
                                            .label("Review and commit")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.open_commit_screen(cx)
                                            })),
                                    ),
                            )
                            .when_some(files, |panel, details| {
                                panel.child(self.render_files(false, &details, cx))
                            }),
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
        let loaded = self.selected_details();

        Some(
            panel
                .child(
                    div()
                        .v_flex()
                        .size_full()
                        .p_4()
                        .gap_4()
                        .child(
                            // Capped, so that a long message can't push the files out of view.
                            div()
                                .v_flex()
                                .flex_none()
                                .max_h(relative(0.5))
                                .gap_4()
                                .overflow_y_scrollbar()
                                .child(self.render_summary(commit, loaded, cx))
                                .child(self.render_facts(commit, cx))
                                .child(self.render_actions(commit, cx)),
                        )
                        .when_some(loaded, |panel, details| {
                            panel.child(self.render_files(commit.is_merge(), details, cx))
                        })
                        .when(
                            loaded.is_none() && self.inspector.loading.is_some(),
                            |panel| panel.child(self.render_loading(cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Stands where the file list will be while git is still reading it.
    fn render_loading(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .h_flex()
            .gap_2()
            .h(px(24.))
            .text_color(muted)
            .child(Spinner::new().small())
            .child("Loading changed files…")
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
            .child(title().child(
                StyledText::new(format!("{prefix}{description}")).with_highlights([(
                    0..prefix.len(),
                    HighlightStyle {
                        color: Some(muted),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..Default::default()
                    },
                )]),
            ))
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
                .text_size(TEXT_SMALL)
                .text_color(link)
                .child(short_oid(parent).to_owned())
                .on_click(cx.listener(move |this, _, _, cx| this.select_oid(&oid, cx)))
        });
        let label = if commit.is_merge() {
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
                    .text_size(TEXT_SMALL)
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
                        list.text_size(TEXT_SMALL).text_color(muted).child("none")
                    })
                    .into_any_element(),
            ))
    }

    fn render_actions(&self, commit: &Commit, cx: &mut Context<Self>) -> impl IntoElement {
        let oid = commit.oid.clone();
        let actions = self.refs.actions_for(commit);
        let (branch, is_merge) = (actions.branch.clone(), actions.is_merge);
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
                    .label(actions.check_out_label())
                    .disabled(busy || !actions.can_check_out)
                    .on_click(check_out),
            )
            .when(actions.can_cherry_pick, |list| {
                list.child(
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

impl GraphView {
    fn render_files(
        &self,
        is_merge: bool,
        details: &Arc<CommitDetails>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, added, removed) = (theme.muted_foreground, theme.success, theme.danger);
        let total = details.files.len();
        let (additions, deletions) = details.totals();
        let merge_note = tokens(cx.theme().is_dark()).subtext0;
        let listed = details.clone();

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap(px(2.))
            // Rows run edge to edge, past the panel's padding.
            .mx(px(-8.))
            .child(
                section_label(muted)
                    .h_flex()
                    .h(px(24.))
                    .px_2()
                    .justify_between()
                    .child(format!("{} changed", count(total, "file")))
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .text_size(TEXT_CHIP)
                            .child(div().text_color(added).child(format!("+{additions}")))
                            .child(div().text_color(removed).child(format!("−{deletions}"))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "inspector-files",
                            total,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .filter_map(|index| {
                                        let file = listed.files.get(index)?;
                                        Some(this.render_file(index, file, cx))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.inspector.files_scroll)
                        .size_full(),
                    )
                    .scrollbar(&self.inspector.files_scroll, ScrollbarAxis::Vertical),
            )
            .when(is_merge && total == 0, |list| {
                list.child(
                    div()
                        .px_2()
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
        let muted = cx.theme().muted_foreground;
        let shown = self.shown_file() == Some(index);
        let count = if file.binary {
            "binary".to_owned()
        } else {
            format!("+{} −{}", file.additions, file.deletions)
        };
        panel_row(("file", index), shown, cx)
            .h(px(28.))
            .on_click(cx.listener(move |this, _, _, cx| this.show_file(index, cx)))
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .text_size(TEXT_CHIP)
                    .font_medium()
                    .text_color(status_color(file.status, cx))
                    .child(file.status.to_string()),
            )
            .child(file_path(&file.path, muted))
            .child(
                div()
                    .flex_none()
                    .text_size(TEXT_CHIP)
                    .text_color(muted)
                    .child(count),
            )
    }
}
