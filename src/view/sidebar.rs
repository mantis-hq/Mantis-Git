//! The branch sidebar. Clicking a branch spotlights its history in the graph.

use super::GraphView;
use crate::git::NamedRef;
use crate::spotlight::Spotlight;
use crate::theme::{lane_color, selected_row, tokens};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::sync::Arc;

const WIDTH: Pixels = px(300.);
/// Below this the sidebar starts closed, so the graph keeps some room.
pub(super) const WIDE_WINDOW: Pixels = px(1500.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Remotes,
    Tags,
    Stashes,
}

/// One clickable row: a branch, remote branch or tag.
struct RefRow<'a> {
    id: ElementId,
    name: &'a str,
    tip: Arc<str>,
    color: Hsla,
    detail: String,
    on: bool,
    strong: bool,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) window_is_wide: bool,
    /// What the user chose, which outranks the window's width.
    open: Option<bool>,
    pub(super) spotlight: Option<Arc<Spotlight>>,
    expanded: Vec<Section>,
}

impl GraphView {
    pub(super) fn sidebar_is_open(&self) -> bool {
        self.sidebar.open.unwrap_or(self.sidebar.window_is_wide)
    }

    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar.open = Some(!self.sidebar_is_open());
        cx.notify();
    }

    /// Fades everything outside a branch's history, or turns that off again.
    pub(super) fn spotlight(&mut self, tip: Arc<str>, cx: &mut Context<Self>) {
        let same = self
            .sidebar
            .spotlight
            .as_ref()
            .is_some_and(|s| s.tip == tip);
        self.sidebar.spotlight =
            (!same).then(|| Arc::new(Spotlight::new(tip.clone(), &self.entries)));
        if !same && let Some(row) = self.entries.iter().position(|e| e.commit.oid == tip) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// History changed, so the same branch's lit commits need working out again.
    pub(super) fn refresh_spotlight(&mut self) {
        if let Some(spot) = &self.sidebar.spotlight {
            let spot = Spotlight::new(spot.tip.clone(), &self.entries);
            self.sidebar.spotlight = Some(Arc::new(spot));
        }
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        match self.sidebar.expanded.iter().position(|s| *s == section) {
            Some(index) => {
                self.sidebar.expanded.remove(index);
            }
            None => self.sidebar.expanded.push(section),
        }
        cx.notify();
    }

    pub(super) fn render_sidebar_toggle(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.refs.branches.first()?;
        Some(
            Button::new("toggle-sidebar")
                .ghost()
                .small()
                .label(if self.sidebar_is_open() {
                    "Hide branches"
                } else {
                    "Branches"
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
                .into_any_element(),
        )
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.sidebar_is_open() {
            return None;
        }
        let theme = cx.theme();
        let (panel, border, muted, link) = (
            theme.title_bar,
            theme.border,
            theme.muted_foreground,
            theme.primary,
        );
        let spotlit = self.sidebar.spotlight.clone();
        let label = |text: &'static str| {
            div()
                .h_flex()
                .flex_1()
                .text_size(px(13.))
                .font_medium()
                .text_color(muted)
                .child(text)
        };

        let mut branches = Vec::with_capacity(self.refs.branches.len());
        for (index, branch) in self.refs.branches.iter().enumerate() {
            let on = spotlit.as_ref().is_some_and(|s| s.tip == branch.tip);
            let delta = branch.delta(self.refs.default_branch.as_deref());
            let color = lane_color(branch.color, cx.theme().is_dark());
            let row = RefRow {
                id: ("branch", index).into(),
                name: &branch.name,
                tip: branch.tip.clone(),
                color,
                detail: delta,
                on,
                strong: branch.is_head,
            };
            branches.push(self.render_ref_row(row, cx));
        }
        let remotes = self.render_section(Section::Remotes, "Remotes", &self.refs.remotes, cx);
        let tags = self.render_section(Section::Tags, "Tags", &self.refs.tags, cx);
        let stashes = self.render_stashes(cx);

        Some(
            div()
                .w(WIDTH)
                .flex_none()
                .bg(panel)
                .border_r_1()
                .border_color(border)
                .child(
                    div()
                        .id("sidebar")
                        .v_flex()
                        .size_full()
                        .px_2()
                        .py_3()
                        .gap_4()
                        .overflow_y_scrollbar()
                        .child(
                            div()
                                .v_flex()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .h_flex()
                                        .h(px(24.))
                                        .px_2()
                                        .child(label("Branches"))
                                        .when(spotlit.is_some(), |heading| {
                                            let tip = spotlit.as_ref().map(|s| s.tip.clone());
                                            heading.child(
                                                div()
                                                    .id("show-all")
                                                    .cursor_pointer()
                                                    .text_size(px(13.))
                                                    .font_medium()
                                                    .text_color(link)
                                                    .child("Show all")
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            if let Some(tip) = tip.clone() {
                                                                this.spotlight(tip, cx);
                                                            }
                                                        },
                                                    )),
                                            )
                                        }),
                                )
                                .children(branches)
                                .child(
                                    div()
                                        .mt_1()
                                        .px_2()
                                        .text_size(px(13.))
                                        .text_color(muted)
                                        .child("Click a branch to trace its history."),
                                ),
                        )
                        .child(
                            div()
                                .v_flex()
                                .gap(px(2.))
                                .child(remotes)
                                .child(tags)
                                .child(stashes),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_ref_row(&self, row: RefRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let RefRow {
            id,
            name,
            tip,
            color,
            detail,
            on,
            strong,
        } = row;
        let theme = cx.theme();
        let (hover, muted) = (theme.list_hover, theme.muted_foreground);
        let active = selected_row(theme);
        let text = if strong || on {
            theme.foreground
        } else {
            theme.secondary_foreground
        };
        div()
            .id(id)
            .h_flex()
            .h(px(30.))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .when(on, |row| row.bg(active))
            .when(!on, |row| row.hover(|style| style.bg(hover)))
            .on_click(cx.listener(move |this, _, _, cx| this.spotlight(tip.clone(), cx)))
            .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(text)
                    .when(strong, |name| name.font_semibold())
                    .child(name.to_owned()),
            )
            .child(div().flex_none().text_xs().text_color(muted).child(detail))
    }

    fn render_section(
        &self,
        section: Section,
        title: &'static str,
        refs: &[NamedRef],
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.sidebar.expanded.contains(&section);
        let header = self.render_section_header(section, title, refs.len(), cx);
        let spotlit = self.sidebar.spotlight.clone();
        let mut rows = Vec::new();
        if open {
            for (index, named) in refs.iter().enumerate() {
                let on = spotlit.as_ref().is_some_and(|s| s.tip == named.tip);
                let is_dark = cx.theme().is_dark();
                let color = match section {
                    Section::Tags => cx.theme().warning,
                    _ => lane_color(named.color, is_dark),
                };
                let row = RefRow {
                    id: (title, index).into(),
                    name: &named.name,
                    tip: named.tip.clone(),
                    color,
                    detail: String::new(),
                    on,
                    strong: false,
                };
                rows.push(self.render_ref_row(row, cx));
            }
        }
        div().v_flex().gap(px(2.)).child(header).children(rows)
    }

    fn render_stashes(&self, cx: &mut Context<Self>) -> Div {
        let count = self.refs.stashes.len();
        let open = self.sidebar.expanded.contains(&Section::Stashes);
        let header = self.render_section_header(Section::Stashes, "Stashes", count, cx);
        let subtext = tokens(cx.theme().is_dark()).subtext0;
        let rows = self.refs.stashes.iter().filter(|_| open).map(|stash| {
            div()
                .h_flex()
                .h(px(30.))
                .px_2()
                .text_color(subtext)
                .child(div().min_w_0().truncate().child(stash.clone()))
        });
        div().v_flex().gap(px(2.)).child(header).children(rows)
    }

    fn render_section_header(
        &self,
        section: Section,
        title: &'static str,
        count: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        div()
            .id(title)
            .h_flex()
            .h(px(30.))
            .px_2()
            .justify_between()
            .rounded(px(6.))
            .cursor_pointer()
            .text_color(tokens(theme.is_dark()).subtext0)
            .hover(|style| style.bg(theme.list_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(title)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(count.to_string()),
            )
    }
}
