//! Selecting text in the dock's monospace views, the terminal and the log, by dragging.
//! Both draw rows of equal-width cells, so a point on screen is a row and a column.

use gpui_kit::component::*;
use gpui_kit::*;
use std::ops::Range;

/// A drag from one cell to another: `(row, column)` pairs, where a column counts the gaps
/// between characters, so the selection covers the characters between the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::view) struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
    dragging: bool,
}

impl Selection {
    pub fn start(cell: (usize, usize)) -> Self {
        Self {
            anchor: cell,
            head: cell,
            dragging: true,
        }
    }

    pub fn extend(&mut self, cell: (usize, usize)) {
        self.head = cell;
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }

    pub fn finish(&mut self) {
        self.dragging = false;
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The characters of `row` that are selected, when it has `len` of them.
    pub fn columns(&self, row: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if row < start.0 || row > end.0 {
            return None;
        }
        let from = if row == start.0 { start.1 } else { 0 };
        let to = if row == end.0 { end.1 } else { len };
        let (from, to) = (from.min(len), to.min(len));
        (from < to).then_some(from..to)
    }

    /// The selected text, given each row's text. Trailing blanks are left off every line.
    pub fn text(&self, row_text: impl Fn(usize) -> Option<String>) -> String {
        let (start, end) = self.ordered();
        let mut lines = Vec::new();
        for row in start.0..=end.0 {
            let Some(text) = row_text(row) else {
                break;
            };
            let len = text.chars().count();
            let line = match self.columns(row, len) {
                Some(columns) => text
                    .chars()
                    .skip(columns.start)
                    .take(columns.len())
                    .collect::<String>(),
                None => String::new(),
            };
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }
}

/// The byte range of the characters `columns` in `text`.
pub(in crate::view) fn bytes(text: &str, columns: Range<usize>) -> Range<usize> {
    let at = |column: usize| {
        text.char_indices()
            .nth(column)
            .map_or(text.len(), |(index, _)| index)
    };
    at(columns.start)..at(columns.end)
}

/// The cell under `point`, where `origin` is the top left of the first row's text.
pub(in crate::view) fn cell(
    point: Point<Pixels>,
    origin: Point<Pixels>,
    cell_width: Pixels,
    row_height: Pixels,
) -> (usize, usize) {
    let row = ((point.y - origin.y) / row_height).floor().max(0.) as usize;
    let column = ((point.x - origin.x) / cell_width).round().max(0.) as usize;
    (row, column)
}

/// The width of one character of the theme's monospace font at `size`.
pub(in crate::view) fn cell_width(window: &Window, cx: &App, size: Pixels) -> Option<Pixels> {
    let font = Font {
        family: cx.theme().mono_font_family.clone(),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    };
    let system = window.text_system();
    system
        .advance(system.resolve_font(&font), size, 'm')
        .ok()
        .map(|advance| advance.width)
}

/// Whether `key` is the key that copies, as the platform's terminals have it: Command C
/// on macOS, Ctrl Shift C elsewhere. `plain_control` adds Ctrl C, for where it can't mean
/// anything else.
pub(in crate::view) fn is_copy(key: &Keystroke, plain_control: bool) -> bool {
    let modifiers = &key.modifiers;
    key.key == "c"
        && if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control && (modifiers.shift || plain_control)
        }
}

/// The colour a selection is drawn in: Blue, tinted.
pub(in crate::view) fn tint(cx: &App) -> Hsla {
    cx.theme().primary.opacity(0.35)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
    use super::{Selection, bytes};

    #[test]
    fn a_selection_covers_the_characters_between_its_ends() {
        let mut selection = Selection::start((1, 2));
        selection.extend((0, 3));
        let rows = ["abcdef", "uvwxyz", "tail"];
        assert_eq!(selection.columns(0, 6), Some(3..6));
        assert_eq!(selection.columns(1, 6), Some(0..2));
        assert_eq!(selection.columns(2, 4), None);
        assert_eq!(
            selection.text(|row| rows.get(row).map(|r| r.to_string())),
            "def\nuv"
        );
    }

    #[test]
    fn a_click_selects_nothing() {
        assert!(Selection::start((2, 4)).is_empty());
    }

    #[test]
    fn bytes_follow_characters_not_bytes() {
        assert_eq!(bytes("é→x", 1..2), 2..5);
    }
}
