//! The panel beside the graph that shows the selected commit.

pub(super) mod commit_form;
mod files;

use super::parts::TEXT_SMALL;
use super::parts::{loading, title};
use super::{GraphView, Status};
use crate::theme::tokens;
use chrono::{Local, TimeZone};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mantisgit_core::cache::keys::Caches;
use mantisgit_core::commit::actions;
use mantisgit_core::commit::details::{self, CommitDetails};
use mantisgit_core::history::{Commit, Entry, split_subject};
use mantisgit_core::ops::{self, Cancel};
use mantisgit_core::text::short_oid;
use mantisgit_core::working::{self, Changes, Side, WorkingFile};
use std::collections::HashSet;
use std::path::PathBuf;
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
    /// Set when another refresh came in during that read, which then runs once more, as
    /// widely as the widest asked for.
    read_working_again: Option<Scan>,
    /// The read in progress, as far as it has come.
    read: WorkingRead,
    /// Set while git walks the working tree for untracked files, after everything else is
    /// in. The list shows what it has, and says more may follow.
    pub(super) scanning_untracked: bool,
}

/// How much of the working changes a read looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Scan {
    /// Only the files git tracks. The untracked ones stay as the last read had them, which
    /// is right when nothing happened that could add or remove one, such as saving a file
    /// that was in conflict.
    Tracked,
    /// Every file, walking the whole tree for the untracked ones, which can take seconds.
    #[default]
    Everything,
}

/// The parts of one read of the working changes, which arrive apart: the tracked files
/// are quick, and the walk for untracked ones is not.
#[derive(Default)]
struct WorkingRead {
    scan: Scan,
    tracked: Option<Result<Changes, String>>,
    /// Stays `None` for a read that doesn't look for them.
    untracked: Option<Result<Vec<WorkingFile>, String>>,
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
        let (repo_again, caches_again, fingerprint_again) =
            (repo.clone(), caches.clone(), fingerprint.clone());
        Self::spawn_blocking(
            move || {
                // The disk is read here too, so a commit seen in an earlier run is quick.
                let cached = fingerprint
                    .as_deref()
                    .and_then(|fingerprint| caches.cached_details(fingerprint, &wanted));
                match cached {
                    Some(details) => Ok(details),
                    // Only the names: counting diffs every file, which can take long.
                    None => details::read_names(&repo, &wanted, &cancel).map(Arc::new),
                }
            },
            move |this, details, cx| {
                // Only the newest read has anything to say: an older one was cancelled.
                if finished.is_cancelled() || this.selected_oid.as_ref() != Some(&oid) {
                    return;
                }
                let names = details.ok();
                let counting = names.clone().filter(|names| !names.counted);
                if counting.is_none() {
                    this.inspector.loading = None;
                }
                this.inspector.details = names;
                this.inspector
                    .files_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                this.details_loaded(cx);
                cx.notify();
                if let Some(names) = counting {
                    this.count_files(
                        names,
                        repo_again,
                        caches_again,
                        fingerprint_again,
                        finished,
                        cx,
                    );
                }
            },
            cx,
        );
    }

    /// Reads the `+n −n` of a commit's files, which are on show without them by now, and
    /// swaps the counted details in. Stops with the selection, like the read before it.
    fn count_files(
        &mut self,
        names: Arc<CommitDetails>,
        repo: PathBuf,
        caches: Caches,
        fingerprint: Option<Arc<str>>,
        cancel: Cancel,
        cx: &mut Context<Self>,
    ) {
        let oid = names.oid.clone();
        let finished = cancel.clone();
        Self::spawn_blocking(
            move || {
                let counted = details::count(&repo, (*names).clone(), &cancel)?;
                Ok::<_, String>(match fingerprint {
                    Some(fingerprint) => caches.store_details(&fingerprint, counted),
                    None => Arc::new(counted),
                })
            },
            move |this, counted, cx| {
                if finished.is_cancelled() || this.selected_oid.as_deref() != Some(&*oid) {
                    return;
                }
                this.inspector.loading = None;
                // The files are the same and in the same order, so the open diff stays.
                if let Ok(counted) = counted {
                    this.inspector.details = Some(counted);
                }
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
        self.reload_working_as(Scan::Everything, cx);
    }

    /// Reads the uncommitted changes as far as `scan` says. The tracked files go on show
    /// as soon as they are read, without waiting for the walk for untracked ones.
    pub(super) fn reload_working_as(&mut self, scan: Scan, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        if self.inspector.reading_working {
            let wider = self
                .inspector
                .read_working_again
                .map_or(scan, |a| a.max(scan));
            self.inspector.read_working_again = Some(wider);
            return;
        }
        // With nothing read yet there are no untracked files to keep.
        let scan = if self.inspector.working.is_none() {
            Scan::Everything
        } else {
            scan
        };
        self.inspector.reading_working = true;
        self.inspector.read = WorkingRead {
            scan,
            ..WorkingRead::default()
        };
        let cancel = Cancel::new();
        self.inspector.loading = Some(cancel.clone());
        if scan == Scan::Everything {
            let (repo, cancel) = (repo.clone(), cancel.clone());
            Self::spawn_blocking(
                move || working::read_untracked(&repo, &cancel),
                |this, found, cx| {
                    this.inspector.read.untracked = Some(found);
                    this.working_read_arrived(cx);
                },
                cx,
            );
        }
        Self::spawn_blocking(
            move || working::read_tracked(&repo, &cancel),
            |this, found, cx| {
                this.inspector.read.tracked = Some(found);
                this.working_read_arrived(cx);
            },
            cx,
        );
    }

    /// Takes in one part of the read. Once the tracked files are in they are shown, and
    /// once everything is in the read is over.
    fn working_read_arrived(&mut self, cx: &mut Context<Self>) {
        let read = &self.inspector.read;
        let Some(tracked) = &read.tracked else {
            return;
        };
        if read.scan == Scan::Everything && read.untracked.is_none() {
            let stale = self.inspector.read_working_again.is_some();
            if let (false, true, Ok(tracked)) = (stale, self.working_row_is_selected(), tracked) {
                let changes = tracked.clone().with_untracked(self.kept_untracked());
                self.inspector.scanning_untracked = true;
                self.show_working(changes, cx);
                cx.notify();
            }
            return;
        }

        let read = std::mem::take(&mut self.inspector.read);
        self.inspector.reading_working = false;
        self.inspector.scanning_untracked = false;
        let again = self.inspector.read_working_again.take();
        if !self.working_row_is_selected() {
            return;
        }
        let gone = || Err("git stopped".to_owned());
        let (tracked, untracked) = (read.tracked.unwrap_or_else(gone), read.untracked);
        let stopped = |error: &String| error == ops::CANCELLED;
        let cancelled = tracked.as_ref().is_err_and(stopped)
            || untracked
                .as_ref()
                .is_some_and(|found| found.as_ref().is_err_and(stopped));
        if again.is_some() || cancelled {
            // Out of date, or stopped: only the newest read has anything to say.
            return self.reload_working_as(again.unwrap_or(read.scan), cx);
        }
        self.inspector.loading = None;
        let changes = match (tracked, untracked) {
            (Err(error), _) | (_, Some(Err(error))) => Err(error),
            (Ok(tracked), Some(Ok(found))) => Ok(tracked.with_untracked(found)),
            (Ok(tracked), None) => Ok(tracked.with_untracked(self.kept_untracked())),
        };
        match changes {
            Ok(changes) => self.show_working(changes, cx),
            Err(_) => self.inspector.working = None,
        }
        cx.notify();
    }

    /// The untracked files of the last read, for a read that doesn't look for them again.
    fn kept_untracked(&self) -> Vec<WorkingFile> {
        (self.inspector.working.as_ref())
            .map_or_else(Vec::new, |working| working.changes.untracked())
    }

    /// Puts the working changes on show, unless they are what is there already.
    fn show_working(&mut self, changes: Changes, cx: &mut Context<Self>) {
        let same = (self.inspector.working.as_ref()).is_some_and(|w| w.changes == changes);
        if same {
            // Nothing in the list moved, so only an open diff may have changed.
            if self.diff_is_open() {
                self.working_loaded(cx);
            }
            return;
        }
        let details = CommitDetails {
            oid: WORKING_ID.into(),
            body: String::new(),
            merge: false,
            files: changes.all().map(WorkingFile::change).collect(),
            counted: true,
        };
        self.inspector.working = Some(Working {
            changes,
            details: Arc::new(details),
        });
        self.working_loaded(cx);
        cx.notify();
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
                            // Lists take only the room their rows need; the form stays at
                            // the foot.
                            .child(div().flex_1().min_h_0())
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
                    move |repo| actions::check_out(repo, branch.as_deref(), &oid),
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
                    move |repo| actions::cherry_pick(repo, &oid),
                    cx,
                );
            })
        };
        let revert = cx.listener(move |this, _, _, cx| {
            let oid = oid.clone();
            this.run_operation(
                "Reverting",
                move |repo| actions::revert(repo, &oid, is_merge),
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
