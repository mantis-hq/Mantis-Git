//! Pieces of the interface that more than one region draws, so they can't drift apart.

use crate::theme::{selected_row, tokens};
use gitclient_core::patch::LineKind;
use gitclient_core::text::split_path;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

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

// The type scale of docs/design.md. Size text with these, not with literal pixels.
/// Commit title.
pub(super) const TEXT_TITLE: Pixels = px(16.);
/// The status sentence.
pub(super) const TEXT_STATUS: Pixels = px(14.);
/// Lists and body text, and the base size of every window.
pub(super) const TEXT_BODY: Pixels = px(13.);
/// Section labels, code and diffs, and secondary text.
pub(super) const TEXT_SMALL: Pixels = px(12.);
/// Chips and counts.
pub(super) const TEXT_CHIP: Pixels = px(11.);

/// The short line between a header's way back and its sentence.
pub(super) fn header_divider(cx: &App) -> Div {
    div().flex_none().w(px(1.)).h(px(20.)).bg(cx.theme().border)
}

/// Commit title type, for the largest text on a panel.
pub(super) fn title() -> Div {
    div()
        .text_size(TEXT_TITLE)
        .line_height(px(22.))
        .font_semibold()
}

pub(super) fn section_label(muted: Hsla) -> Div {
    div().text_size(TEXT_SMALL).font_medium().text_color(muted)
}

/// A row in a side panel that can be picked.
pub(super) fn panel_row(id: impl Into<ElementId>, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    // Not `list_hover`: in Mocha that is Mantle, which is the side panel's own colour.
    let active = selected_row(theme);
    let hover = active.opacity(0.5);
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

/// A 28 px tab in a strip: the active one takes the selected-row fill and Text colour, the
/// rest are muted.
pub(super) fn tab(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let selected = selected_row(theme);
    div()
        .id(id)
        .h_flex()
        .h(px(28.))
        .max_w(px(220.))
        .pl_3()
        .pr_1()
        .gap_1()
        .items_center()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(TEXT_BODY)
        .when(active, |tab| tab.bg(selected).text_color(theme.foreground))
        .when(!active, |tab| {
            tab.text_color(theme.muted_foreground)
                .hover(|tab| tab.bg(selected.opacity(0.5)))
        })
}

/// One option of a choice, with a tick when picked so that colour isn't the only cue.
pub(super) fn chip(id: impl Into<ElementId>, label: &str, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (selected, hover) = (selected_row(theme), theme.list_hover);
    let (text, quiet) = (theme.foreground, theme.secondary_foreground);
    div()
        .id(id)
        .h_flex()
        .h(px(26.))
        .px_2()
        .gap_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(TEXT_SMALL)
        .when(on, |chip| {
            chip.bg(selected).text_color(text).font_semibold()
        })
        .when(!on, |chip| {
            chip.text_color(quiet).hover(|chip| chip.bg(hover))
        })
        .when(on, |chip| chip.child("✓"))
        .child(label.to_owned())
}

/// A path that truncates, with its folder quieter than the file's name.
/// What stands where a list will be while it is read: a spinner and a sentence saying
/// what is coming, over `rows` grey placeholders that breathe slowly. The placeholders are
/// the height of a list row, so the list takes their place without the page jumping.
/// Give each use its own `id`, which keeps its animation apart from the others.
pub(super) fn loading(id: impl Into<ElementId>, label: &str, rows: usize, cx: &App) -> Div {
    // Different widths, so that the placeholders read as paths and not as a bar chart.
    const WIDTHS: [f32; 5] = [0.62, 0.46, 0.7, 0.38, 0.54];
    let theme = cx.theme();
    let (muted, fill) = (theme.muted_foreground, selected_row(theme));
    let bar = |width: DefiniteLength, height: f32| {
        div()
            .h(px(height))
            .w(width)
            .flex_none()
            .rounded(px(4.))
            .bg(fill)
    };
    let placeholders = (0..rows).map(|index| {
        let share = WIDTHS[index % WIDTHS.len()];
        div()
            .h_flex()
            .h(px(28.))
            .gap_2()
            .child(bar(px(12.).into(), 10.))
            .child(div().flex_1().min_w_0().child(bar(relative(share), 10.)))
            .child(bar(px(40.).into(), 10.))
    });
    div()
        .v_flex()
        .gap(px(2.))
        .child(
            div()
                .h_flex()
                .h(px(24.))
                .gap_2()
                .text_size(TEXT_SMALL)
                .text_color(muted)
                .child(Spinner::new().small())
                .child(label.to_owned()),
        )
        .child(
            div().v_flex().children(placeholders).with_animation(
                id,
                Animation::new(Duration::from_millis(1500))
                    .repeat()
                    .with_easing(pulsating_between(0.35, 1.0)),
                |rows, amount| rows.opacity(amount),
            ),
        )
}

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

/// A 16 px box, with a tick or a dash so its state doesn't rest on colour alone.
pub(super) fn checkbox(
    id: impl Into<ElementId>,
    on: bool,
    partly: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (primary, on_primary) = (theme.primary, theme.primary_foreground);
    let idle = tokens(theme.is_dark()).overlay1;
    let filled = on || partly;
    div()
        .id(id)
        .h_flex()
        .justify_center()
        .size(px(16.))
        .flex_none()
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(TEXT_CHIP)
        .font_semibold()
        .text_color(on_primary)
        .when(filled, |tick| tick.bg(primary))
        .when(!filled, |tick| tick.border(px(1.5)).border_color(idle))
        .when(on, |tick| tick.child("✓"))
        .when(partly, |tick| tick.child("–"))
}

/// A panel edge that can be dragged to make the panel smaller or bigger.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    Sidebar,
    Inspector,
    Diff,
    Dock,
    Log,
    Files,
    Form,
    Steps,
    Settings,
}

/// What a drag of an edge carries. It draws nothing, since the edge itself moves.
struct Resize(Edge);

struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A 4 px strip centred on a panel edge that takes a drag, and tints Blue under the
/// pointer. `vertical` is an edge between side-by-side panels. Place it between the
/// two panels, or first in a column for a top edge: it takes no room of its own.
pub(super) fn grip(edge: Edge, vertical: bool, cx: &App) -> AnyElement {
    let hover = cx.theme().primary.opacity(0.5);
    let strip = div()
        .id(("grip", edge as usize))
        .absolute()
        .on_drag(Resize(edge), |_, _, _, cx| cx.new(|_| Ghost));
    let (wrapper, strip) = if vertical {
        (
            div().w(px(0.)),
            strip
                .top_0()
                .left(px(-2.))
                .w(px(4.))
                .h_full()
                .cursor_col_resize(),
        )
    } else {
        (
            div().h(px(0.)).w_full(),
            strip
                .left_0()
                .top(px(-2.))
                .h(px(4.))
                .w_full()
                .cursor_row_resize(),
        )
    };
    wrapper
        .relative()
        .flex_none()
        .child(deferred(strip.hover(move |style| style.bg(hover))).with_priority(5))
        .into_any_element()
}

/// Calls `apply` with the pointer and the bounds of `element` while `edge` is dragged.
/// Put it on an element that spans the whole area the edge can move through.
pub(super) fn on_resize<V: 'static, E: InteractiveElement>(
    element: E,
    edge: Edge,
    cx: &mut Context<V>,
    apply: impl Fn(&mut V, Point<Pixels>, Bounds<Pixels>) + 'static,
) -> E {
    element.on_drag_move(
        cx.listener(move |this, event: &DragMoveEvent<Resize>, _, cx| {
            if event.drag(cx).0 == edge {
                apply(this, event.event.position, event.bounds);
                cx.notify();
            }
        }),
    )
}

/// A dragged size kept between what a panel can show and what it must leave.
pub(super) fn fit(size: Pixels, min: f32, max: f32) -> f32 {
    f32::from(size).clamp(min, max.max(min))
}
