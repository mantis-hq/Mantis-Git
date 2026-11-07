//! The diff panel under the graph, so history stays visible while reading it.

use super::parts::{LineStyle, expand_tabs, file_path, line_style};
use super::{GraphView, Notice};
use crate::theme::tokens;
use gitclient_core::details::{self, FileChange};
use gitclient_core::patch::{FilePatch, Line};
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
    scroll: UniformListScrollHandle,
}

/// A patch flattened into the rows of a list.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shown {
    oid: Arc<str>,
    path: Arc<str>,
    binary: bool,
    rows: Vec<Row>,
    /// The row with the longest text, which sets how far the list scrolls sideways.
    widest: Option<usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum Row {
    Hunk(String),
    Line(Line),
}

impl Shown {
    fn new(oid: Arc<str>, file: &FileChange, patch: &FilePatch) -> Self {
        let mut rows = Vec::new();
        for hunk in &patch.hunks {
            rows.push(Row::Hunk(hunk.title.clone()));
            rows.extend(hunk.lines.iter().cloned().map(Row::Line));
        }
        let width = |row: &Row| match row {
            Row::Hunk(title) => title.len(),
            Row::Line(line) => line.text.len(),
        };
        let widest = (0..rows.len()).max_by_key(|&index| width(&rows[index]));
        Self {
            oid,
            path: file.path.clone(),
            binary: patch.binary || file.binary,
            rows,
            widest,
        }
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
    /// the new read, so a refresh never closes what is being read.
    pub(super) fn working_loaded(&mut self, cx: &mut Context<Self>) {
        let on_show = self
            .shown_file()
            .and(self.diff.patch.as_ref())
            .map(|p| p.path.clone());
        let same = on_show.and_then(|path| {
            let files = &self.shown_details()?.files;
            files.iter().position(|file| file.path == path)
        });
        match same {
            Some(index) => self.load_file(index, cx),
            None => self.details_loaded(cx),
        }
    }

    pub(super) fn show_file(&mut self, index: usize, cx: &mut Context<Self>) {
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
        Self::spawn_blocking(
            move || {
                let patch = details::file_diff(&repo, &oid, &file)?;
                Ok::<_, String>(Shown::new(oid, &file, &patch))
            },
            move |this, shown, cx| {
                let current = this
                    .selected_details()
                    .is_some_and(|d| d.oid == details.oid);
                if !current || this.diff.file != Some(index) {
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

    /// The working changes carry their patches already, so nothing is read.
    fn load_working_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(details) = self.shown_details().cloned() else {
            return;
        };
        let (Some(change), Some(file)) = (details.files.get(index), self.working_patch(index))
        else {
            return;
        };
        let shown = Shown::new(details.oid.clone(), change, &file.patch);
        // Stay where the reader was if this is the same file coming back after a refresh.
        let same_file = self
            .diff
            .patch
            .as_ref()
            .is_some_and(|old| old.path == shown.path);
        self.diff.file = Some(index);
        self.diff.patch = Some(Arc::new(shown));
        if !same_file {
            self.diff.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.diff.closed = true;
        cx.notify();
    }

    pub(super) fn render_diff(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.shown_file()?;
        let shown = self.diff.patch.clone()?;
        let theme = cx.theme();
        let (border, panel, muted) = (theme.border, theme.title_bar, theme.muted_foreground);
        let empty = shown.rows.is_empty();

        Some(
            div()
                .v_flex()
                .flex_none()
                .h(relative(0.38))
                .min_h(px(180.))
                .bg(panel)
                .border_t_1()
                .border_color(border)
                .child(
                    div()
                        .h_flex()
                        .h(px(34.))
                        .flex_none()
                        .pl_4()
                        .pr_2()
                        .gap_3()
                        .text_size(px(12.))
                        .border_b_1()
                        .border_color(border)
                        .child(file_path(&shown.path, muted))
                        .child(
                            Button::new("close-diff")
                                .ghost()
                                .xsmall()
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| this.close_diff(cx))),
                        ),
                )
                .when(empty, |panel_body| {
                    panel_body.child(div().p_4().text_size(px(13.)).text_color(muted).child(
                        if shown.binary {
                            "This is a binary file, so there is no text to show."
                        } else {
                            "No lines changed in this file."
                        },
                    ))
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

fn render_rows(shown: &Shown, range: Range<usize>, cx: &mut App) -> Vec<Div> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let colors = tokens(theme.is_dark());

    range
        .map(|index| {
            let row = div()
                .h_flex()
                .h(ROW_HEIGHT)
                .min_w_full()
                .text_size(px(12.))
                .whitespace_nowrap();
            let line = match &shown.rows[index] {
                Row::Line(line) => line,
                Row::Hunk(title) => {
                    return row
                        .pl(NUMBER_WIDTH * 2. + SIGN_WIDTH)
                        .pr_4()
                        .text_color(muted)
                        .child(expand_tabs(title));
                }
            };
            let LineStyle {
                wash,
                sign,
                sign_color,
                text,
            } = line_style(line.kind, cx);
            let number = |value: Option<u32>| {
                div()
                    .w(NUMBER_WIDTH)
                    .flex_none()
                    .text_right()
                    .text_color(colors.overlay1)
                    .child(value.map(|n| n.to_string()).unwrap_or_default())
            };
            row.when_some(wash, |row, wash| row.bg(wash))
                .child(number(line.old))
                .child(number(line.new))
                .child(
                    div()
                        .w(SIGN_WIDTH)
                        .flex_none()
                        .text_center()
                        .text_color(sign_color)
                        .child(sign),
                )
                .child(div().pr_4().text_color(text).child(expand_tabs(&line.text)))
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
        assert_eq!(shown.rows.len(), 7);
        assert!(matches!(&shown.rows[0], Row::Hunk(title) if title == "fn top() {"));
        assert!(matches!(&shown.rows[4], Row::Hunk(_)));
        assert_eq!(shown.widest, Some(2));
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
