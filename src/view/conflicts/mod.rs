//! A paused rebase, cherry-pick, revert or merge. The inspector lists the files that need a
//! decision. A file opens in the resolver over the graph: its conflicts side by side, to take
//! whole or line by line, and the file's result in an editor, saved with one button.

mod resolver;

use resolver::FileView;

use super::dialog::Confirm;
use super::inspector::Scan;
use super::parts::{
    TEXT_CHIP, TEXT_SMALL, checkbox, expand_tabs, file_path, loading, panel_row, pill,
    scroll_marks, section_label,
};
use super::{GraphView, Notice};
use crate::theme::{lane_color, tokens};
use gitclient_core::repo::conflict::{self, Decision, Piece, Progress, ResultCheck, Side};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

/// Rows are as tall as the diff's, so that a file reads the same in both.
const ROW_HEIGHT: Pixels = px(24.);
const NUMBER_WIDTH: Pixels = px(48.);
/// How long typing in the result pauses before it is read for the conflicts left in it, so
/// that a large file is read once per pause and not once per key.
const CHECK_AFTER: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(super) struct State {
    progress: Option<Arc<Progress>>,
    /// Whether the resolver is open over the graph.
    pub(super) open: bool,
    /// The file on show in the resolver.
    shown: Option<String>,
    /// Each file's result so far. It starts as the file with its markers, and is edited
    /// from there, by picking from the sides above it or by hand. Shared, so that reading
    /// it in the background copies nothing.
    drafts: HashMap<String, Arc<str>>,
    /// Whether the editor holds typing that its file's draft doesn't have yet. The editor's
    /// text is copied into the draft when it is next needed, not on every key.
    behind: bool,
    /// Counts the edits to results, so that a check started before the latest one is dropped.
    revision: u64,
    /// Each file laid out for its list, built in the background when it is opened.
    views: HashMap<String, Arc<FileView>>,
    /// For each file, which of its conflicts still stand in its result. Worked out when the
    /// result changes, in the background while typing, so that drawing never reads the file.
    standing: HashMap<String, Vec<bool>>,
    /// For each file, the lines each of its conflicts takes in its result, worked out with
    /// `standing`, which lines the result up with the two versions above it.
    places: HashMap<String, Vec<Range<usize>>>,
    /// For each file, the text the ticks put in place of each conflict, if they did, which
    /// is how a picked conflict is found in the result again.
    picked: HashMap<String, Vec<Option<Arc<str>>>>,
    /// For each file, where each conflict is in its result, in bytes, as of the last check:
    /// `None` for one edited by hand, which ticks no longer change.
    regions: HashMap<String, Vec<Option<Range<usize>>>>,
    /// For each file, its conflicts' marker blocks as git wrote them, which a conflict goes
    /// back to when nothing is ticked in it.
    originals: HashMap<String, Arc<Vec<String>>>,
    /// Where the list and the result were last seen, in rows and lines, after one was moved
    /// to follow the other.
    synced: Option<(f32, f32)>,
    /// Set for the frame after one was moved to follow the other, which takes it as the new
    /// place of both rather than as a scroll to follow.
    settling: bool,
    /// The result editor's scroll offset when last seen, so that only a change of it, and
    /// not the cursor blinking, draws the window again.
    result_offset: Option<Pixels>,
    scroll: UniformListScrollHandle,
    /// The side picked for a file that can only go one way as a whole, such as a binary one.
    sides: HashMap<String, Side>,
    /// The lines ticked in each file's conflicts, by conflict: side and line. The result is
    /// built from them: a conflict with an entry has its ticked lines in the result, even
    /// none (a side with no lines taken), and one without keeps its markers.
    picks: HashMap<String, HashMap<usize, HashSet<(Side, usize)>>>,
    /// Files saved at this stop. They stay listed until the operation goes on.
    resolved: Vec<String>,
    /// The file being saved, which already counts as resolved on screen while git writes it.
    saving: Option<String>,
    /// Files git has written and staged, whose conflict git hasn't yet been seen to drop.
    /// They are listed as resolved all the same, with a spinner until it is.
    unconfirmed: HashSet<String>,
    /// Counts the reads of the paused operation, so that one started before the latest is
    /// dropped: it may not know of a file saved since.
    check_seq: u64,
    /// An editor for each file whose result has been on show at this stop. Loading a large
    /// file into an editor takes a while, so coming back to one doesn't do it again.
    editors: HashMap<String, ResultEditor>,
    /// The file on show whose editor holds its result.
    editing: Option<String>,
    /// The conflict the list last went to with Previous or Next, in the file on show.
    jumped: Option<usize>,
    /// Set while continuing, so that the check afterwards can tell finished from cancelled.
    continuing: Option<Arc<Progress>>,
    rebase_target: Option<String>,
}

/// The editor of one file's result.
struct ResultEditor {
    state: Entity<TextareaState>,
    /// Whether it holds the file's result yet. It is made empty and drawn once before the
    /// result goes in, which costs far less for a large file than drawing it full at first.
    loaded: bool,
    _changes: Subscription,
    _scrolled: Subscription,
}

impl State {
    /// Forgets everything about the stop that was open.
    fn reset(&mut self) {
        self.open = false;
        self.shown = None;
        self.drafts.clear();
        self.views.clear();
        self.standing.clear();
        self.places.clear();
        self.picked.clear();
        self.regions.clear();
        self.originals.clear();
        self.synced = None;
        self.result_offset = None;
        self.sides.clear();
        self.picks.clear();
        self.resolved.clear();
        self.saving = None;
        self.unconfirmed.clear();
        self.editors.clear();
        self.editing = None;
        self.behind = false;
        self.jumped = None;
    }

    /// The editor holding the result of the file on show, once it does.
    fn active_editor(&self) -> Option<&Entity<TextareaState>> {
        let editor = self.editors.get(self.editing.as_deref()?)?;
        editor.loaded.then_some(&editor.state)
    }
}

impl GraphView {
    pub fn paused_operation(&self) -> Option<&Arc<Progress>> {
        self.conflict.progress.as_ref()
    }

    /// Whether a conflicted file is open over the graph.
    pub fn resolver_is_open(&self) -> bool {
        self.conflict.open && self.conflict.progress.is_some()
    }

    /// Where the list and the result are scrolled to, in rows and lines.
    #[cfg(test)]
    pub(super) fn resolver_scroll(&self, cx: &App) -> (f32, f32) {
        let list = self.conflict.scroll.0.borrow().base_handle.offset().y;
        let editor = self.conflict.active_editor().unwrap().read(cx);
        let line_height = editor.line_height().unwrap();
        (
            -f32::from(list) / f32::from(ROW_HEIGHT),
            -f32::from(editor.scroll_offset().y) / f32::from(line_height),
        )
    }

    /// The files listed as resolved, and whether git is still on each ("Saving…").
    #[cfg(test)]
    pub(super) fn saved_files(&self) -> Vec<(String, bool)> {
        let settling = |path: &String| {
            self.conflict.saving.as_ref() == Some(path) || self.conflict.unconfirmed.contains(path)
        };
        (self.conflict.resolved.iter())
            .map(|path| (path.clone(), settling(path)))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn conflict_editor(&self) -> Option<Entity<TextareaState>> {
        self.conflict.active_editor().cloned()
    }

    /// Looks for a paused operation, which can appear or go away outside the app too.
    pub(super) fn check_paused_operation(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        self.conflict.check_seq += 1;
        let seq = self.conflict.check_seq;
        Self::spawn_blocking(
            move || (conflict::detect(&repo), conflict::rebase_target(&repo)),
            move |this, read, cx| {
                // A newer read is on its way, and knows more.
                if this.conflict.check_seq == seq {
                    this.paused_operation_read(read, cx);
                }
            },
            cx,
        );
    }

    /// A file was saved, and git took it. It is resolved for certain, which the screen
    /// already says, so what is left is to read what git reports and to say that this is
    /// under way.
    pub(super) fn conflict_written(&mut self, cx: &mut Context<Self>) {
        if let Some(saved) = self.conflict.saving.take() {
            self.conflict.unconfirmed.insert(saved);
        }
        self.check_paused_operation(cx);
        if self.working_row_is_selected() {
            self.reload_working_as(Scan::Tracked, cx);
        }
        cx.notify();
    }

    /// Git refused to save a file after the screen counted it resolved: it is back among
    /// the files to decide, open, with what was typed in it, and the band says why.
    pub(super) fn conflict_not_written(&mut self, cx: &mut Context<Self>) {
        if let Some(saved) = self.conflict.saving.take() {
            self.conflict.resolved.retain(|path| *path != saved);
            self.show_conflict(&saved, cx);
        }
    }

    /// The files with a decision still to make, leaving out those saved, which git may not
    /// have dropped from its list yet.
    fn files_to_decide(&self, progress: &Progress) -> Vec<conflict::ConflictFile> {
        let mut files = progress.files();
        files.retain(|file| !self.conflict.resolved.contains(&file.path));
        files
    }

    fn paused_operation_read(
        &mut self,
        (progress, target): (Option<Progress>, Option<String>),
        cx: &mut Context<Self>,
    ) {
        self.conflict.rebase_target = target;
        let continuing = self.conflict.continuing.take();
        let failed = matches!(self.notice, Some(Notice::Error(_)));
        match progress {
            Some(new) => {
                // Saving a file changes the decisions but not where the operation stands.
                let same =
                    (self.conflict.progress.as_deref()).is_some_and(|old| old.is_same_stop(&new));
                let new = Arc::new(new);
                if same {
                    self.conflict.progress = Some(new.clone());
                    self.after_save(&new, cx);
                } else {
                    self.conflict.reset();
                    self.conflict.progress = Some(new);
                    self.focus_conflicts(cx);
                }
            }
            None => {
                self.conflict.reset();
                self.conflict.progress = None;
                if let Some(finished) = continuing.filter(|_| !failed) {
                    self.notice = Some(Notice::Info(finished.finished_sentence().into()));
                }
            }
        }
        cx.notify();
    }

    /// Once git is seen to have dropped the files it was given, they are confirmed, and the
    /// resolver closes if there are none left to decide.
    fn after_save(&mut self, progress: &Progress, cx: &mut Context<Self>) {
        let awaited = std::mem::take(&mut self.conflict.unconfirmed);
        if awaited.is_empty() {
            return;
        }
        let files = progress.files();
        for saved in awaited {
            if files.iter().any(|file| file.path == saved) {
                // Git still has it in conflict, which is the last word: it needs deciding.
                self.conflict.resolved.retain(|path| *path != saved);
                continue;
            }
            self.conflict.drafts.remove(&saved);
            self.conflict.sides.remove(&saved);
            self.conflict.editors.remove(&saved);
            self.conflict.picks.remove(&saved);
            self.conflict.picked.remove(&saved);
            if self.conflict.editing.as_deref() == Some(saved.as_str()) {
                self.conflict.editing = None;
                self.conflict.behind = false;
            }
        }
        // The screen moved on to the next file when this one was saved, so what is left is
        // to say so when it was the last.
        if self.files_to_decide(progress).is_empty() {
            self.conflict.open = false;
            self.notice = Some(Notice::Info(progress.resolved_sentence().into()));
        }
        cx.notify();
    }

    /// Shows the working changes, where the conflicts are listed, so that a paused
    /// operation is never out of sight.
    pub(super) fn focus_conflicts(&mut self, cx: &mut Context<Self>) {
        if self.conflict.progress.is_none() || self.working_row_is_selected() {
            return;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.working) {
            self.select(index, cx);
        }
    }

    pub(super) fn render_rebase_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.conflict.rebase_target.clone()?;
        Some(
            Button::new("rebase")
                .ghost()
                .small()
                .label(format!("Rebase onto {target}"))
                .disabled(self.busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let target = target.clone();
                    let confirm = Confirm::banner(
                        conflict::rebase_warning(&target),
                        "Rebase",
                        move |this, cx| {
                            let target = target.clone();
                            this.run_operation(
                                "Rebasing",
                                move |repo| conflict::rebase_onto(repo, &target),
                                cx,
                            );
                        },
                    );
                    this.ask(confirm, cx);
                }))
                .into_any_element(),
        )
    }

    /// The conflicts of a file that has lines to merge, in the order they come.
    fn file_decisions<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a Decision> + 'a {
        let decisions = self.conflict.progress.iter().flat_map(|p| &p.decisions);
        decisions.filter(move |decision| decision.path == path && !decision.whole_file)
    }

    /// Whether a file has no lines to merge, such as a binary one, and is one choice.
    fn is_whole_file(&self, path: &str) -> bool {
        let mut decisions = self.conflict.progress.iter().flat_map(|p| &p.decisions);
        decisions.any(|decision| decision.path == path && decision.whole_file)
    }

    /// How many conflicts a file still has in its result.
    pub(super) fn conflicts_left(&self, path: &str) -> usize {
        if self.is_whole_file(path) {
            return usize::from(!self.conflict.sides.contains_key(path));
        }
        match self.conflict.standing.get(path) {
            Some(open) => open.iter().filter(|open| **open).count(),
            None => self.file_decisions(path).count(),
        }
    }

    /// Works out which of a file's conflicts still stand in its result, here and now. For
    /// a click, where it has to be right before anything else happens; typing uses
    /// [`Self::check_later`].
    fn refresh_open(&mut self, path: &str) {
        let Some(check) = self.check_of(path) else {
            return;
        };
        let found = check.run();
        self.apply_check(path, found);
        // A check still on its way read an older result.
        self.conflict.revision += 1;
    }

    /// What checking a file's result needs, gathered so that it can be done on another
    /// thread.
    fn check_of(&self, path: &str) -> Option<Check> {
        Some(Check {
            progress: self.conflict.progress.clone()?,
            path: path.to_owned(),
            text: self.conflict.drafts.get(path).cloned()?,
            view: self.conflict.views.get(path).cloned(),
            picked: self.conflict.picked.get(path).cloned().unwrap_or_default(),
        })
    }

    fn apply_check(&mut self, path: &str, found: ResultCheck) {
        self.conflict
            .standing
            .insert(path.to_owned(), found.standing);
        self.conflict.places.insert(path.to_owned(), found.places);
        self.conflict.regions.insert(path.to_owned(), found.regions);
    }

    /// Copies typing in the editor into its file's draft, if there is any it doesn't have.
    fn catch_up_draft(&mut self, cx: &App) {
        if !self.conflict.behind {
            return;
        }
        self.conflict.behind = false;
        if let (Some(path), Some(editor)) = (&self.conflict.editing, self.conflict.active_editor())
        {
            let text: Arc<str> = editor.read(cx).text().to_string().into();
            self.conflict.drafts.insert(path.clone(), text);
        }
    }

    /// Notes an edit to a result and checks it for conflicts once edits pause. A check
    /// that an edit overtakes is dropped, so typing never waits on reading the file.
    fn check_later(&mut self, cx: &mut Context<Self>) {
        self.conflict.revision += 1;
        let revision = self.conflict.revision;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CHECK_AFTER).await;
            this.update(cx, |this, cx| this.check_in_background(revision, cx))
                .ok();
        })
        .detach();
    }

    fn check_in_background(&mut self, revision: u64, cx: &mut Context<Self>) {
        if self.conflict.revision != revision {
            return;
        }
        self.catch_up_draft(cx);
        let Some(path) = self.conflict.editing.clone() else {
            return;
        };
        let Some(check) = self.check_of(&path) else {
            return;
        };
        Self::spawn_blocking(
            move || check.run(),
            move |this, found, cx| {
                if this.conflict.revision == revision {
                    this.apply_check(&path, found);
                    cx.notify();
                }
            },
            cx,
        );
    }

    /// Sees a move of the result editor's scroll, and draws the window again so that the
    /// list follows it. Other changes to the editor, such as the cursor blinking, don't.
    fn result_scrolled(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.conflict.active_editor() else {
            return;
        };
        let offset = editor.read(cx).scroll_offset().y;
        if self.conflict.result_offset != Some(offset) {
            self.conflict.result_offset = Some(offset);
            cx.notify();
        }
    }

    /// Keeps the list and the result at the same place in the file: whichever moved since
    /// the last frame, the other follows, line for line between the conflicts.
    pub(super) fn sync_result_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.conflict.open {
            return;
        }
        let (Some(path), Some(editor)) = (
            self.conflict.editing.clone(),
            self.conflict.active_editor().cloned(),
        ) else {
            return;
        };
        let (Some(view), Some(places)) = (
            self.conflict.views.get(&path).cloned(),
            self.conflict.places.get(&path),
        ) else {
            return;
        };
        let Some(line_height) = editor.read(cx).line_height() else {
            // Not drawn yet, so it has no place to follow or be followed from.
            return;
        };
        let list_offset = self.conflict.scroll.0.borrow().base_handle.offset();
        let result_offset = editor.read(cx).scroll_offset();
        let list_at = -f32::from(list_offset.y) / f32::from(ROW_HEIGHT);
        let result_at = -f32::from(result_offset.y) / f32::from(line_height);
        let same = |a: f32, b: f32| (a - b).abs() < 0.01;

        if std::mem::take(&mut self.conflict.settling) {
            // What the follower came to, after keeping to its own limits.
            self.conflict.synced = Some((list_at, result_at));
            return;
        }
        let anchors = view.anchors(places);
        match self.conflict.synced {
            Some((list, result)) if same(list, list_at) && same(result, result_at) => {}
            // The result moved, and the list follows it.
            Some((list, _)) if same(list, list_at) => {
                let row = conflict::line_up(&anchors, result_at, true).max(0.);
                let y = -px(row * f32::from(ROW_HEIGHT));
                let scroll = self.conflict.scroll.0.borrow();
                scroll.base_handle.set_offset(point(list_offset.x, y));
                drop(scroll);
                self.conflict.settling = true;
                draw_again(cx);
            }
            // The list moved, or the two were never lined up: the result follows the list.
            _ => {
                let line = conflict::line_up(&anchors, list_at, false).max(0.);
                let y = -px(line * f32::from(line_height));
                editor.update(cx, |editor, cx| {
                    editor.set_scroll_offset(point(result_offset.x, y), cx)
                });
                self.conflict.settling = true;
                draw_again(cx);
            }
        }
    }

    /// Moves the operation on once every file is resolved.
    pub(super) fn continue_operation(&mut self, cx: &mut Context<Self>) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        if !self.files_to_decide(&progress).is_empty() {
            return;
        }
        let kind = progress.kind;
        self.conflict.continuing = Some(progress.clone());
        let label = format!("Continuing {}", kind.title().to_lowercase());
        self.run_operation(
            label,
            move |repo| conflict::carry_on(repo, kind).map(|_| None),
            cx,
        );
    }

    fn cancel_operation(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.conflict.progress.as_ref().map(|p| p.kind) else {
            return;
        };
        self.run_operation("Cancelling", move |repo| conflict::cancel(repo, kind), cx);
    }

    /// The inspector while an operation is paused: what happened, the files that need a
    /// decision, and the way out either side.
    pub(super) fn render_conflict_panel(&self, progress: &Progress, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let colors = tokens(theme.is_dark());
        let (border, muted, success) = (theme.border, theme.muted_foreground, theme.success);
        let kind = progress.kind.title().to_lowercase();
        let files = self.files_to_decide(progress);
        let ready = files.is_empty();
        let busy = self.busy.is_some();
        let shown_path = self.conflict.shown.clone().filter(|_| self.conflict.open);

        let mut rows: Vec<Stateful<Div>> = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let left = self.conflicts_left(&file.path);
            let (path, menu_path) = (file.path.clone(), file.path.clone());
            rows.push(
                panel_row(
                    ("conflict-file", index),
                    shown_path.as_deref() == Some(file.path.as_str()),
                    cx,
                )
                .h(px(28.))
                .on_click(cx.listener(move |this, _, _, cx| this.show_conflict(&path, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_conflict_menu(&menu_path, event.position, cx);
                    }),
                )
                .child(
                    div()
                        .w(px(12.))
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .font_semibold()
                        .text_color(colors.peach)
                        .child("!"),
                )
                .child(file_path(&file.path, muted))
                .child(
                    div()
                        .flex_none()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child(file.note(left)),
                ),
            );
        }
        // Saved files stay in the list, ticked, until the operation goes on. One git hasn't
        // finished with has a spinner and says "Saving…", though it is resolved for sure.
        for (index, path) in self.conflict.resolved.iter().enumerate() {
            let settling = self.conflict.saving.as_ref() == Some(path)
                || self.conflict.unconfirmed.contains(path);
            rows.push(
                panel_row(("resolved-file", index), false, cx)
                    .h(px(28.))
                    .cursor_default()
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .h_flex()
                            .text_size(TEXT_CHIP)
                            .font_semibold()
                            .text_color(success)
                            .child(if settling {
                                Spinner::new().xsmall().into_any_element()
                            } else {
                                "✓".into_any_element()
                            }),
                    )
                    .child(file_path(path, muted))
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(if settling { "Saving…" } else { "Resolved" }),
                    ),
            );
        }

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
                    .child(div().h_flex().gap_2().child(pill(
                        &format!("{} paused", progress.kind.title()),
                        colors.peach,
                        cx,
                    )))
                    .child(div().text_color(colors.subtext0).child(progress.sentence())),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.))
                    .mx(px(-8.))
                    .child(
                        section_label(muted)
                            .h(px(24.))
                            .px_2()
                            .h_flex()
                            .child(progress.list_heading(self.conflict.resolved.len())),
                    )
                    .child(
                        div()
                            .id("conflict-files")
                            .v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .flex_none()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .text_color(muted)
                            .child(progress.cancel_note()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("cancel-operation")
                                    .ghost()
                                    .label(format!("Cancel {kind}"))
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_operation(cx)),
                                    ),
                            )
                            .child(
                                Button::new("continue-operation")
                                    .primary()
                                    .label(format!("Continue {kind}"))
                                    .disabled(busy || !ready)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.continue_operation(cx)),
                                    ),
                            ),
                    ),
            )
    }
}

/// What it takes to check a file's result, on any thread.
struct Check {
    progress: Arc<Progress>,
    path: String,
    text: Arc<str>,
    /// The file laid out for the list, which says how many lines come between conflicts.
    /// Without it the places are only a guess, which matters to nothing until it is drawn.
    view: Option<Arc<FileView>>,
    picked: Vec<Option<Arc<str>>>,
}

impl Check {
    fn run(&self) -> ResultCheck {
        let agreed = self.view.as_ref().map_or(&[][..], |view| view.agreed());
        (self.progress).check_result(&self.path, &self.text, agreed, &self.picked)
    }
}

/// Asks for one more frame, from inside drawing one, where a notify asks for none: the
/// task runs once this frame is done.
fn draw_again(cx: &mut Context<GraphView>) {
    cx.spawn(async move |this, cx| this.update(cx, |_, cx| cx.notify()).ok())
        .detach();
}
