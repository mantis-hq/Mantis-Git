//! Pull and merge requests in the sidebar, for a remote on GitHub, GitLab or Gitea. What
//! they are and how git lists them lives in `gitclient_core::repo::forge`; this only asks
//! when the section is opened and draws the answer.

use super::{ITEM_HEIGHT, Section, State, indent};
use crate::theme::tokens;
use crate::view::GraphView;
use crate::view::parts::{TEXT_CHIP, TEXT_SMALL, panel_row};
use gitclient_core::menu;
use gitclient_core::repo::forge::{self, Requests, Site};
use gitclient_core::repo::remote::Remote;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// How the list of the repository's requests stands.
pub(in crate::view) enum Load {
    Loading,
    Done(Requests),
    Failed(String),
}

/// What was asked of which remote, so that a tab that moves to another repository doesn't
/// show the last one's list.
#[derive(Default)]
pub(super) struct Asked {
    key: String,
    load: Option<Load>,
}

impl State {
    fn requests_for(&self, key: &str) -> Option<&Load> {
        self.requests
            .load
            .as_ref()
            .filter(|_| self.requests.key == key)
    }
}

impl GraphView {
    /// The remote whose requests are shown, with where it is on the web. `None` when no
    /// remote is on a forge Mantis Git knows, and then the section isn't drawn.
    pub(super) fn request_source(&self) -> Option<(Remote, Site)> {
        let (remote, site) = forge::pick(&self.refs.remote_configs, self.refs.forge)?;
        Some((remote.clone(), site))
    }

    /// Asks a server that nothing else identifies what it is, once for each remote the
    /// view shows, so that a self-hosted GitLab or Gitea gets its section. It runs after
    /// a load and costs three small requests to a host the repository already talks to.
    pub(in crate::view) fn probe_forge(&mut self, cx: &mut Context<Self>) {
        let Some((_, address)) = forge::pick_unknown(&self.refs.remote_configs, self.refs.forge)
        else {
            return;
        };
        let origin = address.origin();
        if self.sidebar.probed.as_deref() == Some(origin.as_str()) {
            return;
        }
        self.sidebar.probed = Some(origin);
        Self::spawn_blocking(
            move || forge::probe::detect(&address, &forge::probe::Web),
            |_, found, cx| {
                // The section is drawn from what the probe left in memory.
                if found.is_some() {
                    cx.notify();
                }
            },
            cx,
        );
    }

    fn request_key(&self, remote: &Remote, site: &Site) -> String {
        format!("{:?}|{}|{}", self.workdir(), remote.name, site.web())
    }

    /// What is known of the list now.
    pub(super) fn loaded_requests(&self) -> Option<&Load> {
        let (remote, site) = self.request_source()?;
        self.sidebar.requests_for(&self.request_key(&remote, &site))
    }

    /// Asks the remote for its requests, which can take a while. The section says so.
    pub(super) fn load_requests(&mut self, cx: &mut Context<Self>) {
        let (Some((remote, site)), Some(workdir)) = (self.request_source(), self.workdir()) else {
            return;
        };
        let key = self.request_key(&remote, &site);
        self.sidebar.requests = Asked {
            key: key.clone(),
            load: Some(Load::Loading),
        };
        cx.notify();
        Self::spawn_blocking(
            move || forge::list(&workdir, &remote, &site),
            move |this, result, cx| {
                // Another remote or tab may have taken the section over meanwhile.
                if this.sidebar.requests.key == key {
                    this.sidebar.requests.load = Some(match result {
                        Ok(found) => Load::Done(found),
                        Err(message) => Load::Failed(message),
                    });
                    cx.notify();
                }
            },
            cx,
        );
    }

    /// Opens the section the first time, and asks then.
    pub(super) fn requests_opened(&mut self, cx: &mut Context<Self>) {
        if self.loaded_requests().is_none() {
            self.load_requests(cx);
        }
    }

    pub(super) fn open_request_menu(
        &mut self,
        number: u64,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((_, site)) = self.request_source() else {
            return;
        };
        self.show_menu(position, menu::request_menu(&site, number), cx);
    }

    /// The rows under the heading, as text: what is happening, or what went wrong, or what
    /// the list leaves out. A request itself has its own row.
    pub(super) fn requests_note(&self) -> Option<(String, bool)> {
        let (remote, site) = self.request_source()?;
        match self.loaded_requests()? {
            Load::Loading => Some((format!("Asking {}…", remote.name), false)),
            Load::Failed(message) => Some((message.clone(), true)),
            Load::Done(found) if found.newest.is_empty() => Some((
                format!("{} has no {}s.", remote.name, site.forge.noun()),
                false,
            )),
            Load::Done(_) => None,
        }
    }

    /// "Pull requests" or "Merge requests", ending in a quiet "Refresh" once it has asked.
    pub(super) fn render_requests_header(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (_, site) = self.request_source()?;
        let open = self.sidebar.expanded.contains(&Section::Requests);
        let theme = cx.theme();
        let (link, muted) = (theme.primary, theme.muted_foreground);
        let title = site.forge.heading();
        let shown = match self.loaded_requests() {
            Some(Load::Done(found)) => Some(found.total),
            _ => None,
        };
        let refreshable = open && !matches!(self.loaded_requests(), Some(Load::Loading) | None);
        Some(
            panel_row("requests-header", false, cx)
                .h(px(30.))
                .justify_between()
                .text_color(tokens(theme.is_dark()).subtext0)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_section(Section::Requests, cx);
                    if this.sidebar.expanded.contains(&Section::Requests) {
                        this.requests_opened(cx);
                    }
                }))
                .child(div().flex_1().child(title))
                .when(refreshable, |row| {
                    row.child(
                        div()
                            .id("refresh-requests")
                            .flex_none()
                            .px_1()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_color(link)
                            .font_semibold()
                            .child("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                // The header under it would otherwise fold the section.
                                cx.stop_propagation();
                                this.load_requests(cx);
                            })),
                    )
                })
                .when_some(shown, |row, total| {
                    row.child(
                        div()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(total.to_string()),
                    )
                }),
        )
    }

    /// One request: its number, then what its newest commit says, or that commit's hash.
    /// Clicking opens it on the forge, and right-clicking opens its menu.
    pub(super) fn render_request_row(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let Some(Load::Done(found)) = self.loaded_requests() else {
            return None;
        };
        let request = found.newest.get(index)?;
        let (_, site) = self.request_source()?;
        let theme = cx.theme();
        let (muted, text) = (theme.muted_foreground, theme.foreground);
        let number = request.number;
        let url = site.request_url(number);
        Some(
            panel_row(("request", index), false, cx)
                .h(px(30.))
                .pl(indent(0))
                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_request_menu(number, event.position, cx);
                    }),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(text)
                        .font_semibold()
                        .child(site.label(number)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(TEXT_SMALL)
                        .text_color(muted)
                        .child(request.detail()),
                ),
        )
    }

    /// A sentence in the list, at the height of a row, with room to wrap into a second line.
    pub(super) fn render_requests_note(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (text, is_error) = self.requests_note()?;
        let theme = cx.theme();
        let color = if is_error {
            tokens(theme.is_dark()).peach
        } else {
            theme.muted_foreground
        };
        Some(
            div()
                .h(ITEM_HEIGHT)
                .px_2()
                .pl(indent(0))
                .h_flex()
                .text_size(TEXT_SMALL)
                .text_color(color)
                .child(div().min_w_0().truncate().child(text)),
        )
    }
}

#[cfg(test)]
impl GraphView {
    /// Stands in for the answer of the remote, so that a test needs no network. It also
    /// opens the section, as the click that asks would.
    pub(in crate::view) fn set_requests_for_test(&mut self, load: Load) {
        let (remote, site) = self.request_source().expect("a remote on a forge");
        self.sidebar.requests = Asked {
            key: self.request_key(&remote, &site),
            load: Some(load),
        };
        if !self.sidebar.expanded.contains(&Section::Requests) {
            self.sidebar.expanded.push(Section::Requests);
        }
    }

    /// The sidebar's request rows as text, from the heading down to the sentence under them.
    pub(in crate::view) fn request_rows_for_test(&self) -> Vec<String> {
        use super::Item;
        let site = self.request_source().map(|(_, site)| site);
        let mut rows = Vec::new();
        for item in self.sidebar_items() {
            match item {
                Item::RequestsHeader => {
                    rows.push(site.as_ref().unwrap().forge.heading().to_owned())
                }
                Item::Request(index) => {
                    let Some(Load::Done(found)) = self.loaded_requests() else {
                        continue;
                    };
                    let request = &found.newest[index];
                    let label = site.as_ref().unwrap().label(request.number);
                    rows.push(format!("{label} {}", request.detail()));
                }
                Item::RequestsNote => rows.push(self.requests_note().unwrap().0),
                _ => {}
            }
        }
        rows
    }
}
