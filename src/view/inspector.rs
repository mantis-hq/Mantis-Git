//! The panel beside the graph that shows the selected commit.

use super::parts::{TEXT_CHIP, TEXT_SMALL};
use super::parts::{
    checkbox, chip, file_path, loading, panel_row, section_label, status_color, title,
};
use super::{GraphView, Notice, Status};
use crate::theme::tokens;
use chrono::{Local, TimeZone};
use gitclient_core::details::{self, CommitDetails, FileChange};
use gitclient_core::git::{Commit, Entry, split_subject};
use gitclient_core::ops::{self, Cancel};
use gitclient_core::text::{count, short_oid};
use gitclient_core::tree::{self, FileView, TreeRow};
use gitclient_core::working::{self, Changes, Side, WorkingFile};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

const WIDTH: Pixels = px(380.);
/// Tree rows step in by this much for each folder they sit in.
const INDENT: f32 = 14.;

#[derive(Default)]
pub(super) struct State {
    /// The selected commit's details, once read.
    details: Option<Arc<CommitDetails>>,
    /// The uncommitted changes, read in full so a file's diff needs no further git call.
    working: Option<Working>,
    /// The width the user dragged the panel to.
    pub(super) width: Option<f32>,
    /// Scroll of the file list (the unstaged one, for the working changes), which draws
    /// only the rows in view so that a commit with thousands of files costs no more than
    /// one with a few.
    files_scroll: UniformListScrollHandle,
    staged_scroll: UniformListScrollHandle,
    /// The folders the user closed in the tree, for each of the two lists.
    closed_folders: HashSet<String>,
    closed_staged: HashSet<String>,
    /// Set while files are moving in or out of the index, so that a second click waits
    /// instead of racing the first for git's lock.
    pub(super) staging: bool,
    /// The read of the selected commit's details, while it runs. Selecting something else
    /// sets it, so that git stops work nobody will look at.
    loading: Option<Cancel>,
    /// Set while the working changes are being read, so that a burst of refreshes (the
    /// window used again, a file staged) waits for the read in progress rather than
    /// starting one git per event.
    reading_working: bool,
    /// Whether another refresh came in during that read, which then runs once more.
    read_working_again: bool,
}

/// What the working changes row shows: what is staged and what is not, and the same list
/// a commit has, with the unstaged files first and the staged ones after.
pub(super) struct Working {
    pub(super) changes: Changes,
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

    pub(super) fn working_changes(&self) -> Option<&Changes> {
        self.inspector
            .working
            .as_ref()
            .map(|working| &working.changes)
    }

    /// A working file by its place in the two lists end to end, with the list it is in.
    pub(super) fn working_file(&self, index: usize) -> Option<(Side, &WorkingFile)> {
        self.working_changes()?.file(index)
    }

    /// Where a file of one side sits in the two lists end to end.
    pub(super) fn working_position(&self, side: Side, path: &str) -> Option<usize> {
        let changes = self.working_changes()?;
        let at = changes
            .side(side)
            .iter()
            .position(|file| file.path == path)?;
        Some(match side {
            Side::Unstaged => at,
            Side::Staged => changes.unstaged.len() + at,
        })
    }

    pub(super) fn staged_count(&self) -> usize {
        self.working_changes()
            .map_or(0, |changes| changes.staged.len())
    }

    pub(super) fn reset_file_scrolls(&mut self) {
        self.inspector.files_scroll = UniformListScrollHandle::new();
        self.inspector.staged_scroll = UniformListScrollHandle::new();
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
        self.refresh_files_view();
        self.stop_loading_details();
        if self.working_row_is_selected() {
            return self.reload_working(cx);
        }
        let (Some(repo), Some(commit)) = (self.workdir(), self.selected_commit()) else {
            self.inspector.details = None;
            return;
        };
        let oid = commit.oid.clone();
        let fingerprint = self.fingerprint.clone();
        // Seen before: no git, no spinner, and no waiting on anything.
        let cached = fingerprint
            .as_deref()
            .and_then(|fingerprint| self.caches.peek_details(fingerprint, &oid));
        if let Some(details) = cached {
            self.inspector.details = Some(details);
            self.inspector
                .files_scroll
                .scroll_to_item(0, ScrollStrategy::Top);
            self.details_loaded(cx);
            return cx.notify();
        }

        let wanted = oid.clone();
        let caches = self.caches.clone();
        let cancel = Cancel::new();
        self.inspector.loading = Some(cancel.clone());
        let finished = cancel.clone();
        Self::spawn_blocking(
            move || match fingerprint {
                // The disk is read here too, so a commit seen in an earlier run is quick.
                Some(fingerprint) => caches.read_details(&repo, &fingerprint, &wanted, &cancel),
                None => details::read(&repo, &wanted, &cancel).map(Arc::new),
            },
            move |this, details, cx| {
                // Only the newest read has anything to say: an older one was cancelled.
                if finished.is_cancelled() || this.selected_oid.as_ref() != Some(&oid) {
                    return;
                }
                this.inspector.loading = None;
                this.inspector.details = details.ok();
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
    pub(super) fn stop_loading_details(&mut self) {
        if let Some(cancel) = self.inspector.loading.take() {
            cancel.cancel();
        }
    }

    /// Reads the uncommitted changes. Also run when the window is used again, because
    /// they change under the app as files are edited. One read runs at a time: a request
    /// during it makes it run once more when it ends, and selecting a commit stops it.
    pub(super) fn reload_working(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        if self.inspector.reading_working {
            self.inspector.read_working_again = true;
            return;
        }
        self.inspector.reading_working = true;
        let cancel = Cancel::new();
        self.inspector.loading = Some(cancel.clone());
        Self::spawn_blocking(
            move || working::read(&repo, &cancel),
            move |this, changes, cx| {
                this.inspector.reading_working = false;
                let again = std::mem::take(&mut this.inspector.read_working_again);
                if !this.working_row_is_selected() {
                    return;
                }
                if again || changes.as_ref().is_err_and(|e| e == ops::CANCELLED) {
                    // Out of date, or stopped: only the newest read has anything to say.
                    return this.reload_working(cx);
                }
                this.inspector.loading = None;
                let Ok(changes) = changes else {
                    this.inspector.working = None;
                    return cx.notify();
                };
                let same = this
                    .inspector
                    .working
                    .as_ref()
                    .is_some_and(|working| working.changes == changes);
                if same {
                    // Nothing in the list moved, so only an open diff may have changed.
                    if this.diff_is_open() {
                        this.working_loaded(cx);
                    }
                    return;
                }
                let details = CommitDetails {
                    oid: WORKING_ID.into(),
                    body: String::new(),
                    files: changes.all().map(WorkingFile::change).collect(),
                };
                this.inspector.working = Some(Working {
                    changes,
                    details: Arc::new(details),
                });
                this.working_loaded(cx);
                cx.notify();
            },
            cx,
        );
    }

    /// Selects the working changes row, if there is one.
    pub fn select_working_changes(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.entries.iter().position(|entry| entry.working) {
            self.select(index, cx);
        }
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
        if let Some(progress) = self
            .paused_operation()
            .cloned()
            .filter(|_| self.working_row_is_selected())
        {
            return Some(
                panel
                    .child(self.render_conflict_panel(&progress, cx))
                    .into_any_element(),
            );
        }
        if self.working_row_is_selected() {
            let sentence = self.refs.working_sentence();
            let subtext = tokens(theme.is_dark()).subtext0;
            let border = theme.border;
            let files = self.shown_details().cloned();
            return Some(
                panel
                    .child(
                        div()
                            .v_flex()
                            .size_full()
                            .p_4()
                            .gap_3()
                            .child(
                                div()
                                    .v_flex()
                                    .flex_none()
                                    .gap_2()
                                    .child(
                                        div()
                                            .h_flex()
                                            .justify_between()
                                            .child(title().child("Working changes"))
                                            .child(self.render_view_chips(cx)),
                                    )
                                    .child(div().text_color(subtext).child(sentence)),
                            )
                            .when_some(files, |panel, details| {
                                panel
                                    .child(self.render_working_list(Side::Unstaged, &details, cx))
                                    .child(self.render_working_list(Side::Staged, &details, cx))
                            })
                            .child(
                                // Capped, so that a short window keeps some of its lists.
                                div()
                                    .id("commit-form")
                                    .flex_none()
                                    .max_h(relative(0.42))
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(border)
                                    .overflow_y_scroll()
                                    .child(self.render_commit_form(cx)),
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
                            panel.child(self.render_commit_files(commit.is_merge(), details, cx))
                        })
                        .when(
                            loaded.is_none() && self.inspector.loading.is_some(),
                            |panel| {
                                panel.child(loading(
                                    "inspector-loading",
                                    "Loading changed files…",
                                    6,
                                    cx,
                                ))
                            },
                        ),
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

/// One list of files the inspector can draw.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    /// A commit's files.
    Commit,
    /// Working changes the index doesn't have.
    Unstaged,
    /// Working changes in the index: what the next commit holds.
    Staged,
}

impl Pane {
    fn of(side: Side) -> Self {
        match side {
            Side::Unstaged => Self::Unstaged,
            Side::Staged => Self::Staged,
        }
    }

    /// The list of working changes this is, if it is one.
    fn side(self) -> Option<Side> {
        match self {
            Self::Commit => None,
            Self::Unstaged => Some(Side::Unstaged),
            Self::Staged => Some(Side::Staged),
        }
    }

    /// Ids that keep one list's rows, boxes and animations apart from the others'.
    fn list_id(self) -> &'static str {
        match self {
            Self::Commit => "inspector-files",
            Self::Unstaged => "inspector-unstaged",
            Self::Staged => "inspector-staged",
        }
    }

    fn row_id(self) -> &'static str {
        match self {
            Self::Commit => "file",
            Self::Unstaged => "unstaged-file",
            Self::Staged => "staged-file",
        }
    }

    fn tree_id(self) -> &'static str {
        match self {
            Self::Commit => "tree-row",
            Self::Unstaged => "unstaged-tree-row",
            Self::Staged => "staged-tree-row",
        }
    }

    fn tick_id(self, folder: bool) -> &'static str {
        match (self, folder) {
            (Self::Staged, false) => "tick-staged-file",
            (Self::Staged, true) => "tick-staged-folder",
            (_, false) => "tick-unstaged-file",
            (_, true) => "tick-unstaged-folder",
        }
    }
}

impl GraphView {
    /// Puts files into the index, or takes them out, then reads the changes again so the
    /// lists show where they stand.
    pub fn stage_paths(&mut self, paths: Vec<String>, unstage: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        if paths.is_empty() || self.inspector.staging {
            return;
        }
        self.inspector.staging = true;
        Self::spawn_blocking(
            move || {
                if unstage {
                    working::unstage_files(&repo, &paths)
                } else {
                    working::stage_files(&repo, &paths)
                }
            },
            move |this, result, cx| {
                this.inspector.staging = false;
                if let Err(message) = result {
                    this.notice = Some(Notice::Error(message.into()));
                }
                this.reload_working(cx);
                cx.notify();
            },
            cx,
        );
    }

    fn closed_folders_mut(&mut self, pane: Pane) -> &mut HashSet<String> {
        match pane {
            Pane::Staged => &mut self.inspector.closed_staged,
            _ => &mut self.inspector.closed_folders,
        }
    }

    fn scroll_of(&self, pane: Pane) -> UniformListScrollHandle {
        match pane {
            Pane::Staged => self.inspector.staged_scroll.clone(),
            _ => self.inspector.files_scroll.clone(),
        }
    }

    /// The Tree and List chips, which every list in the inspector shares.
    fn render_view_chips(&self, cx: &mut Context<Self>) -> Div {
        div().h_flex().children(FileView::ALL.map(|view| {
            chip(
                ("inspector-files-view", view as usize),
                view.label(),
                view == self.commit.files_view,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_files_view(view, cx);
            }))
        }))
    }

    /// A commit's files, under a line that counts them.
    fn render_commit_files(
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
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!("{} changed", count(total, "file"))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .text_size(TEXT_CHIP)
                            .child(div().text_color(added).child(format!("+{additions}")))
                            .child(div().text_color(removed).child(format!("−{deletions}"))),
                    )
                    .child(self.render_view_chips(cx)),
            )
            .child(self.render_file_pane(Pane::Commit, details, cx))
            .when(is_merge && total == 0, |list| {
                list.child(
                    div()
                        .px_2()
                        .text_color(merge_note)
                        .child("This merge has no changes of its own."),
                )
            })
    }

    /// One of the two lists of working changes. Its box stages everything in it, or
    /// unstages everything, and a list with nothing in it says so.
    fn render_working_list(
        &self,
        side: Side,
        details: &Arc<CommitDetails>,
        cx: &mut Context<Self>,
    ) -> Div {
        let pane = Pane::of(side);
        let muted = cx.theme().muted_foreground;
        let paths: Vec<String> = self.working_changes().map_or_else(Vec::new, |changes| {
            changes.side(side).iter().map(|f| f.path.clone()).collect()
        });
        let empty = paths.is_empty();
        let unstage = side.unstages();
        let heading = if empty {
            side.heading().to_owned()
        } else {
            format!("{} · {}", side.heading(), count(paths.len(), "file"))
        };

        div()
            .v_flex()
            .min_h_0()
            .gap(px(2.))
            .mx(px(-8.))
            .when(empty, |list| list.flex_none())
            .when(!empty, |list| list.flex_1().min_h(px(110.)))
            .child(
                section_label(muted)
                    .h_flex()
                    .h(px(24.))
                    .px_2()
                    .gap_2()
                    .when(!empty, |header| {
                        header.child(
                            checkbox(("tick-all", side as usize), unstage, false, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.stage_paths(paths.clone(), unstage, cx)
                                }),
                            ),
                        )
                    })
                    .child(div().flex_1().min_w_0().truncate().child(heading)),
            )
            .when(empty, |list| {
                list.child(div().px_2().text_color(muted).child(side.empty()))
            })
            .when(!empty, |list| {
                list.child(self.render_file_pane(pane, details, cx))
            })
    }

    /// The rows of one list, in the chosen way of listing, drawing only those in view.
    fn render_file_pane(
        &self,
        pane: Pane,
        details: &Arc<CommitDetails>,
        cx: &mut Context<Self>,
    ) -> Div {
        let unstaged = self
            .working_changes()
            .map_or(0, |changes| changes.unstaged.len());
        let total = details.files.len();
        let range = match pane {
            Pane::Commit => 0..total,
            Pane::Unstaged => 0..unstaged.min(total),
            Pane::Staged => unstaged.min(total)..total,
        };
        let offset = range.start;
        let tree_view = self.commit.files_view == FileView::Tree;
        let closed = match pane {
            Pane::Staged => &self.inspector.closed_staged,
            _ => &self.inspector.closed_folders,
        };
        let rows: Arc<Vec<TreeRow>> = Arc::new(if tree_view {
            let paths: Vec<&str> = details.files[range.clone()]
                .iter()
                .map(|file| &*file.path)
                .collect();
            tree::rows(&paths, closed)
        } else {
            Vec::new()
        });
        let listed_rows = if tree_view { rows.len() } else { range.len() };
        let listed = details.clone();
        let scroll = self.scroll_of(pane);

        let list = div().relative().flex_1().min_h_0().child(
            uniform_list(
                pane.list_id(),
                listed_rows,
                cx.processor(move |this, rows_in_view: Range<usize>, _, cx| {
                    if tree_view {
                        return rows_in_view
                            .filter_map(|at| rows.get(at).map(|row| (at, row)))
                            .map(|(at, row)| {
                                this.render_details_tree_row(pane, offset, at, row, &listed, cx)
                            })
                            .collect::<Vec<_>>();
                    }
                    rows_in_view
                        .filter_map(|at| {
                            let file = listed.files.get(offset + at)?;
                            Some(this.render_file(pane, offset + at, file, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&scroll)
            .size_full(),
        );
        // A scrollbar takes its identity from the line that adds it, so each list adds its
        // own here. One shared line would make all of them answer to the same bar.
        match pane {
            Pane::Commit => list.scrollbar(&scroll, ScrollbarAxis::Vertical),
            Pane::Unstaged => list.scrollbar(&scroll, ScrollbarAxis::Vertical),
            Pane::Staged => list.scrollbar(&scroll, ScrollbarAxis::Vertical),
        }
    }

    /// The box at the start of a working row. Ticked means in the index, so an unticked
    /// file is staged by ticking it and a ticked one is unstaged by clearing it.
    fn tick_box(
        &self,
        pane: Pane,
        folder: bool,
        key: usize,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let side = pane.side()?;
        let unstage = side.unstages();
        Some(
            checkbox((pane.tick_id(folder), key), unstage, false, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    // The row under the box would otherwise open the diff.
                    cx.stop_propagation();
                    this.stage_paths(paths.clone(), unstage, cx);
                },
            )),
        )
    }

    /// A line of the tree: a folder that opens and closes, or a file stepped in under its
    /// folders. `offset` is where this list starts in the details, and `index` in a file
    /// row counts from there.
    fn render_details_tree_row(
        &self,
        pane: Pane,
        offset: usize,
        at: usize,
        row: &TreeRow,
        details: &CommitDetails,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let muted = cx.theme().muted_foreground;
        match row {
            TreeRow::Folder {
                path,
                name,
                depth,
                open,
                files,
            } => {
                let toggled = path.clone();
                let paths: Vec<String> = files
                    .iter()
                    .filter_map(|index| details.files.get(offset + index))
                    .map(|file| file.path.to_string())
                    .collect();
                let menu_path = path.clone();
                let menu_paths = paths.clone();
                panel_row((pane.tree_id(), at), false, cx)
                    .h(px(28.))
                    .pl(px(8. + INDENT * *depth as f32))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let closed = this.closed_folders_mut(pane);
                        if !closed.remove(&toggled) {
                            closed.insert(toggled.clone());
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let paths = menu_paths.clone();
                            this.open_folder_menu(
                                &menu_path,
                                paths,
                                pane.side(),
                                event.position,
                                cx,
                            );
                        }),
                    )
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(if *open { "▾" } else { "▸" }),
                    )
                    .children(self.tick_box(pane, true, at, paths, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(format!("{name}/")),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(files.len().to_string()),
                    )
            }
            TreeRow::File { index, name, depth } => {
                let index = offset + *index;
                let Some(file) = details.files.get(index) else {
                    return panel_row((pane.tree_id(), at), false, cx);
                };
                let shown = self.shown_file() == Some(index);
                let count = if file.binary {
                    "binary".to_owned()
                } else {
                    format!("+{} −{}", file.additions, file.deletions)
                };
                let paths = vec![file.path.to_string()];
                panel_row((pane.tree_id(), at), shown, cx)
                    .h(px(28.))
                    .pl(px(8. + 20. + INDENT * *depth as f32))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_file(index, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_file_menu(index, event.position, cx);
                        }),
                    )
                    .children(self.tick_box(pane, false, index, paths, cx))
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .font_medium()
                            .text_color(status_color(file.status, cx))
                            .child(file.status.to_string()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(count),
                    )
            }
        }
    }

    /// A file's row. `index` is its place in the details, which is what a click opens.
    fn render_file(
        &self,
        pane: Pane,
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
        panel_row((pane.row_id(), index), shown, cx)
            .h(px(28.))
            .on_click(cx.listener(move |this, _, _, cx| this.show_file(index, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_file_menu(index, event.position, cx);
                }),
            )
            .children(self.tick_box(pane, false, index, vec![file.path.to_string()], cx))
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
