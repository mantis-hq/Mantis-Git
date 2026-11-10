//! The dock along the bottom of the window. Its first tab is the log of every git command
//! the app runs, with output as it arrives; the others are terminals in the repository's
//! folder, for running git by hand.
//!
//! Closed, the dock is a thin bar that still says which command is running, so a slow
//! command is never a silent wait.

mod command_log;
mod selection;
pub(super) mod terminal;

use super::parts::{self, Edge, fit, grip, on_resize};
use super::parts::{TEXT_BODY, TEXT_SMALL};
use crate::theme::tokens;
use gitclient_core::log::{self, Entry, State, Stream};
use gitclient_core::settings::state::DockLayout;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;
use terminal::{ROW_HEIGHT, TerminalPane};

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

/// The terminals of one repository tab, with the one that was in front there.
#[derive(Default)]
struct Group {
    /// The terminal last in front here, as an index into `terminals`. It is kept while the
    /// log is in front, so coming back to the terminals finds this one again.
    terminal: usize,
    terminals: Vec<TerminalTab>,
    /// Numbers terminal tabs, and doesn't reuse a closed one's number.
    opened: usize,
}

pub struct Dock {
    open: bool,
    /// Whether the log is the tab in front. This is the dock's own, and holds across
    /// repository tabs: the log is the same everywhere, while each tab remembers its own
    /// terminal (`Group::terminal`).
    log_in_front: bool,
    /// What the user dragged the body's height and the log's list width to.
    height: Option<f32>,
    log_width: Option<f32>,
    /// The window's height as last drawn, so a drag can leave history in view.
    window_height: f32,
    /// The terminals of the repository tab on show. Each tab has its own, so a shell never
    /// sits in another repository's folder.
    group: Group,
    /// Which repository tab `group` belongs to, and the groups of the tabs behind it.
    scope: Option<EntityId>,
    parked: HashMap<EntityId, Group>,
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
    /// The text picked in the output of one command, which it belongs to.
    log_selection: Option<(u64, selection::Selection)>,
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
            log_in_front: true,
            height: None,
            log_width: None,
            window_height: 0.,
            group: Group::default(),
            scope: None,
            parked: HashMap::new(),
            directory: None,
            selected: None,
            commands_scroll: UniformListScrollHandle::new(),
            output_scroll: UniformListScrollHandle::new(),
            output_seen: (None, 0),
            log_focus: cx.focus_handle(),
            log_selection: None,
            _tasks: [redraw, clock],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn terminal_count(&self) -> usize {
        self.group.terminals.len()
    }

    /// Which tab is showing: 0 is the log.
    pub fn active_tab(&self) -> usize {
        if self.log_in_front || self.group.terminals.is_empty() {
            return 0;
        }
        self.group.terminal.min(self.group.terminals.len() - 1) + 1
    }

    /// Brings tab `index` to the front, where 0 is the log. A terminal that isn't there
    /// falls back to the last one.
    fn select_tab(&mut self, index: usize) {
        self.log_in_front = index == 0;
        if let Some(terminal) = index.checked_sub(1) {
            self.group.terminal = terminal.min(self.group.terminals.len().saturating_sub(1));
        }
    }

    /// New terminals open in this folder.
    pub fn set_directory(&mut self, directory: Option<&Path>) {
        if self.directory.as_deref() != directory {
            self.directory = directory.map(Path::to_path_buf);
        }
    }

    /// Makes `scope`, a repository tab, the one whose terminals show, parking the last
    /// tab's. Says whether it changed.
    pub fn set_scope(&mut self, scope: EntityId) -> bool {
        if self.scope == Some(scope) {
            return false;
        }
        let next = self.parked.remove(&scope).unwrap_or_default();
        let last = std::mem::replace(&mut self.group, next);
        if let Some(old) = self.scope.replace(scope) {
            self.parked.insert(old, last);
        }
        true
    }

    /// Drops the terminals of tabs that are no longer open, which ends their shells.
    pub fn retain_scopes(&mut self, open: &[EntityId]) {
        self.parked.retain(|scope, _| open.contains(scope));
    }

    /// With the dock open, gives a tab that has no terminal a fresh one, in front. The
    /// keyboard stays where it is.
    pub fn ensure_terminal(&mut self, cx: &mut Context<Self>) {
        if self.open && self.group.terminals.is_empty() {
            self.add_terminal(cx);
        }
        cx.notify();
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
        self.select_tab(index);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_terminal(cx);
        self.show(self.group.terminals.len(), window, cx);
    }

    /// Starts a shell in the repository's folder, in a tab at the end.
    fn add_terminal(&mut self, cx: &mut Context<Self>) {
        let directory = self
            .directory
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        self.group.opened += 1;
        self.group.terminals.push(TerminalTab {
            number: self.group.opened,
            pane: cx.new(|cx| TerminalPane::new(&directory, cx)),
        });
    }

    /// Whether the dock is open, with how many terminals and which tab in front.
    pub fn layout(&self) -> DockLayout {
        DockLayout {
            open: self.open,
            terminals: self.group.terminals.len(),
            tab: self.active_tab(),
        }
    }

    /// Brings the dock back as `layout` says, with fresh shells for its terminals. The
    /// keyboard stays with the repository, so the app's shortcuts work from the start.
    pub fn restore(&mut self, layout: DockLayout, cx: &mut Context<Self>) {
        let layout = layout.bounded();
        while self.group.terminals.len() < layout.terminals {
            self.add_terminal(cx);
        }
        self.open = layout.open;
        self.select_tab(layout.tab);
        cx.notify();
    }

    fn close_terminal(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.group.terminals.len() {
            return;
        }
        // The tab beside the closed one comes forward, and the log is always there.
        let (front, closed) = (self.active_tab(), index + 1);
        self.group.terminals.remove(index);
        self.select_tab(if front >= closed { front - 1 } else { front });
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
        let index = self.active_tab().checked_sub(1)?;
        self.group.terminals.get(index).map(|tab| &tab.pane)
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

        let log_tab = parts::tab("dock-log", self.open && self.active_tab() == 0, cx)
            .pr_3()
            .on_click(cx.listener(|this, _, window, cx| this.show(0, window, cx)))
            .child("Log");
        let terminals = (0..self.group.terminals.len()).map(|index| {
            let tab = index + 1;
            parts::tab(
                ("dock-terminal", index),
                self.open && self.active_tab() == tab,
                cx,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.show(tab, window, cx)))
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| this.close_terminal(index, window, cx)),
            )
            .child(format!("Terminal {}", self.group.terminals[index].number))
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
            .when(self.open && self.active_tab() == 0, |bar| {
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
                            if selection::is_copy(&event.keystroke, true) {
                                this.copy_log_selection(cx);
                                cx.stop_propagation();
                                return;
                            }
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
