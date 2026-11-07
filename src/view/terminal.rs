//! One terminal in the dock: a shell in the repository's folder, drawn as text on a grid.

use crate::theme::{ansi_color, rgb_color};
use gitclient_core::terminal::{Color, Cursor, Key, Run, Size, Snapshot, Terminal};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

pub(super) const FONT_SIZE: f32 = 12.;
/// The design's line height for code.
pub(super) const ROW_HEIGHT: f32 = 22.;
const PADDING_X: f32 = 12.;
const PADDING_Y: f32 = 4.;
/// Rows and columns until the first layout says how many fit.
const INITIAL_SIZE: Size = Size { rows: 12, cols: 80 };
/// Screen changes arrive in bursts, and one redraw covers a burst.
const REDRAW_EVERY: Duration = Duration::from_millis(16);

pub(super) const KEY_CONTEXT: &str = "Terminal";

pub struct TerminalPane {
    terminal: Result<Terminal, String>,
    focus: FocusHandle,
    cell_width: Pixels,
    _redraw: Option<Task<()>>,
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
        Self {
            terminal,
            focus: cx.focus_handle(),
            cell_width: px(8.),
            _redraw: redraw,
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

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Ok(terminal) = &self.terminal else {
            return;
        };
        let (key, modifiers) = (&event.keystroke, &event.keystroke.modifiers);
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
            return;
        } else {
            terminal.send_key(&Key {
                name: &key.key,
                text: key.key_char.as_deref(),
                control: modifiers.control,
                alt: modifiers.alt,
                shift: modifiers.shift,
            });
        }
        cx.stop_propagation();
        cx.notify();
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
        cx.notify();
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
                    .text_size(px(13.))
                    .text_color(muted)
                    .child(message.clone())
                    .into_any_element();
            }
        };

        let focused = self.focus.is_focused(window);
        let snapshot = terminal.snapshot();
        let exit = terminal.exit_message();
        let rows = snapshot_rows(&snapshot, focused, is_dark, theme);
        let this = cx.entity();

        shell
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .text_size(px(FONT_SIZE))
            .text_color(text)
            .font_features(FontFeatures::disable_ligatures())
            .on_key_down(cx.listener(|this, event, _, cx| this.on_key(event, cx)))
            .on_scroll(cx.listener(|this, event, _, cx| this.on_scroll(event, cx)))
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
                        .text_size(px(13.))
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
    theme: &Theme,
) -> Vec<AnyElement> {
    let cursor = snapshot.cursor.as_ref().filter(|_| focused);
    snapshot
        .rows
        .iter()
        .enumerate()
        .map(|(index, runs)| {
            let text: String = runs.iter().map(|run| run.text.as_str()).collect();
            let mut highlights = highlights(runs, is_dark, theme);
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
