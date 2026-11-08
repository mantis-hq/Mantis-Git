//! The diff of one file, drawn over the whole graph pane while it is read.

use super::parts::{LineStyle, checkbox, chip, expand_tabs, file_path, line_style};
use super::parts::{TEXT_BODY, TEXT_CHIP, TEXT_SMALL};
use super::{GraphView, Notice};
use crate::theme::tokens;
use gitclient_core::details::{self, FileChange};
use gitclient_core::patch::{DiffView, FilePatch, Line};
use gitclient_core::working::{self, Side};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::sync::Arc;

const ROW_HEIGHT: Pixels = px(22.);
const NUMBER_WIDTH: Pixels = px(40.);
const SIGN_WIDTH: Pixels = px(28.);

#[derive(Default)]
pub(super) struct State {
    /// The file on show in the selected commit, by position.
    file: Option<usize>,
    /// Closing stays closed until a file is picked, however the selection moves.
    closed: bool,
    patch: Option<Arc<Shown>>,
    /// Whole file or just the changes, and whether white space counts. It stays as it is
    /// set while other files and commits are picked.
    view: DiffView,
    scroll: UniformListScrollHandle,
}

/// A patch flattened into the rows of a list.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shown {
    oid: Arc<str>,
    path: Arc<str>,
    binary: bool,
    /// For the working changes, the list the file is in.
    side: Option<Side>,
    /// Whether each hunk has a box that moves it in or out of the index.
    ticks: bool,
    rows: Vec<Row>,
    /// The row with the longest text, which sets how far the list scrolls sideways.
    widest: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum Row {
    /// `index` is the hunk's place in the file's patch, which is what moving it names.
    Hunk {
        title: String,
        index: usize,
    },
    Line(Line),
    /// Space between two hunks, drawn as nothing. Rows share one height, so it is a row.
    Gap,
}

impl Shown {
    fn new(oid: Arc<str>, file: &FileChange, patch: &FilePatch) -> Self {
        let mut rows = Vec::new();
        for (index, hunk) in patch.hunks.iter().enumerate() {
            rows.push(Row::Gap);
            let title = match (hunk.title.as_str(), hunk.lines.first()) {
                ("", Some(first)) => format!("From line {}", first.new.or(first.old).unwrap_or(1)),
                (title, _) => title.to_owned(),
            };
            rows.push(Row::Hunk { title, index });
            rows.extend(hunk.lines.iter().cloned().map(Row::Line));
        }
        let width = |row: &Row| match row {
            Row::Hunk { title, .. } => title.len(),
            Row::Line(line) => line.text.len(),
            Row::Gap => 0,
        };
        let widest = (0..rows.len()).max_by_key(|&index| width(&rows[index]));
        Self {
            oid,
            path: file.path.clone(),
            binary: patch.binary || file.binary,
            side: None,
            ticks: false,
            rows,
            widest,
        }
    }

    /// Marks this as a working file in `side`, whose hunks move when `ticks` is set.
    fn staging(mut self, side: Side, ticks: bool) -> Self {
        self.side = Some(side);
        self.ticks = ticks;
        self
    }
}

impl GraphView {
    pub(super) fn shown_file(&self) -> Option<usize> {
        self.diff.file.filter(|_| !self.diff.closed)
    }

    /// The selected commit's details just arrived. The diff stays hidden until
    /// a file is picked, so selecting a commit never opens one on its own.
    pub(super) fn details_loaded(&mut self, _cx: &mut Context<Self>) {
        self.diff.patch = None;
        self.diff.file = None;
    }

    /// The working changes were read again. A file already on show stays, redrawn from
    /// the new read, so a refresh never closes what is being read. One that has moved to
    /// the other list is followed there.
    pub(super) fn working_loaded(&mut self, cx: &mut Context<Self>) {
        let on_show = self
            .shown_file()
            .and(self.diff.patch.as_ref())
            .and_then(|shown| Some((shown.path.clone(), shown.side?)));
        let same = on_show.and_then(|(path, side)| {
            let other = match side {
                Side::Unstaged => Side::Staged,
                Side::Staged => Side::Unstaged,
            };
            self.working_position(side, &path)
                .or_else(|| self.working_position(other, &path))
        });
        match same {
            Some(index) => self.load_file(index, cx),
            None => self.details_loaded(cx),
        }
    }

    /// Whether a file's diff is open, and so covers the graph.
    pub(super) fn diff_is_open(&self) -> bool {
        self.shown_file().is_some() && self.diff.patch.is_some()
    }

    pub(super) fn set_diff_view(&mut self, view: DiffView, cx: &mut Context<Self>) {
        if view == self.diff.view {
            return;
        }
        self.diff.view = view;
        if let Some(index) = self.shown_file() {
            self.load_file(index, cx);
        }
    }

    pub fn show_file(&mut self, index: usize, cx: &mut Context<Self>) {
        self.conflict.open = false;
        self.diff.closed = false;
        self.load_file(index, cx);
    }

    fn load_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.working_row_is_selected() {
            return self.load_working_file(index, cx);
        }
        let (Some(repo), Some(details)) = (self.workdir(), self.selected_details().cloned()) else {
            return;
        };
        let Some(file) = details.files.get(index).cloned() else {
            return;
        };
        self.diff.file = Some(index);
        cx.notify();

        let oid = details.oid.clone();
        let fingerprint = self.fingerprint.clone();
        let caches = self.caches.clone();
        let view = self.diff.view;
        // Even a cached diff is flattened into rows in the background, since a large one
        // takes a while to lay out.
        Self::spawn_blocking(
            move || {
                // Only the plain view is cached; the others are read from git each time.
                let patch = match fingerprint {
                    Some(fingerprint) if view.is_default() => {
                        caches.read_diff(&repo, &fingerprint, &oid, &file)?
                    }
                    _ => Arc::new(details::file_diff(&repo, &oid, &file, view)?),
                };
                Ok::<_, String>(Shown::new(oid, &file, &patch))
            },
            move |this, shown, cx| {
                let current = this
                    .selected_details()
                    .is_some_and(|d| d.oid == details.oid);
                if !current || this.diff.file != Some(index) || this.diff.view != view {
                    return;
                }
                match shown {
                    Ok(shown) => this.diff.patch = Some(Arc::new(shown)),
                    Err(message) => this.notice = Some(Notice::Error(message.into())),
                }
                this.diff.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            },
            cx,
        );
    }

    /// A working file's text isn't read with the list, so it is read from git when opened.
    fn load_working_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(details) = self.shown_details().cloned() else {
            return;
        };
        let (Some(change), Some((side, file))) =
            (details.files.get(index), self.working_file(index))
        else {
            return;
        };
        let (change, file) = (change.clone(), file.clone());
        // Stay where the reader was if this is the same file coming back after a refresh.
        let same_file = self
            .diff
            .patch
            .as_ref()
            .is_some_and(|old| *old.path == *file.path.as_str() && old.side == Some(side));
        let view = self.diff.view;
        let oid = details.oid.clone();
        self.diff.file = Some(index);
        let Some(repo) = self.workdir() else {
            return;
        };
        cx.notify();
        Self::spawn_blocking(
            move || {
                let patch = working::file_diff(&repo, &file, side, view)?;
                // Hunks of a patch that skips white space or shows every line can't be
                // applied on their own, so they get no boxes.
                let movable = view.is_default() && file.has_movable_hunks();
                Ok::<_, String>(Shown::new(oid, &change, &patch).staging(side, movable))
            },
            move |this, shown, cx| {
                if this.diff.file != Some(index) || this.diff.view != view {
                    return;
                }
                match shown {
                    Ok(shown) => this.show_working_patch(shown, same_file, cx),
                    Err(message) => {
                        this.notice = Some(Notice::Error(message.into()));
                        cx.notify();
                    }
                }
            },
            cx,
        );
    }

    fn show_working_patch(&mut self, shown: Shown, same_file: bool, cx: &mut Context<Self>) {
        self.diff.patch = Some(Arc::new(shown));
        if !same_file {
            self.diff.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// Moves one hunk of the file on show into the index, or out of it.
    pub(super) fn move_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        let Some((side, file)) = self.shown_file().and_then(|index| self.working_file(index))
        else {
            return;
        };
        if self.inspector.staging {
            return;
        }
        let file = file.clone();
        self.inspector.staging = true;
        Self::spawn_blocking(
            move || working::move_hunk(&repo, &file, hunk, side),
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

    pub(super) fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.diff.closed = true;
        cx.notify();
    }

    /// The diff, drawn in place of the graph.
    pub(super) fn render_diff(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.shown_file()?;
        let shown = self.diff.patch.clone()?;
        let theme = cx.theme();
        let (border, panel, muted) = (theme.border, theme.title_bar, theme.muted_foreground);
        let background = theme.background;
        let view = self.diff.view;
        let empty = shown.rows.is_empty();
        // A box for the whole file, ticked while it is in the index.
        let file_tick = shown.side.map(|side| {
            let path = shown.path.to_string();
            checkbox("diff-tick-file", side.unstages(), false, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.stage_paths(vec![path.clone()], side.unstages(), cx);
                },
            ))
        });

        Some(
            div()
                .v_flex()
                .flex_1()
                .min_h_0()
                .bg(background)
                .child(
                    div()
                        .h_flex()
                        .h(px(40.))
                        .bg(panel)
                        .flex_none()
                        .pl_4()
                        .pr_2()
                        .gap_3()
                        .text_size(TEXT_SMALL)
                        .border_b_1()
                        .border_color(border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_flex()
                                .gap_2()
                                .children(file_tick)
                                .child(file_path(&shown.path, muted)),
                        )
                        .child(div().h_flex().children([false, true].map(|whole_file| {
                            chip(
                                ("diff-whole-file", usize::from(whole_file)),
                                if whole_file { "Whole file" } else { "Changes" },
                                view.whole_file == whole_file,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let view = DiffView {
                                        whole_file,
                                        ..this.diff.view
                                    };
                                    this.set_diff_view(view, cx);
                                },
                            ))
                        })))
                        .child(
                            chip(
                                "diff-ignore-whitespace",
                                "Hide white space",
                                view.ignore_whitespace,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let view = DiffView {
                                    ignore_whitespace: !this.diff.view.ignore_whitespace,
                                    ..this.diff.view
                                };
                                this.set_diff_view(view, cx);
                            })),
                        )
                        .child(
                            Button::new("close-diff")
                                .ghost()
                                .xsmall()
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| this.close_diff(cx))),
                        ),
                )
                .when(empty, |panel_body| {
                    panel_body.child(
                        div()
                            .p_4()
                            .text_size(TEXT_BODY)
                            .text_color(muted)
                            .child(view.empty_sentence(shown.binary)),
                    )
                })
                .when(!empty, |panel_body| {
                    panel_body.child(self.render_lines(&shown, cx))
                })
                .into_any_element(),
        )
    }

    fn render_lines(&self, shown: &Arc<Shown>, cx: &mut Context<Self>) -> impl IntoElement {
        let list_shown = shown.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "diff-lines",
                    shown.rows.len(),
                    cx.processor(move |_, range: Range<usize>, _, cx| {
                        render_rows(&list_shown, range, cx)
                    }),
                )
                .with_width_from_item(shown.widest)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .track_scroll(&self.diff.scroll)
                .size_full(),
            )
            .scrollbar(&self.diff.scroll, ScrollbarAxis::Both)
    }
}

fn render_rows(shown: &Shown, range: Range<usize>, cx: &mut Context<GraphView>) -> Vec<Div> {
    let theme = cx.theme();
    let (muted, border, panel) = (theme.muted_foreground, theme.border, theme.title_bar);
    let colors = tokens(theme.is_dark());
    // Hunks have a box only where moving one on its own can work.
    let ticks = shown.side.filter(|_| shown.ticks);

    range
        .map(|at| {
            // Each hunk is a card on its own row of rows: a header, then its lines. The
            // margins are the row's padding, so the card has room around it.
            let outer = div()
                .h_flex()
                .h(ROW_HEIGHT)
                .min_w_full()
                .px_3()
                .text_size(TEXT_SMALL)
                .whitespace_nowrap();
            let has_lines = matches!(shown.rows.get(at + 1), Some(Row::Line(_)));
            let line = match &shown.rows[at] {
                Row::Gap => return outer,
                Row::Line(line) => line,
                Row::Hunk { title, index } => {
                    let hunk = *index;
                    let tick = ticks.map(|side| {
                        checkbox(("diff-tick-hunk", hunk), side.unstages(), false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| this.move_hunk(hunk, cx)))
                    });
                    return outer.child(
                        div()
                            .h_flex()
                            .flex_1()
                            .h_full()
                            .gap(px(10.))
                            .px_3()
                            .overflow_hidden()
                            .bg(panel)
                            .border_t_1()
                            .border_l_1()
                            .border_r_1()
                            .border_b_1()
                            .border_color(border)
                            .rounded_t(px(8.))
                            .when(!has_lines, |header| header.rounded_b(px(8.)))
                            .children(tick)
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(colors.subtext0)
                                    .child(expand_tabs(title)),
                            )
                            .when_some(ticks, |header, side| {
                                header.child(
                                    div()
                                        .flex_none()
                                        .text_size(TEXT_CHIP)
                                        .text_color(muted)
                                        .child(side.hunk_note()),
                                )
                            }),
                    );
                }
            };
            let LineStyle {
                wash,
                sign,
                sign_color,
                text,
            } = line_style(line.kind, cx);
            let last = !matches!(shown.rows.get(at + 1), Some(Row::Line(_)));
            outer.child(
                div()
                    .h_flex()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .bg(panel)
                    .border_l_1()
                    .border_r_1()
                    .border_color(border)
                    .when(last, |row| row.border_b_1().rounded_b(px(8.)))
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .h_full()
                            .when_some(wash, |row, wash| row.bg(wash))
                            .child(
                                div()
                                    .w(NUMBER_WIDTH)
                                    .flex_none()
                                    .text_right()
                                    .text_color(colors.overlay1)
                                    .child(
                                        line.new
                                            .or(line.old)
                                            .map(|n| n.to_string())
                                            .unwrap_or_default(),
                                    ),
                            )
                            .child(
                                div()
                                    .w(SIGN_WIDTH)
                                    .flex_none()
                                    .text_center()
                                    .text_color(sign_color)
                                    .child(sign),
                            )
                            .child(div().pr_4().text_color(text).child(expand_tabs(&line.text))),
                    ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
    use super::{FileChange, FilePatch, Row, Shown};
    use gitclient_core::patch;

    #[test]
    fn hunks_flatten_into_rows_and_the_longest_row_is_found() {
        let diff = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1,2 +1,2 @@ fn top() {
 short
-a line that is much longer than the rest
+new
@@ -9 +9 @@
-b
+c
";
        let file = FileChange {
            path: "x".into(),
            ..FileChange::default()
        };
        let shown = Shown::new("oid".into(), &file, &patch::parse(diff)[0]);
        assert_eq!(shown.rows.len(), 9, "a gap before each of the two hunks");
        assert!(matches!(&shown.rows[0], Row::Gap));
        assert!(matches!(&shown.rows[1], Row::Hunk { title, index: 0 } if title == "fn top() {"));
        assert!(matches!(&shown.rows[6], Row::Hunk { index: 1, .. }));
        assert_eq!(shown.widest, Some(3));
    }

    #[test]
    fn a_binary_file_has_nothing_to_list() {
        let file = FileChange {
            path: "img".into(),
            binary: true,
            ..FileChange::default()
        };
        let shown = Shown::new("oid".into(), &file, &FilePatch::default());
        assert!(shown.binary && shown.rows.is_empty());
        assert_eq!(shown.widest, None);
    }
}
