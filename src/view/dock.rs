//! The dock along the bottom of the window. Its first tab is the log of every git command
//! the app runs, with output as it arrives; the others are terminals in the repository's
//! folder, for running git by hand.
//!
//! Closed, the dock is a thin bar that still says which command is running, so a slow
//! command is never a silent wait.

use super::parts;
use super::terminal::{ROW_HEIGHT, TerminalPane};
use crate::theme::tokens;
use gitclient_core::log::{self, Entry, State, Stream};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

actions!(dock, [ToggleDock, NewTerminal]);

pub(super) fn bind_keys(cx: &mut App) {
    // Neither belongs to a context: they must work from the graph and from a terminal.
    cx.bind_keys([
        KeyBinding::new("ctrl-`", ToggleDock, None),
        KeyBinding::new("ctrl-shift-`", NewTerminal, None),
    ]);
}

const BAR_HEIGHT: f32 = 36.;
/// How often a running command's clock is redrawn.
const CLOCK_EVERY: Duration = Duration::from_secs(1);
/// Log lines arrive in bursts, and one redraw covers a burst.
const REDRAW_EVERY: Duration = Duration::from_millis(40);

struct TerminalTab {
    /// The number in its name, which it keeps as other terminals close.
    number: usize,
    pane: Entity<TerminalPane>,
}

pub struct Dock {
    open: bool,
    /// 0 is the log, then one per terminal.
    active: usize,
    terminals: Vec<TerminalTab>,
    /// Numbers terminal tabs, and doesn't reuse a closed one's number.
    terminals_opened: usize,
    /// Where new terminals start: the repository on show.
    directory: Option<PathBuf>,
    log_scroll: UniformListScrollHandle,
    log_rows: usize,
    log_focus: FocusHandle,
    _tasks: [Task<()>; 2],
}

impl Dock {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let changes = log::subscribe();
        let redraw = cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
                cx.background_executor().timer(REDRAW_EVERY).await;
            }
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOCK_EVERY).await;
                let running = log::read(|entries| entries.iter().any(Entry::is_running));
                if running && this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        Self {
            open: false,
            active: 0,
            terminals: Vec::new(),
            terminals_opened: 0,
            directory: None,
            log_scroll: UniformListScrollHandle::new(),
            log_rows: 0,
            log_focus: cx.focus_handle(),
            _tasks: [redraw, clock],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }

    /// Which tab is showing: 0 is the log.
    pub fn active_tab(&self) -> usize {
        self.active
    }

    /// New terminals open in this folder.
    pub fn set_directory(&mut self, directory: Option<&Path>) {
        if self.directory.as_deref() != directory {
            self.directory = directory.map(Path::to_path_buf);
        }
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Opens the dock on tab `index`.
    pub fn show(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.active = index.min(self.terminals.len());
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self
            .directory
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        self.terminals_opened += 1;
        self.terminals.push(TerminalTab {
            number: self.terminals_opened,
            pane: cx.new(|cx| TerminalPane::new(&directory, cx)),
        });
        self.show(self.terminals.len(), window, cx);
    }

    fn close_terminal(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        self.terminals.remove(index);
        // The tab beside the closed one comes forward, and the log is always there.
        let closed = index + 1;
        if self.active >= closed {
            self.active = self.active.saturating_sub(1);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_terminal() {
            Some(terminal) => {
                let focus = terminal.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
            }
            None => window.focus(&self.log_focus, cx),
        }
    }

    /// The terminal showing, if a terminal tab is.
    pub fn active_terminal(&self) -> Option<&Entity<TerminalPane>> {
        let index = self.active.checked_sub(1)?;
        self.terminals.get(index).map(|tab| &tab.pane)
    }

    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let running = log::read(|entries| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.is_running())
                .map(|entry| (entry.command.clone(), entry.summary()))
        });

        let log_tab = parts::tab("dock-log", self.open && self.active == 0, cx)
            .pr_3()
            .on_click(cx.listener(|this, _, window, cx| this.show(0, window, cx)))
            .child("Log");
        let terminals = (0..self.terminals.len()).map(|index| {
            let tab = index + 1;
            parts::tab(
                ("dock-terminal", index),
                self.open && self.active == tab,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.show(tab, window, cx)))
            .child(format!("Terminal {}", self.terminals[index].number))
            .child(
                Button::new(("close-terminal", index))
                    .ghost()
                    .compact()
                    .label("×")
                    .tooltip("Close this terminal")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_terminal(index, window, cx)
                    })),
            )
        });

        div()
            .h_flex()
            .h(px(BAR_HEIGHT))
            .flex_none()
            .px_2()
            .gap_1()
            .items_center()
            .when(self.open, |bar| bar.border_b_1().border_color(border))
            .child(log_tab)
            .children(terminals)
            .child(
                Button::new("new-terminal")
                    .ghost()
                    .small()
                    .label("+")
                    .tooltip("Open a terminal in this repository's folder (Ctrl Shift `)")
                    .on_click(cx.listener(|this, _, window, cx| this.new_terminal(window, cx))),
            )
            .child(div().flex_1())
            .when_some(running, |bar, (command, summary)| {
                bar.child(Spinner::new().small()).child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted)
                        .child(format!("{command} · {summary}")),
                )
            })
            .when(self.open && self.active == 0, |bar| {
                bar.child(
                    Button::new("clear-log")
                        .ghost()
                        .label("Clear")
                        .tooltip("Forget finished commands")
                        .on_click(|_, _, _| log::clear()),
                )
            })
            .child(
                Button::new("toggle-dock")
                    .ghost()
                    .label(if self.open { "Hide" } else { "Show" })
                    .tooltip(if self.open {
                        "Hide the log and terminals (Ctrl `)"
                    } else {
                        "Show the log and terminals (Ctrl `)"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
            )
    }

    fn render_log(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let total = log::read(|entries| entries.iter().map(rows_in_entry).sum::<usize>());
        if total == 0 {
            return div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(
                    "Every git command the app runs is listed here, with its output as it arrives.",
                )
                .into_any_element();
        }

        // Stay on the newest line unless the reader has scrolled up to look at something.
        if total != self.log_rows {
            let at_end = {
                let scroll = self.log_scroll.0.borrow();
                let base = &scroll.base_handle;
                base.offset().y + base.max_offset().y <= px(ROW_HEIGHT)
            };
            if at_end {
                self.log_scroll
                    .scroll_to_item(total - 1, ScrollStrategy::Bottom);
            }
            self.log_rows = total;
        }

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "log-rows",
                    total,
                    cx.processor(|_, range: Range<usize>, _, cx| log_rows(range, cx)),
                )
                .track_scroll(&self.log_scroll)
                .size_full(),
            )
            .scrollbar(&self.log_scroll, ScrollbarAxis::Vertical)
            .into_any_element()
    }
}

/// A command's rows: its header, its output lines, and a note if lines were dropped.
fn rows_in_entry(entry: &Entry) -> usize {
    1 + entry.lines.len() + (entry.dropped > 0) as usize
}

fn log_rows(range: Range<usize>, cx: &mut App) -> Vec<Div> {
    let theme = cx.theme();
    let (muted, text, subtext) = (
        theme.muted_foreground,
        theme.foreground,
        tokens(theme.is_dark()).subtext0,
    );
    let (success, danger) = (theme.success, theme.danger);
    let row = || {
        div()
            .h_flex()
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_3()
            .text_size(px(13.))
            .whitespace_nowrap()
    };

    log::read(|entries| {
        let mut rows = Vec::with_capacity(range.len());
        let mut first = 0;
        for entry in entries {
            let count = rows_in_entry(entry);
            let (start, end) = (first, first + count);
            first = end;
            if end <= range.start {
                continue;
            }
            if start >= range.end {
                break;
            }
            for index in start.max(range.start)..end.min(range.end) {
                let at = index - start;
                rows.push(if at == 0 {
                    let (summary_color, summary) = match &entry.state {
                        State::Running => (muted, entry.summary()),
                        State::Done { success: true, .. } => (success, entry.summary()),
                        State::Done { .. } => (danger, entry.summary()),
                    };
                    row()
                        // Each command starts a new block, set off from the one above.
                        .mt(px(if index == 0 { 0. } else { 6. }))
                        .child(div().flex_none().text_color(muted).child("$"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_semibold()
                                .text_color(text)
                                .child(entry.command.clone()),
                        )
                        .child(div().flex_none().text_color(summary_color).child(summary))
                } else if let Some(line) = entry.lines.get(at - 1) {
                    let failed = matches!(entry.state, State::Done { success: false, .. });
                    let color = match line.stream {
                        Stream::Out => subtext,
                        Stream::Err if failed => danger,
                        Stream::Err | Stream::Note => muted,
                    };
                    row()
                        .pl_7()
                        .text_color(color)
                        .child(parts::expand_tabs(&line.text))
                } else {
                    row()
                        .pl_7()
                        .text_color(muted)
                        .child(format!("… {} more lines not shown", entry.dropped))
                });
            }
        }
        rows
    })
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (panel, border) = (theme.title_bar, theme.border);
        // The dock never takes more than 40% of the window, so history stays in view.
        let height = (window.viewport_size().height * 0.4).clamp(px(160.), px(420.));

        let body = self.open.then(|| {
            let terminal = self.active_terminal().cloned();
            let content = match &terminal {
                Some(terminal) => terminal.clone().into_any_element(),
                None => self.render_log(cx),
            };
            div()
                .v_flex()
                .h(height)
                .flex_none()
                // The log has no focus of its own, so it needs one for the dock to hold.
                .when(terminal.is_none(), |body| body.track_focus(&self.log_focus))
                .child(content)
        });

        div()
            .v_flex()
            .flex_none()
            .w_full()
            .bg(panel)
            .border_t_1()
            .border_color(border)
            .child(self.render_bar(cx))
            .children(body)
    }
}
