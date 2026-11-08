//! The window's root: a row of tabs, one repository each, above the active tab's view.
//!
//! Each tab is a whole [`GraphView`], so selection, scroll, screens and half-typed
//! commits stay as they were when you switch away. The strip only appears once there
//! are two tabs, since one tab is a control that does nothing.

use super::dock::{Dock, NewTerminal, ToggleDock};
use super::parts::TEXT_BODY;
use super::{GraphView, OpenWorktreeInTab, SwitchWorktree, parts, repository_name};
use gitclient_core::settings::state::AppState;
use gpui_kit::component::button::*;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};

const WORKSPACE_CONTEXT: &str = "Workspace";

actions!(
    workspace,
    [
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        MoveTabLeft,
        MoveTabRight,
        OpenRepositoryInTab
    ]
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
        KeyBinding::new("ctrl-shift-pageup", MoveTabLeft, context),
        KeyBinding::new("ctrl-shift-pagedown", MoveTabRight, context),
    ]);
}

/// What the cursor carries while a tab is dragged along the strip.
#[derive(Clone)]
struct DraggedTab {
    index: usize,
    name: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drawn as the active tab, on the strip's fill so it stays readable over the graph.
        div().rounded(px(6.)).bg(cx.theme().title_bar).child(
            parts::tab("dragged-tab", true, cx)
                .pr_3()
                .child(self.name.clone()),
        )
    }
}

pub struct Workspace {
    tabs: Vec<Entity<GraphView>>,
    active: usize,
    dock: Entity<Dock>,
    /// Where the open repositories are remembered. `None` remembers nothing, as in tests.
    state_file: Option<PathBuf>,
    /// The tabs and front tab last written, so that only a change writes.
    saved: Option<(Vec<PathBuf>, usize)>,
}

impl Workspace {
    /// A workspace with one empty tab.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            dock: cx.new(Dock::new),
            state_file: None,
            saved: None,
        };
        let first = this.make_tab(window, cx);
        this.tabs.push(first);
        this.track(0, cx);
        this
    }

    /// Starts remembering the open repositories in `file`. Call it once the tabs from
    /// the last run are back, so that opening them one by one doesn't overwrite the rest.
    pub fn remember_in(&mut self, file: Option<PathBuf>, cx: &mut Context<Self>) {
        self.state_file = file;
        for tab in &self.tabs {
            let file = self.state_file.clone();
            tab.update(cx, |tab, _| tab.set_state_file(file));
        }
        self.persist(cx);
    }

    /// Saves when the tab at `index` changes, since a tab can open a repository itself.
    fn track(&self, index: usize, cx: &mut Context<Self>) {
        cx.observe(&self.tabs[index], |this, _, cx| this.persist(cx))
            .detach();
    }

    /// Writes the open repositories and the front tab, unless that is what is saved.
    /// Empty tabs aren't kept, and the file's other settings are left as they are.
    fn persist(&mut self, cx: &App) {
        let Some(file) = &self.state_file else {
            return;
        };
        if self.active >= self.tabs.len() {
            return;
        }
        let mut open = Vec::new();
        let mut front = 0;
        for (index, tab) in self.tabs.iter().enumerate() {
            if index == self.active {
                front = open.len();
            }
            open.extend(tab.read(cx).repository().map(Path::to_owned));
        }
        let now = (open, front);
        if self.saved.as_ref() == Some(&now) {
            return;
        }
        let mut state = AppState::load(file);
        (state.tabs, state.active) = now.clone();
        match state.save(file) {
            Ok(()) => self.saved = Some(now),
            Err(error) => eprintln!("{error}"),
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

    /// A tab with no repository, which listens for asks to switch worktree. The tab
    /// moves to the other working tree in place, keeping its position in the strip.
    fn make_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<GraphView> {
        let view = cx.new(|cx| GraphView::new(window, cx));
        view.update(cx, |view, _| view.set_state_file(self.state_file.clone()));
        cx.subscribe_in(
            &view,
            window,
            |this, _, event: &OpenWorktreeInTab, window, cx| {
                // Opening dedupes, so a worktree that has a tab is brought forward.
                this.open(event.0.clone(), window, cx)
            },
        )
        .detach();
        cx.subscribe(&view, |_, view, event: &SwitchWorktree, cx| {
            let path = event.0.clone();
            view.update(cx, |view, cx| view.open(path, cx));
            cx.notify();
        })
        .detach();
        view
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
        let view = self.make_tab(window, cx);
        self.tabs.insert(self.active + 1, view);
        self.track(self.active + 1, cx);
        self.activate(self.active + 1, window, cx);
    }

    pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            // The last tab empties instead of closing, so the window never has nothing.
            self.tabs[0] = self.make_tab(window, cx);
            self.track(0, cx);
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
        self.persist(cx);
        cx.notify();
    }

    /// Moves the tab at `from` to `to`, so it sits where that tab was. The same tab stays
    /// in front, and the new order is saved.
    pub fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let front = self.tabs[self.active].clone();
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self.tabs.iter().position(|tab| *tab == front).unwrap_or(0);
        self.persist(cx);
        cx.notify();
    }

    /// Moves the front tab one place along the strip, stopping at either end.
    fn shift_active(&mut self, delta: isize, cx: &mut Context<Self>) {
        let to = self.active as isize + delta;
        if (0..self.tabs.len() as isize).contains(&to) {
            self.move_tab(self.active, to as usize, cx);
        }
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
        let dragged = DraggedTab {
            index,
            name: name.clone().into(),
        };

        parts::tab(("tab", index), index == self.active, cx)
            .on_click(cx.listener(move |this, _, window, cx| this.activate(index, window, cx)))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .can_drop(move |dragged, _, _| {
                dragged
                    .downcast_ref::<DraggedTab>()
                    .is_some_and(|source| source.index != index)
            })
            .drag_over::<DraggedTab>(|style, _, _, cx| style.bg(cx.theme().primary.opacity(0.14)))
            .on_drop(cx.listener(move |this, source: &DraggedTab, _, cx| {
                this.move_tab(source.index, index, cx)
            }))
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
                    .on_action(
                        cx.listener(|this, _: &MoveTabLeft, _, cx| this.shift_active(-1, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &MoveTabRight, _, cx| this.shift_active(1, cx)),
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
    use gitclient_core::settings::state::AppState;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

    fn open_workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, VisualTestContext) {
        // The dock listens to the process-wide command log, which any test's git thread
        // can wake, and a terminal wakes its pane from its own thread. The scheduler
        // would call both non-deterministic.
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
        });
        // Inside gpui-kit's root, as `gpui_kit::open_window` puts it, whose keys (Tab,
        // Ctrl C) sit above everything the workspace draws.
        let mut view = None;
        let window = cx.add_window(|window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            view = Some(workspace.clone());
            gpui_kit::base::Root::new(workspace, window, cx)
        });
        let cx = VisualTestContext::from_window(window.into(), cx);
        (view.unwrap(), cx)
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

        // Tab completes and Ctrl C interrupts. No key the shell uses may become an app
        // action, such as moving focus or copying, and the terminal keeps focus.
        let actions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let _observer = cx.update(|_, cx| {
            let actions = actions.clone();
            cx.observe_keystrokes(move |event, _, _| {
                if let Some(action) = &event.action {
                    actions.borrow_mut().push(action.name());
                }
            })
        });
        let focused = |cx: &mut VisualTestContext| cx.update(|window, cx| window.focused(cx));
        let before = focused(&mut cx);
        cx.simulate_keystrokes(
            "tab shift-tab ctrl-c ctrl-r ctrl-b up down home end enter escape j",
        );
        assert_eq!(*actions.borrow(), Vec::<&str>::new());
        assert_eq!(focused(&mut cx), before);
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

    #[gpui_kit::test]
    fn open_repositories_and_the_front_tab_are_remembered_and_empty_tabs_are_not(
        cx: &mut TestAppContext,
    ) {
        let (view, mut cx) = open_workspace(cx);
        let state_dir = tempfile::tempdir().unwrap();
        let file = state_dir.path().join("config").join("state");
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());

        // Nothing is written until remembering starts, as while the last run's tabs return.
        view.update_in(&mut cx, |view, window, cx| {
            view.open(one.path().to_owned(), window, cx)
        });
        assert!(!file.exists());

        view.update(&mut cx, |view, cx| view.remember_in(Some(file.clone()), cx));
        assert_eq!(AppState::load(&file).tabs, vec![one.path().to_owned()]);

        view.update_in(&mut cx, |view, window, cx| {
            view.open(two.path().to_owned(), window, cx)
        });
        cx.simulate_keystrokes("ctrl-t");
        let saved = AppState::load(&file);
        assert_eq!(
            saved.tabs,
            vec![one.path().to_owned(), two.path().to_owned()]
        );
        assert_eq!(saved.active, 2, "an empty tab in front points past the end");

        view.update_in(&mut cx, |view, window, cx| view.activate(0, window, cx));
        assert_eq!(AppState::load(&file).active, 0);

        view.update_in(&mut cx, |view, window, cx| view.close_tab(0, window, cx));
        assert_eq!(AppState::load(&file).tabs, vec![two.path().to_owned()]);
    }

    #[gpui_kit::test]
    fn moving_a_tab_keeps_the_same_one_in_front_and_saves_the_new_order(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        let state_dir = tempfile::tempdir().unwrap();
        let file = state_dir.path().join("state");
        let repos: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let paths: Vec<_> = repos.iter().map(|repo| repo.path().to_owned()).collect();
        for path in &paths {
            let path = path.clone();
            view.update_in(&mut cx, |view, window, cx| view.open(path, window, cx));
        }
        view.update(&mut cx, |view, cx| view.remember_in(Some(file.clone()), cx));
        let front = view.read_with(&cx, |view, _| view.active_view().clone());
        assert_eq!(state(&view, &cx), (3, 2));

        // Dropping the last tab on the first puts it first, and it stays in front.
        view.update(&mut cx, |view, cx| view.move_tab(2, 0, cx));
        assert_eq!(state(&view, &cx), (3, 0));
        view.read_with(&cx, |view, _| assert_eq!(view.active_view(), &front));
        let saved = AppState::load(&file);
        assert_eq!(
            saved.tabs,
            vec![paths[2].clone(), paths[0].clone(), paths[1].clone()]
        );
        assert_eq!(saved.active, 0);

        // The keys move the front tab one place, and stop at the ends.
        cx.simulate_keystrokes("ctrl-shift-pageup");
        assert_eq!(state(&view, &cx), (3, 0), "already first");
        cx.simulate_keystrokes("ctrl-shift-pagedown ctrl-shift-pagedown ctrl-shift-pagedown");
        assert_eq!(state(&view, &cx), (3, 2));
        view.read_with(&cx, |view, _| assert_eq!(view.active_view(), &front));
        let saved = AppState::load(&file);
        assert_eq!(
            saved.tabs,
            vec![paths[0].clone(), paths[1].clone(), paths[2].clone()]
        );
        assert_eq!(saved.active, 2);

        // Moving a tab behind the front one leaves the front tab where it is.
        view.update(&mut cx, |view, cx| view.move_tab(0, 1, cx));
        assert_eq!(state(&view, &cx), (3, 2));
        assert_eq!(
            AppState::load(&file).tabs,
            vec![paths[1].clone(), paths[0].clone(), paths[2].clone()]
        );
    }

    #[gpui_kit::test]
    fn remembering_keeps_the_settings_already_in_the_file(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        let state_dir = tempfile::tempdir().unwrap();
        let file = state_dir.path().join("state");
        AppState {
            reopen_tabs: false,
            ..Default::default()
        }
        .save(&file)
        .unwrap();
        let repo = tempfile::tempdir().unwrap();
        view.update_in(&mut cx, |view, window, cx| {
            view.open(repo.path().to_owned(), window, cx)
        });
        view.update(&mut cx, |view, cx| view.remember_in(Some(file.clone()), cx));
        let saved = AppState::load(&file);
        assert!(!saved.reopen_tabs);
        assert_eq!(saved.tabs, vec![repo.path().to_owned()]);
    }

    #[gpui_kit::test]
    fn switching_worktree_changes_the_current_tab_and_opens_no_new_one(cx: &mut TestAppContext) {
        use crate::view::SwitchWorktree;
        let (view, mut cx) = open_workspace(cx);
        let (main, linked) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = main.path().to_owned();
        view.update_in(&mut cx, |view, window, cx| view.open(path, window, cx));
        let tab = view.read_with(&cx, |view, _| view.active_view().clone());

        let to = linked.path().to_owned();
        tab.update(&mut cx, |_, cx| cx.emit(SwitchWorktree(to.clone())));
        assert_eq!(state(&view, &cx), (1, 0), "no new tab");
        let showing = tab.read_with(&cx, |view, _| view.repository().map(|p| p.to_owned()));
        assert_eq!(showing, Some(to));
    }

    /// A tab that shows `main` and knows `linked` is another worktree of it, without
    /// starting a load, which would run on a thread the test scheduler can't follow.
    fn tab_with_worktrees(
        view: &Entity<Workspace>,
        main: &std::path::Path,
        linked: &std::path::Path,
        cx: &mut VisualTestContext,
    ) -> Entity<crate::view::GraphView> {
        let tab = view.read_with(cx, |view, _| view.active_view().clone());
        let listing = format!(
            "worktree {}\nHEAD aaaaaaa\nbranch refs/heads/main\n\nworktree {}\nHEAD aaaaaaa\ndetached\n",
            main.display(),
            linked.display()
        );
        let mut refs = gitclient_core::history::refs::Refs::default();
        refs.worktrees = gitclient_core::repo::worktree::parse(&listing);
        refs.worktrees[0].is_current = true;
        let main = main.to_owned();
        tab.update(cx, |tab, _| {
            tab.repo = Some(main);
            tab.refs = std::sync::Arc::new(refs);
        });
        tab
    }

    /// Right-clicks `linked` and chooses the first line.
    fn choose_first_line(
        tab: &Entity<crate::view::GraphView>,
        linked: &std::path::Path,
        cx: &mut VisualTestContext,
    ) {
        let linked = linked.to_owned();
        tab.update(cx, |tab, cx| {
            tab.open_worktree_menu(
                &linked,
                gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.)),
                cx,
            );
            assert!(tab.step_menu(1, cx));
            tab.activate_menu(cx);
        });
    }

    #[gpui_kit::test]
    fn the_worktree_menu_switches_in_place_or_opens_a_tab(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        let (main, linked) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let tab = tab_with_worktrees(&view, main.path(), linked.path(), &mut cx);

        // The first line is "Switch to", which keeps to the one tab.
        choose_first_line(&tab, linked.path(), &mut cx);
        assert_eq!(state(&view, &cx), (1, 0));
        let showing = tab.read_with(&cx, |tab, _| tab.repository().map(|p| p.to_owned()));
        assert_eq!(showing.as_deref(), Some(linked.path()));

        // Back on the first one, the second line opens a tab of its own.
        let tab = tab_with_worktrees(&view, main.path(), linked.path(), &mut cx);
        let path = linked.path().to_owned();
        tab.update(&mut cx, |tab, cx| {
            tab.open_worktree_menu(
                &path,
                gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.)),
                cx,
            );
            tab.step_menu(1, cx);
            tab.step_menu(1, cx);
            tab.activate_menu(cx);
        });
        assert_eq!(state(&view, &cx), (2, 1), "a new tab shows the worktree");
    }

    #[gpui_kit::test]
    fn the_submodule_menu_opens_a_downloaded_one_in_a_tab(cx: &mut TestAppContext) {
        let (view, mut cx) = open_workspace(cx);
        let (main, inner) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let tab = view.read_with(&cx, |view, _| view.active_view().clone());
        let module = gitclient_core::repo::submodule::Submodule {
            name: "lib".into(),
            path: "lib".into(),
            folder: inner.path().to_owned(),
            url: None,
            recorded: "a".repeat(40),
            head: Some("a".repeat(40)),
            state: gitclient_core::repo::submodule::State::Current,
        };
        let mut refs = gitclient_core::history::refs::Refs::default();
        refs.submodules = vec![module];
        let main = main.path().to_owned();
        tab.update(&mut cx, |tab, _| {
            tab.repo = Some(main);
            tab.refs = std::sync::Arc::new(refs);
        });

        // The first line of its menu is "Open lib in a new tab".
        tab.update(&mut cx, |tab, cx| {
            tab.open_submodule_menu(
                "lib",
                gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.)),
                cx,
            );
            assert!(tab.step_menu(1, cx));
            tab.activate_menu(cx);
        });
        assert_eq!(state(&view, &cx), (2, 1), "a new tab shows the submodule");
    }
}
