//! Searching commits: a bar over the graph, and the commits it finds listed in place of
//! the graph until it is closed.

use super::GraphView;
use super::notice::Notice;
use super::parts::{TEXT_BODY, TEXT_SMALL, chip};
use super::rows::format_when;
use crate::theme::{selected_row, tokens};
use chrono::Utc;
use gitclient_core::history::search::{self, Mode, Results};
use gitclient_core::history::split_subject;
use gitclient_core::ops::Cancel;
use gitclient_core::text::short_oid;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::time::Duration;

/// How long typing pauses before git is asked, so that each key isn't a search.
const TYPING_PAUSE: Duration = Duration::from_millis(300);
const ROW_HEIGHT: Pixels = px(32.);
const BAR_HEIGHT: Pixels = px(44.);

#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    mode: Mode,
    input: Option<Entity<InputState>>,
    /// What the results below are for.
    searched: String,
    results: Option<Results>,
    error: Option<String>,
    running: bool,
    task: Option<Task<()>>,
    cancel: Option<Cancel>,
    scroll: UniformListScrollHandle,
    refocus: bool,
    _events: Vec<Subscription>,
}

impl State {
    /// The results replace the graph while there is something to show.
    pub fn is_showing_results(&self) -> bool {
        self.open && (self.results.is_some() || self.running || self.error.is_some())
    }

    fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.task = None;
        self.running = false;
    }
}

#[cfg(test)]
impl GraphView {
    /// Types `text` into the bar and searches at once, as Enter would.
    pub(super) fn search_for(
        &mut self,
        text: &str,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_search(window, cx);
        if let Some(input) = self.search.input.clone() {
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
        self.search.mode = mode;
        self.run_search(cx);
    }

    /// The subjects the search found, once it has finished.
    pub(super) fn search_subjects(&self) -> Option<Vec<String>> {
        let results = self.search.results.as_ref()?;
        Some(
            results
                .commits
                .iter()
                .map(|c| c.subject.to_string())
                .collect(),
        )
    }

    pub(super) fn search_is_open(&self) -> bool {
        self.search.open
    }

    pub(super) fn pick_search_result(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let oid = self.search.results.as_ref().unwrap().commits[index]
            .oid
            .clone();
        self.pick_result(&oid, window, cx);
    }
}

impl GraphView {
    /// Ctrl F: opens the bar with the cursor in it, or brings the cursor back to it.
    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo.is_none() {
            return;
        }
        self.search.open = true;
        if self.search.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type to search"));
            let events = cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => this.search_soon(cx),
                InputEvent::PressEnter { .. } => this.run_search(cx),
                _ => {}
            });
            self.search.input = Some(input);
            self.search._events = vec![events];
        }
        if let Some(input) = &self.search.input {
            window.focus(&input.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Escape or the close button: back to the graph, keeping the commit picked.
    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search.stop();
        self.search.open = false;
        self.search.results = None;
        self.search.error = None;
        self.search.searched.clear();
        // The next search starts empty.
        self.search.input = None;
        self.search._events.clear();
        // The input had the keys, and is gone: the next render hands them back.
        self.search.refocus = true;
        if let Some(index) = self.selected {
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Gives the keys back to the graph after the search closed. Called while rendering.
    pub(super) fn refocus_after_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.search.refocus) {
            window.focus(&self.focus, cx);
        }
    }

    fn set_search_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.search.mode != mode {
            self.search.mode = mode;
            self.run_search(cx);
        }
    }

    fn search_text(&self, cx: &App) -> String {
        self.search
            .input
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string())
    }

    /// Searches once typing has paused.
    fn search_soon(&mut self, cx: &mut Context<Self>) {
        self.search.stop();
        self.search.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_PAUSE).await;
            this.update(cx, |this, cx| this.run_search(cx)).ok();
        }));
        cx.notify();
    }

    /// Asks git, off the UI thread, and stops any search still running.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.search.stop();
        let query = self.search_text(cx);
        let (Some(repo), false) = (self.workdir(), query.trim().is_empty()) else {
            self.search.results = None;
            self.search.error = None;
            self.search.searched.clear();
            cx.notify();
            return;
        };
        let (mode, cancel) = (self.search.mode, Cancel::new());
        self.search.cancel = Some(cancel.clone());
        self.search.running = true;
        let asked = query.clone();
        self.search.task = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { search::search(&repo, &query, mode, &cancel) })
                .await;
            this.update(cx, |this, cx| {
                this.search.running = false;
                this.search.task = None;
                this.search.cancel = None;
                this.search.searched = asked;
                match found {
                    Ok(results) => {
                        this.search.results = Some(results);
                        this.search.error = None;
                    }
                    Err(message) => {
                        this.search.results = None;
                        this.search.error = Some(message);
                    }
                }
                this.search.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// A click on a result: shows it in the inspector and in the graph beneath, when the
    /// graph has loaded that far.
    fn pick_result(&mut self, oid: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.row_of(oid) {
            Some(index) => self.select(index, cx),
            None => {
                self.notice = Some(Notice::Info(
                    search::not_loaded_message(self.commit_count(), self.commit_limit()).into(),
                ));
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The bar over the graph.
    pub(super) fn render_search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search.open {
            return None;
        }
        let input = self.search.input.clone()?;
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let mode = self.search.mode;
        Some(
            div()
                .h_flex()
                .flex_none()
                .h(BAR_HEIGHT)
                .px_3()
                .gap_2()
                .bg(panel)
                .border_b_1()
                .border_color(border)
                .child(div().flex_1().min_w_0().child(Input::new(&input)))
                .children([Mode::Commit, Mode::File].map(|option| {
                    chip(
                        ("search-mode", option as usize),
                        option.label(),
                        mode == option,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.set_search_mode(option, cx)))
                }))
                .when(self.search.running, |bar| bar.child(Spinner::new().small()))
                .when(!self.search.running, |bar| {
                    bar.child(
                        div()
                            .flex_none()
                            .text_size(TEXT_SMALL)
                            .text_color(muted)
                            .child("Esc closes"),
                    )
                })
                .child(
                    Button::new("close-search")
                        .ghost()
                        .label("Close")
                        .tooltip("Back to the graph (Esc)")
                        .on_click(cx.listener(|this, _, _, cx| this.close_search(cx))),
                )
                .into_any_element(),
        )
    }

    /// What the search found, in place of the graph.
    pub(super) fn render_search_results(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search.is_showing_results() {
            return None;
        }
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let peach = tokens(theme.is_dark()).peach;
        let sentence = match (&self.search.error, &self.search.results) {
            (Some(message), _) => div().text_color(peach).child(message.clone()),
            (None, Some(results)) => div()
                .text_color(muted)
                .child(results.summary(&self.search.searched)),
            (None, None) => div().text_color(muted).child("Searching…"),
        };
        let count = self
            .search
            .results
            .as_ref()
            .map_or(0, |results| results.commits.len());
        let list = (count > 0).then(|| {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list(
                        "search-results",
                        count,
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_result_rows(range, cx)
                        }),
                    )
                    .track_scroll(&self.search.scroll)
                    .size_full(),
                )
                .vertical_scrollbar(&self.search.scroll)
        });
        Some(
            div()
                .v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .h_flex()
                        .flex_none()
                        .h(px(32.))
                        .px_3()
                        .border_b_1()
                        .border_color(border)
                        .text_size(TEXT_SMALL)
                        .child(sentence),
                )
                .children(list)
                .into_any_element(),
        )
    }

    fn render_result_rows(
        &self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let Some(results) = &self.search.results else {
            return Vec::new();
        };
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (foreground, muted, background) =
            (theme.foreground, theme.muted_foreground, theme.background);
        let (hover, active) = (theme.list_hover, selected_row(theme));
        let now = Utc::now().timestamp();
        range
            .filter_map(|index| {
                let commit = results.commits.get(index)?;
                let picked = self.selected_oid.as_deref() == Some(&*commit.oid);
                let loaded = self.row_of(&commit.oid).is_some();
                let (prefix, description) = split_subject(&commit.subject);
                let oid = commit.oid.clone();
                Some(
                    div()
                        .id(("result", index))
                        .h_flex()
                        .h(ROW_HEIGHT)
                        .w_full()
                        .px_3()
                        .gap_3()
                        .text_size(TEXT_BODY)
                        .bg(if picked { active } else { background })
                        .when(!picked, |row| row.hover(|style| style.bg(hover)))
                        .when(!loaded, |row| row.opacity(0.6))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_result(&oid, window, cx)
                        }))
                        .child(
                            div()
                                .w(px(64.))
                                .flex_none()
                                .text_color(tokens(is_dark).subtext0)
                                .child(short_oid(&commit.oid).to_owned()),
                        )
                        .child(
                            div()
                                .h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1p5()
                                .truncate()
                                .text_color(foreground)
                                .child(div().flex_none().text_color(muted).child(prefix.to_owned()))
                                .child(div().min_w_0().truncate().child(description.to_owned())),
                        )
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .truncate()
                                .text_right()
                                .text_color(muted)
                                .child(SharedString::from(commit.author.clone())),
                        )
                        .child(
                            div()
                                .w(px(72.))
                                .flex_none()
                                .text_right()
                                .text_color(muted)
                                .child(format_when(commit, now)),
                        ),
                )
            })
            .collect()
    }
}
