//! Pieces of the interface that more than one region draws, so they can't drift apart.

use crate::patch::LineKind;
use crate::text::split_path;
use crate::theme::{selected_row, tokens};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const TAB: &str = "    ";

/// The 52 px band across the top of a screen.
pub(super) fn header_bar(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .h_flex()
        .min_h(px(52.))
        .flex_none()
        .pl_4()
        .pr_3()
        .gap_4()
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.border)
}

/// The short line between a header's way back and its sentence.
pub(super) fn header_divider(cx: &App) -> Div {
    div().flex_none().w(px(1.)).h(px(20.)).bg(cx.theme().border)
}

/// Commit title type, for the largest text on a panel.
pub(super) fn title() -> Div {
    div()
        .text_size(px(18.))
        .line_height(px(24.))
        .font_semibold()
}

pub(super) fn section_label(muted: Hsla) -> Div {
    div().text_size(px(13.)).font_medium().text_color(muted)
}

/// A row in a side panel that can be picked.
pub(super) fn panel_row(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (active, hover) = (selected_row(theme), theme.list_hover);
    div()
        .id(id)
        .h_flex()
        .px_2()
        .gap_2()
        .rounded(px(6.))
        .cursor_pointer()
        .when(on, |row| row.bg(active))
        .when(!on, |row| row.hover(|style| style.bg(hover)))
}

/// A path that truncates, with its folder quieter than the file's name.
pub(super) fn file_path(path: &str, muted: Hsla) -> Div {
    let (folder, name) = split_path(path);
    div()
        .h_flex()
        .flex_1()
        .min_w_0()
        .truncate()
        .child(div().flex_none().text_color(muted).child(folder.to_owned()))
        .child(div().min_w_0().truncate().child(name.to_owned()))
}

/// The colour of a file's status letter.
pub(super) fn status_color(status: char, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        'A' => theme.success,
        'D' => theme.danger,
        _ => theme.warning,
    }
}

/// How a line of a diff is drawn.
pub(super) struct LineStyle {
    pub wash: Option<Hsla>,
    pub sign: &'static str,
    pub sign_color: Hsla,
    pub text: Hsla,
}

pub(super) fn line_style(kind: LineKind, cx: &App) -> LineStyle {
    let theme = cx.theme();
    let (wash, sign, sign_color, text) = match kind {
        LineKind::Added => (Some(theme.success), "+", theme.success, theme.foreground),
        LineKind::Removed => (Some(theme.danger), "-", theme.danger, theme.foreground),
        LineKind::Context => (
            None,
            "",
            theme.muted_foreground,
            tokens(theme.is_dark()).subtext0,
        ),
    };
    LineStyle {
        wash: wash.map(|color| color.opacity(0.10)),
        sign,
        sign_color,
        text,
    }
}

/// A line of code as drawn: the list measures rows, which a tab would throw off.
pub(super) fn expand_tabs(line: &str) -> String {
    line.replace('\t', TAB)
}
