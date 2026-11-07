//! The dock along the bottom of the window. Its first tab is the log of every git command
//! the app runs, with output as it arrives; the others are terminals in the repository's
//! folder, for running git by hand.
//!
//! Closed, the dock is a thin bar that still says which command is running, so a slow
//! command is never a silent wait.

use super::parts::{self, Edge, fit, grip, on_resize};
use super::parts::{TEXT_BODY, TEXT_SMALL};
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
const SIDEBAR_WIDTH: f32 = 280.;
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
    /// What the user dragged the body's height and the log's list width to.
    height: Option<f32>,
    log_width: Option<f32>,
    /// The window's height as last drawn, so a drag can leave history in view.
    window_height: f32,
    /// 0 is the log, then one per terminal.
    active: usize,
    terminals: Vec<TerminalTab>,
    /// Numbers terminal tabs, and doesn't reuse a closed one's number.
    terminals_opened: usize,
    /// Where new terminals start: the repository on show.
    directory: Option<PathBuf>,
    /// The command whose output is on show, or None to follow the newest.
    selected: Option<u64>,
    /// The sidebar's list of commands.
    commands_scroll: UniformListScrollHandle,
    /// The output of the command on show.
    output_scroll: UniformListScrollHandle,
    /// Which command the output list was last drawn for, and how many rows it had.
    output_seen: (Option<u64>, usize),
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
            height: None,
            log_width: None,
            window_height: 0.,
            active: 0,
            terminals: Vec::new(),
            terminals_opened: 0,
            directory: None,
            selected: None,
            commands_scroll: UniformListScrollHandle::new(),
            output_scroll: UniformListScrollHandle::new(),
            output_seen: (None, 0),
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
                        .text_size(TEXT_BODY)
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

    /// The command on show: the one picked, else the newest.
    fn shown(&self) -> Option<u64> {
        log::read(|entries| {
            let picked = self
                .selected
                .filter(|id| entries.iter().any(|entry| entry.id == *id));
            picked.or_else(|| entries.last().map(|entry| entry.id))
        })
    }

    /// Shows a command's output. Picking the newest goes back to following it.
    fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        let newest = log::read(|entries| entries.last().map(|entry| entry.id));
        self.selected = (newest != Some(id)).then_some(id);
        cx.notify();
    }

    /// Moves through the sidebar, newest at the top: negative goes toward the newest.
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids: Vec<u64> = log::read(|entries| entries.iter().rev().map(|e| e.id).collect());
        let Some(current) = self
            .shown()
            .and_then(|shown| ids.iter().position(|id| *id == shown))
        else {
            return;
        };
        let target = current
            .saturating_add_signed(delta)
            .min(ids.len().saturating_sub(1));
        self.commands_scroll
            .scroll_to_item(target, ScrollStrategy::Nearest);
        self.select(ids[target], cx);
    }

    fn render_log(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(shown) = self.shown() else {
            return div()
                .flex_1()
                .p_4()
                .text_size(TEXT_BODY)
                .text_color(muted)
                .child(
                    "Every git command the app runs is listed here, with its output as it arrives.",
                )
                .into_any_element();
        };
        let commands = log::read(|entries| entries.len());
        let rows = log::read(|entries| {
            entries
                .iter()
                .find(|entry| entry.id == shown)
                .map_or(0, output_rows)
        });

        // A command opens at its top. While it runs, the view stays on the newest line
        // unless the reader has scrolled up to look at something.
        let (seen, seen_rows) = self.output_seen;
        if seen != Some(shown) {
            self.output_scroll.scroll_to_item(0, ScrollStrategy::Top);
        } else if rows != seen_rows && rows > 0 {
            let at_end = {
                let scroll = self.output_scroll.0.borrow();
                let base = &scroll.base_handle;
                base.offset().y + base.max_offset().y <= px(ROW_HEIGHT)
            };
            if at_end {
                self.output_scroll
                    .scroll_to_item(rows - 1, ScrollStrategy::Bottom);
            }
        }
        self.output_seen = (Some(shown), rows);

        let border = cx.theme().border;
        let sidebar = div()
            .relative()
            .v_flex()
            .w(px(self.log_width.unwrap_or(SIDEBAR_WIDTH)))
            // A row of columns centres them, and a column with no height shows nothing.
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(border)
            .child(
                uniform_list(
                    "log-commands",
                    commands,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        this.command_rows(range, shown, cx)
                    }),
                )
                .track_scroll(&self.commands_scroll)
                .size_full(),
            )
            .scrollbar(&self.commands_scroll, ScrollbarAxis::Vertical);

        let output = div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(output_header(shown, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "log-output",
                            rows,
                            cx.processor(move |_, range: Range<usize>, _, cx| {
                                output_lines(shown, range, cx)
                            }),
                        )
                        .track_scroll(&self.output_scroll)
                        .size_full(),
                    )
                    .scrollbar(&self.output_scroll, ScrollbarAxis::Vertical),
            );

        let row = div()
            .h_flex()
            .flex_1()
            .min_h_0()
            .child(sidebar)
            .child(grip(Edge::Log, true, cx))
            .child(output);
        on_resize(row, Edge::Log, cx, |this, pointer, bounds| {
            let max = f32::from(bounds.size.width) - 240.;
            this.log_width = Some(fit(pointer.x - bounds.left(), 160., max));
        })
        .into_any_element()
    }

    /// The sidebar's rows, newest first.
    fn command_rows(
        &self,
        range: Range<usize>,
        shown: u64,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let found: Vec<_> = log::read(|entries| {
            range
                .filter_map(|index| {
                    let entry = entries.get(entries.len().checked_sub(index + 1)?)?;
                    let failed = matches!(entry.state, State::Done { success: false, .. });
                    Some((entry.id, entry.title.clone(), entry.brief(), failed))
                })
                .collect()
        });
        found
            .into_iter()
            .map(|(id, title, brief, failed)| {
                parts::panel_row(("log-command", id as usize), id == shown, cx)
                    .h(px(28.))
                    .w_full()
                    .text_size(TEXT_SMALL)
                    .whitespace_nowrap()
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .flex_none()
                            .text_color(if failed { danger } else { muted })
                            .child(brief),
                    )
            })
            .collect()
    }
}

/// The rows of one command's output: its lines, and a note if lines were dropped.
fn output_rows(entry: &Entry) -> usize {
    entry.lines.len() + (entry.dropped > 0) as usize
}

/// The command as it ran and how it stands, above its output.
fn output_header(id: u64, cx: &App) -> Div {
    let theme = cx.theme();
    let (muted, text, border) = (theme.muted_foreground, theme.foreground, theme.border);
    let (success, danger) = (theme.success, theme.danger);
    let (command, summary, color) = log::read(|entries| {
        entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or_else(Default::default, |entry| {
                let color = match entry.state {
                    State::Running => muted,
                    State::Done { success: true, .. } => success,
                    State::Done { .. } => danger,
                };
                (entry.command.clone(), entry.summary(), color)
            })
    });
    div()
        .h_flex()
        .h(px(ROW_HEIGHT + 8.))
        .flex_none()
        .px_3()
        .gap_3()
        .items_center()
        .text_size(TEXT_SMALL)
        .whitespace_nowrap()
        .border_b_1()
        .border_color(border)
        .child(div().flex_none().text_color(muted).child("$"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_semibold()
                .text_color(text)
                .child(command.clone()),
        )
        .child(div().flex_none().text_color(color).child(summary))
        .child(
            Button::new("copy-command")
                .ghost()
                .small()
                .label("Copy")
                .tooltip("Copy the command to the clipboard")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()))
                }),
        )
}

fn output_lines(id: u64, range: Range<usize>, cx: &mut App) -> Vec<Div> {
    let theme = cx.theme();
    let (muted, subtext) = (theme.muted_foreground, tokens(theme.is_dark()).subtext0);
    let danger = theme.danger;
    let row = || {
        div()
            .h_flex()
            .h(px(ROW_HEIGHT))
            .px_3()
            .text_size(TEXT_SMALL)
            .whitespace_nowrap()
    };

    log::read(|entries| {
        let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
            return Vec::new();
        };
        let failed = matches!(entry.state, State::Done { success: false, .. });
        range
            .map(|index| match entry.lines.get(index) {
                Some(line) => {
                    let color = match line.stream {
                        Stream::Out => subtext,
                        Stream::Err if failed => danger,
                        Stream::Err | Stream::Note => muted,
                    };
                    row()
                        .text_color(color)
                        .child(parts::expand_tabs(&line.text))
                }
                None => row()
                    .text_color(muted)
                    .child(format!("… {} more lines not shown", entry.dropped)),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
    use super::Dock;
    use gitclient_core::log;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::process::Command;

    fn command(name: &str) -> log::Handle {
        let mut command = Command::new("git");
        command.args(["-C", "/dock-test", name]);
        log::start(&command)
    }

    fn id_of(title: &str) -> u64 {
        log::read(|entries| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.title == title)
                .expect("the command is logged")
                .id
        })
    }

    #[gpui_kit::test]
    fn the_log_shows_one_command_and_a_pick_in_the_sidebar_changes_which(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|_, cx| Dock::new(cx));
        let dock = window.root(cx).unwrap();
        let cx = VisualTestContext::from_window(window.into(), cx);

        command("dock-first").finish(true, "exit 0");
        command("dock-second").finish(true, "exit 0");
        let (first, second) = (id_of("dock-first"), id_of("dock-second"));

        dock.read_with(&cx, |dock, _| {
            assert_eq!(dock.shown(), Some(second), "follows the newest");
        });
        dock.update(&mut cx.clone(), |dock, cx| dock.select(first, cx));
        dock.read_with(&cx, |dock, _| {
            assert_eq!(dock.shown(), Some(first), "a pick holds, whatever arrives");
        });
        command("dock-third");
        dock.read_with(&cx, |dock, _| assert_eq!(dock.shown(), Some(first)));

        // Moving toward the newest, one step at a time.
        dock.update(&mut cx.clone(), |dock, cx| dock.move_selection(-1, cx));
        dock.read_with(&cx, |dock, _| {
            let shown = dock.shown().unwrap();
            assert!(shown > first, "moved to a newer command");
        });
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (panel, border) = (theme.title_bar, theme.border);
        // The dock never takes more than 40% of the window, so history stays in view.
        self.window_height = f32::from(window.viewport_size().height);
        let height = self.height.map_or_else(
            || (window.viewport_size().height * 0.4).clamp(px(160.), px(420.)),
            px,
        );

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
                // The log has no focus of its own, so it needs one for the dock to hold, and
                // it takes the arrow keys to move between commands.
                .when(terminal.is_none(), |body| {
                    body.track_focus(&self.log_focus).on_key_down(cx.listener(
                        |this, event: &KeyDownEvent, _, cx| {
                            let delta = match event.keystroke.key.as_str() {
                                "up" | "k" => -1,
                                "down" | "j" => 1,
                                _ => return,
                            };
                            this.move_selection(delta, cx);
                            cx.stop_propagation();
                        },
                    ))
                })
                .child(content)
        });

        let open = self.open;
        let dock = div()
            .v_flex()
            .flex_none()
            .w_full()
            .bg(panel)
            .border_t_1()
            .border_color(border)
            .when(open, |dock| dock.child(grip(Edge::Dock, false, cx)))
            .child(self.render_bar(cx))
            .children(body);
        on_resize(dock, Edge::Dock, cx, |this, pointer, bounds| {
            // The body sits under the bar, so the pointer sets where the bar's top is.
            let height = bounds.bottom() - pointer.y - px(BAR_HEIGHT);
            this.height = Some(fit(height, 120., this.window_height - 240.));
        })
    }
}
