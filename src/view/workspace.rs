//! The window's root: a row of tabs, one repository each, above the active tab's view.
//!
//! Each tab is a whole [`GraphView`], so selection, scroll, screens and half-typed
//! commits stay as they were when you switch away. The strip only appears once there
//! are two tabs, since one tab is a control that does nothing.

use super::dock::{Dock, NewTerminal, ToggleDock};
use super::parts::TEXT_BODY;
use super::{GraphView, parts, repository_name};
use gpui_kit::component::button::*;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};

const WORKSPACE_CONTEXT: &str = "Workspace";

actions!(
    workspace,
    [NewTab, CloseTab, NextTab, PreviousTab, OpenRepositoryInTab]
);

pub(super) fn bind_workspace_keys(cx: &mut App) {
    let (new, close, open) = if cfg!(target_os = "macos") {
        ("cmd-t", "cmd-w", "cmd-o")
    } else {
        ("ctrl-t", "ctrl-w", "ctrl-o")
    };
    let context = Some(WORKSPACE_CONTEXT);
    cx.bind_keys([
        KeyBinding::new(new, NewTab, context),
        KeyBinding::new(close, CloseTab, context),
        KeyBinding::new(open, OpenRepositoryInTab, context),
        KeyBinding::new("ctrl-tab", NextTab, context),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, context),
        KeyBinding::new("ctrl-pagedown", NextTab, context),
        KeyBinding::new("ctrl-pageup", PreviousTab, context),
    ]);
}

pub struct Workspace {
    tabs: Vec<Entity<GraphView>>,
    active: usize,
    dock: Entity<Dock>,
}

impl Workspace {
    /// A workspace with one empty tab.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = cx.new(|cx| GraphView::new(window, cx));
        Self {
            tabs: vec![first],
            active: 0,
            dock: cx.new(Dock::new),
        }
    }

    pub fn dock(&self) -> &Entity<Dock> {
        &self.dock
    }

    /// Shows or hides the dock. Hiding hands the keyboard back to the repository.
    fn toggle_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| dock.toggle(window, cx));
        if !self.dock.read(cx).is_open() {
            let focus = self.tabs[self.active].read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        }
    }

    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_dock_directory(cx);
        self.dock
            .update(cx, |dock, cx| dock.new_terminal(window, cx));
    }

    /// New terminals start in the folder of the repository on show.
    fn sync_dock_directory(&self, cx: &mut Context<Self>) {
        let directory = self.tabs[self.active].read(cx).workdir();
        self.dock
            .update(cx, |dock, _| dock.set_directory(directory.as_deref()));
    }

    /// Opens `repo` in a tab: the one already showing it, else the active tab if that is
    /// empty, else a new one.
    pub fn open(&mut self, repo: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tab_showing(&repo, cx) {
            self.activate(index, window, cx);
            return;
        }
        if !self.active_view().read(cx).has_repository() {
            self.active_view()
                .update(cx, |view, cx| view.open(repo, cx));
            cx.notify();
            return;
        }
        self.new_tab(window, cx);
        self.active_view()
            .update(cx, |view, cx| view.open(repo, cx));
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active_view(&self) -> &Entity<GraphView> {
        &self.tabs[self.active]
    }

    pub fn tab(&self, index: usize) -> Option<&Entity<GraphView>> {
        self.tabs.get(index)
    }

    fn tab_showing(&self, repo: &Path, cx: &App) -> Option<usize> {
        let wanted = canonical(repo);
        self.tabs.iter().position(|tab| {
            tab.read(cx)
                .repository()
                .is_some_and(|open| canonical(open) == wanted)
        })
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| GraphView::new(window, cx));
        self.tabs.insert(self.active + 1, view);
        self.activate(self.active + 1, window, cx);
    }

    pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            // The last tab empties instead of closing, so the window never has nothing.
            self.tabs[0] = cx.new(|cx| GraphView::new(window, cx));
            self.activate(0, window, cx);
            return;
        }
        self.tabs.remove(index);
        let next = if index < self.active {
            self.active - 1
        } else {
            self.active.min(self.tabs.len() - 1)
        };
        self.active = usize::MAX;
        self.activate(next, window, cx);
    }

    pub fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let changed = index != self.active;
        self.active = index;
        for (position, tab) in self.tabs.iter().enumerate() {
            tab.update(cx, |view, _| view.set_in_background(position != index));
        }
        if changed {
            // A tab left alone may have missed commits made outside the app.
            self.tabs[index].update(cx, |view, cx| view.catch_up(cx));
        }
        let focus = self.tabs[index].read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len() as isize;
        let next = (self.active as isize + delta).rem_euclid(count) as usize;
        self.activate(next, window, cx);
    }

    fn prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                for path in paths {
                    this.open(path, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn tab_name(&self, index: usize, cx: &App) -> String {
        match self.tabs[index].read(cx).repository() {
            Some(path) => repository_name(path),
            None => "New tab".to_owned(),
        }
    }

    fn render_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (strip, border) = (theme.title_bar, theme.border);
        let tabs = (0..self.tabs.len()).map(|index| self.render_tab(index, cx));
        div()
            .h_flex()
            .h(px(36.))
            .flex_none()
            .px_2()
            .gap_1()
            .items_center()
            .bg(strip)
            .border_b_1()
            .border_color(border)
            .children(tabs)
            .child(
                Button::new("new-tab")
                    .ghost()
                    .small()
                    .label("+")
                    .tooltip("Open another repository in a new tab")
                    .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
            )
            .into_any_element()
    }

    fn render_tab(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let path = self.tabs[index]
            .read(cx)
            .repository()
            .map(|path| path.display().to_string());
        let name = self.tab_name(index, cx);

        parts::tab(("tab", index), index == self.active, cx)
            .on_click(cx.listener(move |this, _, window, cx| this.activate(index, window, cx)))
            .when_some(path, |tab, path| {
                tab.tooltip(move |window, cx| Tooltip::new(path.clone()).build(window, cx))
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .child(
                Button::new(("close-tab", index))
                    .ghost()
                    .compact()
                    .label("×")
                    .tooltip("Close tab")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_tab(index, window, cx)
                    })),
            )
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.theme().background;
        let strip = (self.tabs.len() > 1).then(|| self.render_strip(cx));
        self.sync_dock_directory(cx);
        div()
            .v_flex()
            .size_full()
            .bg(background)
            .text_size(TEXT_BODY)
            .on_action(cx.listener(|this, _: &ToggleDock, window, cx| this.toggle_dock(window, cx)))
            .on_action(
                cx.listener(|this, _: &NewTerminal, window, cx| this.new_terminal(window, cx)),
            )
            .child(
                // The tab shortcuts live here and not on the window, so that a terminal
                // below keeps Ctrl W and Ctrl T for the shell.
                div()
                    .v_flex()
                    .flex_1()
                    .min_h_0()
                    .key_context(WORKSPACE_CONTEXT)
                    .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
                    .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                        this.close_tab(this.active, window, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &NextTab, window, cx| this.step(1, window, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &PreviousTab, window, cx| this.step(-1, window, cx)),
                    )
                    .on_action(cx.listener(|this, _: &OpenRepositoryInTab, window, cx| {
                        this.prompt(window, cx)
                    }))
                    .children(strip)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.tabs[self.active].clone()),
                    ),
            )
            .child(self.dock.clone())
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::Workspace;
    use crate::view::bind_keys;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    fn open_workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
        });
        let window = cx.add_window(Workspace::new);
        let view = window.root(cx).unwrap();
        let cx = VisualTestContext::from_window(window.into(), cx);
        (view, cx)
    }

    fn state(view: &Entity<Workspace>, cx: &VisualTestContext) -> (usize, usize) {
        view.read_with(cx, |view, _| (view.tab_count(), view.active_index()))
    }

    #[gpui_kit::test]
    fn a_new_tab_opens_next_to_the_active_one_and_takes_focus(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        assert_eq!(state(&view, &cx), (1, 0));
        cx.simulate_keystrokes("ctrl-t");
        assert_eq!(state(&view, &cx), (2, 1));
        cx.simulate_keystrokes("ctrl-t");
        assert_eq!(state(&view, &cx), (3, 2));
    }

    fn dock_state(view: &Entity<Workspace>, cx: &VisualTestContext) -> (bool, usize, usize) {
        view.read_with(cx, |view, cx| {
            let dock = view.dock().read(cx);
            (dock.is_open(), dock.terminal_count(), dock.active_tab())
        })
    }

    #[gpui_kit::test]
    fn the_dock_toggles_with_its_key_and_opens_on_the_log(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        assert_eq!(dock_state(&view, &cx), (false, 0, 0), "starts closed");
        cx.simulate_keystrokes("ctrl-`");
        assert_eq!(dock_state(&view, &cx), (true, 0, 0));
        cx.simulate_keystrokes("ctrl-`");
        assert_eq!(dock_state(&view, &cx), (false, 0, 0));
    }

    #[cfg(unix)]
    #[gpui_kit::test]
    fn a_terminal_keeps_the_keys_the_shell_needs_and_closes_from_its_tab(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        cx.simulate_keystrokes("ctrl-t");
        cx.simulate_keystrokes("ctrl-shift-`");
        assert_eq!(
            dock_state(&view, &cx),
            (true, 1, 1),
            "opens on the new terminal"
        );

        // Ctrl W deletes a word in a shell. It must not close the repository tab.
        cx.simulate_keystrokes("ctrl-w");
        assert_eq!(state(&view, &cx), (2, 1));

        // Hiding the dock gives the repository its shortcuts back.
        cx.simulate_keystrokes("ctrl-`");
        cx.simulate_keystrokes("ctrl-w");
        assert_eq!(state(&view, &cx), (1, 0));

        view.update_in(&mut cx, |view, window, cx| {
            view.dock().update(cx, |dock, cx| dock.show(0, window, cx));
        });
        assert_eq!(
            dock_state(&view, &cx),
            (true, 1, 0),
            "the log is always first"
        );
    }

    #[gpui_kit::test]
    fn tabs_cycle_in_both_directions(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        cx.simulate_keystrokes("ctrl-t ctrl-t");
        cx.simulate_keystrokes("ctrl-tab");
        assert_eq!(state(&view, &cx).1, 0, "wraps to the first");
        cx.simulate_keystrokes("ctrl-shift-tab");
        assert_eq!(state(&view, &cx).1, 2, "wraps to the last");
    }

    #[gpui_kit::test]
    fn closing_a_tab_keeps_the_same_tab_in_front_and_the_last_one_empties(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        cx.simulate_keystrokes("ctrl-t ctrl-t");
        let third = view.read_with(&cx, |view, _| view.tab(2).unwrap().clone());

        view.update_in(&mut cx, |view, window, cx| view.close_tab(0, window, cx));
        assert_eq!(state(&view, &cx), (2, 1));
        view.read_with(&cx, |view, _| {
            assert_eq!(view.active_view(), &third);
        });

        cx.simulate_keystrokes("ctrl-w");
        assert_eq!(state(&view, &cx), (1, 0));
        cx.simulate_keystrokes("ctrl-w");
        assert_eq!(state(&view, &cx), (1, 0), "the window keeps one tab");
    }

    #[gpui_kit::test]
    fn opening_a_repository_fills_an_empty_tab_then_adds_tabs_and_never_duplicates(
        cx: &mut TestAppContext,
    ) {
        let (view, mut cx) = open_workspace(cx);
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let open = |path: &std::path::Path, cx: &mut VisualTestContext| {
            let path = path.to_owned();
            view.update_in(cx, |view, window, cx| view.open(path, window, cx));
        };

        open(one.path(), &mut cx);
        assert_eq!(state(&view, &cx), (1, 0), "the empty tab is reused");
        open(two.path(), &mut cx);
        assert_eq!(state(&view, &cx), (2, 1));
        open(one.path(), &mut cx);
        assert_eq!(
            state(&view, &cx),
            (2, 0),
            "switches to the tab already showing it"
        );
    }
}
