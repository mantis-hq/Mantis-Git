//! One terminal in the dock: a shell in the repository's folder, drawn as text on a grid.

use super::selection::{self, Selection};
use crate::theme::{ansi_color, rgb_color};
use crate::view::parts::TEXT_BODY;
use gitclient_core::terminal::{Color, Cursor, Key, Run, Size, Snapshot, Terminal};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

pub(in crate::view) const FONT_SIZE: f32 = 12.;
/// The design's line height for code.
pub(in crate::view) const ROW_HEIGHT: f32 = 22.;
const PADDING_X: f32 = 12.;
const PADDING_Y: f32 = 4.;
/// Rows and columns until the first layout says how many fit.
const INITIAL_SIZE: Size = Size { rows: 12, cols: 80 };
/// Screen changes arrive in bursts, and one redraw covers a burst.
const REDRAW_EVERY: Duration = Duration::from_millis(16);

pub(in crate::view) const KEY_CONTEXT: &str = "Terminal";

pub struct TerminalPane {
    terminal: Result<Terminal, String>,
    focus: FocusHandle,
    cell_width: Pixels,
    /// Where the view was last drawn, to turn the pointer into a cell.
    bounds: Bounds<Pixels>,
    selection: Option<Selection>,
    _redraw: Option<Task<()>>,
    _keys: Subscription,
}

/// Whether `keystroke` belongs to the window even while a terminal has focus: the dock's
/// own keys, which show it and open another terminal. Asking the keymap keeps this in step
/// with the bindings themselves.
fn kept_by_app(keystroke: &Keystroke, context: &[KeyContext], cx: &App) -> bool {
    let (bindings, _) = cx
        .key_bindings()
        .borrow()
        .bindings_for_input(std::slice::from_ref(keystroke), context);
    bindings.iter().any(|binding| {
        let action = binding.action().as_any();
        action.is::<super::ToggleDock>() || action.is::<super::NewTerminal>()
    })
}

impl TerminalPane {
    pub fn new(directory: &Path, cx: &mut Context<Self>) -> Self {
        let terminal = Terminal::spawn(directory, INITIAL_SIZE);
        let redraw = terminal.as_ref().ok().map(|terminal| {
            let changes = terminal.changes();
            cx.spawn(async move |this, cx| {
                while changes.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                    cx.background_executor().timer(REDRAW_EVERY).await;
                }
            })
        });
        let focus = cx.focus_handle();
        // gpui runs key bindings before key listeners, so a listener on the pane would
        // lose Tab and Shift Tab to focus moves and Ctrl C to copying, which the window's
        // root binds. Catching keystrokes before any binding gives the shell all of them.
        let (this, focused) = (cx.weak_entity(), focus.clone());
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if !focused.is_focused(window)
                || kept_by_app(&event.keystroke, &event.context_stack, cx)
            {
                return;
            }
            let taken = this
                .update(cx, |this, cx| this.on_key(&event.keystroke, cx))
                .unwrap_or(false);
            if taken {
                cx.stop_propagation();
            }
        });
        Self {
            terminal,
            focus,
            cell_width: px(8.),
            bounds: Bounds::default(),
            selection: None,
            _redraw: redraw,
            _keys: keys,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref().ok()
    }

    /// Fits the shell to the room the view has, in whole cells.
    fn fit(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let Ok(terminal) = &self.terminal else {
            return;
        };
        self.bounds = bounds;
        let width = bounds.size.width - px(2. * PADDING_X);
        let height = bounds.size.height - px(2. * PADDING_Y);
        let size = Size {
            cols: ((width / self.cell_width).floor().max(1.0)) as u16,
            rows: ((height / px(ROW_HEIGHT)).floor().max(1.0)) as u16,
        };
        if terminal.resize(size) {
            cx.notify();
        }
    }

    /// Sends `key` to the shell, and says whether the shell took it.
    fn on_key(&mut self, key: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Ok(terminal) = &self.terminal else {
            return false;
        };
        let modifiers = &key.modifiers;
        // Ctrl C interrupts the shell, so it copies only where Windows terminals have it
        // copy: while something is selected.
        let selected = self
            .selection
            .is_some_and(|selection| !selection.is_empty());
        if selection::is_copy(key, cfg!(windows) && selected) {
            if selected {
                self.copy(cx);
                return true;
            }
            // Nothing to copy: Ctrl Shift C is not for the shell.
            if modifiers.shift || modifiers.platform {
                return true;
            }
        }
        let paste = if cfg!(target_os = "macos") {
            modifiers.platform && key.key == "v"
        } else {
            (modifiers.control && modifiers.shift && key.key == "v")
                || (modifiers.shift && key.key == "insert")
                // Windows terminals paste on a plain Ctrl V.
                || (cfg!(windows) && modifiers.control && key.key == "v")
        };
        if paste {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                terminal.paste(&text);
            }
        } else if modifiers.platform {
            // Command belongs to the app on macOS, never to the shell.
            return false;
        } else {
            let sent = terminal.send_key(&Key {
                name: &key.key,
                text: key.key_char.as_deref(),
                control: modifiers.control,
                alt: modifiers.alt,
                shift: modifiers.shift,
            });
            if !sent {
                return false;
            }
        }
        self.selection = None;
        cx.notify();
        true
    }

    fn on_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Ok(terminal) = &self.terminal else {
            return;
        };
        let lines = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y / px(ROW_HEIGHT),
            ScrollDelta::Lines(delta) => delta.y * 3.0,
        };
        terminal.scroll(lines.round() as isize);
        // The rows moved under the selection.
        self.selection = None;
        cx.notify();
    }

    /// The cell under `position`.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let origin = self.bounds.origin + point(px(PADDING_X), px(PADDING_Y));
        selection::cell(position, origin, self.cell_width, px(ROW_HEIGHT))
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let (Some(selection), Ok(terminal)) = (self.selection, &self.terminal) else {
            return;
        };
        let snapshot = terminal.snapshot();
        let text = selection.text(|row| snapshot.rows.get(row).map(|runs| row_text(runs)));
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (is_dark, panel, text) = (theme.is_dark(), theme.title_bar, theme.foreground);
        let muted = theme.muted_foreground;
        let font = Font {
            family: theme.mono_font_family.clone(),
            features: FontFeatures::disable_ligatures(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let system = window.text_system();
        if let Ok(advance) = system.advance(system.resolve_font(&font), px(FONT_SIZE), 'm') {
            self.cell_width = advance.width;
        }

        let shell = div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(panel);
        let terminal = match &self.terminal {
            Ok(terminal) => terminal,
            Err(message) => {
                return shell
                    .p_3()
                    .text_size(TEXT_BODY)
                    .text_color(muted)
                    .child(message.clone())
                    .into_any_element();
            }
        };

        let focused = self.focus.is_focused(window);
        let snapshot = terminal.snapshot();
        let exit = terminal.exit_message();
        let rows = snapshot_rows(&snapshot, focused, is_dark, self.selection, theme);
        let this = cx.entity();

        shell
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .text_size(px(FONT_SIZE))
            .text_color(text)
            .font_features(FontFeatures::disable_ligatures())
            .on_scroll(cx.listener(|this, event, _, cx| this.on_scroll(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selection = Some(Selection::start(this.cell_at(event.position)));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let cell = this.cell_at(event.position);
                if event.pressed_button == Some(MouseButton::Left)
                    && let Some(selection) = this.selection.as_mut().filter(|s| s.dragging())
                {
                    selection.extend(cell);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if let Some(selection) = this.selection.as_mut() {
                        selection.finish();
                        if selection.is_empty() {
                            this.selection = None;
                        }
                        cx.notify();
                    }
                }),
            )
            .child(
                // Measures the room, so the shell can be told how many rows and columns fit.
                canvas(
                    move |bounds, _, cx| this.update(cx, |this, cx| this.fit(bounds, cx)),
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .v_flex()
                    .size_full()
                    .px(px(PADDING_X))
                    .py(px(PADDING_Y))
                    .children(rows),
            )
            .when_some(exit, |pane, message| {
                pane.child(
                    div()
                        .absolute()
                        .right_3()
                        .bottom_2()
                        .text_size(TEXT_BODY)
                        .text_color(muted)
                        .child(format!("{message} Close this tab, or open a new one.")),
                )
            })
            .into_any_element()
    }
}

fn snapshot_rows(
    snapshot: &Snapshot,
    focused: bool,
    is_dark: bool,
    selection: Option<Selection>,
    theme: &Theme,
) -> Vec<AnyElement> {
    let cursor = snapshot.cursor.as_ref().filter(|_| focused);
    snapshot
        .rows
        .iter()
        .enumerate()
        .map(|(index, runs)| {
            let text = row_text(runs);
            let mut highlights = highlights(runs, is_dark, theme);
            if let Some(columns) =
                selection.and_then(|selection| selection.columns(index, text.chars().count()))
            {
                let tint = theme.primary.opacity(0.35);
                highlights = overlay(highlights, selection::bytes(&text, columns), |style| {
                    HighlightStyle {
                        background_color: Some(tint),
                        ..style
                    }
                });
            }
            if let Some(cursor) = cursor.filter(|cursor| cursor.row == index) {
                highlights = with_cursor(highlights, cursor, theme);
            }
            div()
                .h(px(ROW_HEIGHT))
                .flex_none()
                .line_height(px(ROW_HEIGHT))
                .whitespace_nowrap()
                .child(StyledText::new(text).with_highlights(highlights))
                .into_any_element()
        })
        .collect()
}

fn row_text(runs: &[Run]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

/// Changes the style of the bytes `over`, whatever styles they had, and leaves the rest.
fn overlay(
    styled: Vec<(Range<usize>, HighlightStyle)>,
    over: Range<usize>,
    patch: impl Fn(HighlightStyle) -> HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut result = Vec::with_capacity(styled.len() + 2);
    let mut at = over.start;
    for (range, style) in styled {
        if range.end <= over.start || range.start >= over.end {
            result.push((range, style));
            continue;
        }
        let (start, end) = (range.start.max(over.start), range.end.min(over.end));
        if range.start < over.start {
            result.push((range.start..over.start, style));
        }
        if start > at {
            result.push((at..start, patch(HighlightStyle::default())));
        }
        result.push((start..end, patch(style)));
        at = end;
        if range.end > over.end {
            result.push((over.end..range.end, style));
        }
    }
    if at < over.end {
        result.push((at..over.end, patch(HighlightStyle::default())));
    }
    result.sort_by_key(|(range, _)| range.start);
    result
}

/// The styles of the runs that differ from plain text.
fn highlights(runs: &[Run], is_dark: bool, theme: &Theme) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut start = 0;
    let mut styled = Vec::new();
    for run in runs {
        let range = start..start + run.text.len();
        start = range.end;
        let (mut foreground, mut background) = (
            colour(run.foreground, is_dark),
            colour(run.background, is_dark),
        );
        if run.inverse {
            // Inverse swaps the two, and "default" on either side means the theme's own.
            let (shown, behind) = (
                foreground.unwrap_or(theme.foreground),
                background.unwrap_or(theme.title_bar),
            );
            (foreground, background) = (Some(behind), Some(shown));
        }
        let plain = foreground.is_none() && background.is_none() && !run.bold && !run.underline;
        if plain {
            continue;
        }
        styled.push((
            range,
            HighlightStyle {
                color: foreground,
                background_color: background,
                font_weight: run.bold.then_some(FontWeight::SEMIBOLD),
                underline: run.underline.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
                ..Default::default()
            },
        ));
    }
    styled
}

fn colour(color: Color, is_dark: bool) -> Option<Hsla> {
    match color {
        Color::Default => None,
        Color::Indexed(index) => Some(ansi_color(index, is_dark)),
        Color::Rgb(r, g, b) => Some(rgb_color(r, g, b)),
    }
}

/// Draws the cursor as Blue with Crust text, cutting whatever style is under it in two.
fn with_cursor(
    styled: Vec<(Range<usize>, HighlightStyle)>,
    cursor: &Cursor,
    theme: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut result = Vec::with_capacity(styled.len() + 3);
    for (range, style) in styled {
        if range.end <= cursor.bytes.start || range.start >= cursor.bytes.end {
            result.push((range, style));
            continue;
        }
        if range.start < cursor.bytes.start {
            result.push((range.start..cursor.bytes.start, style));
        }
        if range.end > cursor.bytes.end {
            result.push((cursor.bytes.end..range.end, style));
        }
    }
    result.push((
        cursor.bytes.clone(),
        HighlightStyle {
            color: Some(theme.primary_foreground),
            background_color: Some(theme.primary),
            ..Default::default()
        },
    ));
    result.sort_by_key(|(range, _)| range.start);
    result
}
