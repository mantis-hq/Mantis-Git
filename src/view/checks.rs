//! What the forge's CI says, drawn in three places: a column of the graph with one mark per
//! commit, a "Checks" tab beside a commit's files with its pipelines and jobs, and a job's
//! log over the graph. What it is and how it is read lives in `mantisgit_core::repo::ci`;
//! this asks, off the UI thread, and draws the answer.
//!
//! None of it exists unless someone is signed in to the repository's forge (Settings →
//! Accounts): without an account there is no column, no tab, and nothing asked.

use super::GraphView;
use super::parts::{TEXT_BODY, TEXT_SMALL, chip, loading, section_label};
use crate::icons::Icon;
use crate::theme::{selected_row, tokens};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mantisgit_core::auth;
use mantisgit_core::repo::ci::{self, Job, Pipeline, Status, Target};
use mantisgit_core::repo::forge;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How wide the column is, which is a mark and a little room.
pub(super) const WIDTH: f32 = 56.;
/// While anything is still running, the forge is asked again after this, then after twice
/// as long each time nothing changed, up to `POLL_MOST`, so a build that runs for an hour
/// isn't asked about 180 times. Anything that changes starts it over.
const POLL: Duration = Duration::from_secs(20);
const POLL_MOST: Duration = Duration::from_secs(300);
/// Coming back to the window asks again, unless it was asked this recently. A fetch that
/// finds nothing new is a 304, but it is still a round trip.
const STALE: Duration = Duration::from_secs(60);
const LOG_ROW: Pixels = px(20.);
const LOG_NUMBER: Pixels = px(56.);
/// A line longer than this is cut, since one of megabytes would take the window a second
/// to lay out, as in a diff.
const LINE_LIMIT: usize = 1000;

/// What the inspector shows beside a commit's summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Files,
    Checks,
}

/// What the newest pipelines said, by commit, or why they couldn't be read.
type Recent = Result<HashMap<String, Status>, String>;

enum Load<T> {
    Loading,
    Done(T),
    Failed(String),
}

struct Detail {
    oid: Arc<str>,
    load: Load<Vec<Pipeline>>,
}

/// A log as lines, with the longest remembered for the list's width.
struct Lines {
    lines: Vec<String>,
    widest: Option<usize>,
}

struct LogView {
    pipeline: String,
    job: Job,
    load: Load<Arc<Lines>>,
}

#[derive(Default)]
pub(super) struct State {
    /// Where CI is read from: `None` unless someone is signed in to the remote's server.
    target: Option<Target>,
    /// The newest pipelines, as one status for each commit they ran on.
    statuses: HashMap<String, Status>,
    /// Why the last read failed, shown where the pipelines would be.
    error: Option<String>,
    refreshed: Option<Instant>,
    pub(super) tab: Tab,
    detail: Option<Detail>,
    log: Option<LogView>,
    log_scroll: UniformListScrollHandle,
    /// Asks again later while something runs. Dropping it cancels the wait.
    poll: Option<Task<()>>,
    /// How many times in a row that wait ended with nothing new, which lengthens it.
    quiet_polls: u32,
    /// A read of the newest pipelines is on its way, so another isn't started beside it.
    reading: bool,
}

impl GraphView {
    /// Whether the graph has the column and the inspector the tab.
    pub(super) fn checks_shown(&self) -> bool {
        self.checks.target.is_some()
    }

    /// Looks again at who is signed in and what the newest pipelines say. Runs after a load
    /// and when the window is used again, so signing in on the Accounts page shows up
    /// here without a restart.
    pub(super) fn refresh_checks(&mut self, cx: &mut Context<Self>) {
        let site = forge::pick(&self.refs.remote_configs, self.refs.forge).map(|(_, site)| site);
        let repo = self.repo.clone();
        // One read at a time: a load, a focus and a poll landing together are one question.
        if self.checks.reading {
            return;
        }
        self.checks.reading = true;
        self.checks.refreshed = Some(Instant::now());
        Self::spawn_blocking(
            move || {
                let target = ci::target(site.as_ref(), &auth::accounts())?;
                let found = ci::recent_live(&target);
                Some((target, found))
            },
            move |this, result, cx| {
                this.checks.reading = false;
                // The tab may have moved to another repository meanwhile.
                if this.repo != repo {
                    return;
                }
                this.checks_arrived(result, cx);
            },
            cx,
        );
    }

    /// Like [`refresh_checks`](Self::refresh_checks) but not when it just ran.
    pub(super) fn refresh_checks_if_stale(&mut self, cx: &mut Context<Self>) {
        if self.checks.refreshed.is_none_or(|at| at.elapsed() > STALE) {
            self.refresh_checks(cx);
        }
    }

    fn checks_arrived(&mut self, result: Option<(Target, Recent)>, cx: &mut Context<Self>) {
        let Some((target, found)) = result else {
            // Signed out, or a repository on no forge: all of it goes.
            let had = self.checks.target.take().is_some();
            self.checks.statuses.clear();
            self.checks.error = None;
            self.checks.detail = None;
            self.checks.log = None;
            self.checks.tab = Tab::Files;
            self.checks.poll = None;
            if had {
                cx.notify();
            }
            return;
        };
        let appeared = self.checks.target.as_ref() != Some(&target);
        self.checks.target = Some(target);
        match found {
            Ok(found) => {
                self.checks.error = None;
                // What a commit's own read said is newer than the list for the commits it
                // names, so those are kept.
                let kept = self
                    .checks
                    .detail
                    .as_ref()
                    .and_then(|detail| match &detail.load {
                        Load::Done(pipelines) => Some((
                            detail.oid.to_string(),
                            Status::worst(pipelines.iter().map(|p| p.status))?,
                        )),
                        _ => None,
                    });
                if found != self.checks.statuses {
                    self.checks.quiet_polls = 0;
                }
                // The list is the newer word. When it disagrees with what the commit's own
                // read said (a re-run, say), that read is out of date and is asked again.
                let stale = kept
                    .as_ref()
                    .is_some_and(|(oid, status)| found.get(oid).is_some_and(|s| s != status));
                self.checks.statuses = found;
                match kept {
                    Some((oid, status)) if !stale => {
                        self.checks.statuses.insert(oid, status);
                    }
                    _ if stale => {
                        if self.checks.tab == Tab::Checks {
                            self.load_checks_detail(true, cx);
                        } else {
                            self.checks.detail = None;
                        }
                    }
                    _ => {}
                }
            }
            Err(message) => self.checks.error = Some(message),
        }
        if appeared {
            self.checks_selection_changed(cx);
        }
        self.schedule_poll(cx);
        cx.notify();
    }

    /// Asks again soon if anything is still going, so a running job turns into a result
    /// without the person doing anything.
    fn schedule_poll(&mut self, cx: &mut Context<Self>) {
        let detail_active = self.checks.detail.as_ref().is_some_and(|detail| {
            matches!(&detail.load, Load::Done(p) if p.iter().any(|p| p.status.is_active()))
        });
        let active = detail_active || self.checks.statuses.values().any(|s| s.is_active());
        let wait = (POLL * 2u32.saturating_pow(self.checks.quiet_polls.min(4))).min(POLL_MOST);
        self.checks.poll = active.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                this.update(cx, |this, cx| {
                    this.checks.poll = None;
                    this.checks.quiet_polls += 1;
                    this.refresh_checks(cx);
                    // The jobs of one commit are only worth asking about while they show.
                    if this.checks.tab == Tab::Checks {
                        this.load_checks_detail(true, cx);
                    }
                })
                .ok();
            })
        });
    }

    /// The selection moved: read the new commit's pipelines if its tab is the one showing.
    pub(super) fn checks_selection_changed(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_commit().map(|commit| commit.oid.clone());
        let same = self
            .checks
            .detail
            .as_ref()
            .is_some_and(|detail| Some(&detail.oid) == selected.as_ref());
        if !same {
            self.checks.detail = None;
        }
        if self.checks.tab == Tab::Checks {
            self.load_checks_detail(false, cx);
        }
    }

    pub(super) fn set_inspector_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.checks.tab = tab;
        if tab == Tab::Checks {
            self.load_checks_detail(false, cx);
        }
        cx.notify();
    }

    /// Reads the selected commit's pipelines and their jobs. `again` reads them even when
    /// they are already here, which is how a refresh sees a job finish.
    fn load_checks_detail(&mut self, again: bool, cx: &mut Context<Self>) {
        let caches = self.caches.clone();
        let (Some(target), Some(commit)) = (self.checks.target.clone(), self.selected_commit())
        else {
            return;
        };
        let oid = commit.oid.clone();
        let here = self
            .checks
            .detail
            .as_ref()
            .is_some_and(|detail| detail.oid == oid && !matches!(detail.load, Load::Failed(_)));
        if here && !again {
            return;
        }
        // A refresh keeps what is showing until the new read replaces it.
        if !here || self.checks.detail.as_ref().is_none_or(|d| d.oid != oid) {
            self.checks.detail = Some(Detail {
                oid: oid.clone(),
                load: Load::Loading,
            });
            cx.notify();
        }
        let sha = oid.to_string();
        Self::spawn_blocking(
            move || ci::for_commit_cached(&target, &caches, &sha, again),
            move |this, result, cx| {
                // Another commit may have been picked while this was read.
                let current = this.checks.detail.as_ref().is_some_and(|d| d.oid == oid);
                if !current {
                    return;
                }
                let load = match result {
                    Ok(pipelines) => {
                        let changed = !matches!(
                            this.checks.detail.as_ref().map(|d| &d.load),
                            Some(Load::Done(before)) if *before == pipelines
                        );
                        if changed {
                            this.checks.quiet_polls = 0;
                        }
                        match Status::worst(pipelines.iter().map(|p| p.status)) {
                            Some(status) => this.checks.statuses.insert(oid.to_string(), status),
                            None => this.checks.statuses.remove(&*oid),
                        };
                        Load::Done(pipelines)
                    }
                    Err(message) => Load::Failed(message),
                };
                this.checks.detail = Some(Detail {
                    oid: oid.clone(),
                    load,
                });
                this.schedule_poll(cx);
                cx.notify();
            },
            cx,
        );
    }

    /// Opens a job's log over the graph.
    fn open_job_log(&mut self, pipeline: String, job: Job, cx: &mut Context<Self>) {
        let Some(target) = self.checks.target.clone() else {
            return;
        };
        let id = job.id.clone();
        let caches = self.caches.clone();
        self.checks.log = Some(LogView {
            pipeline,
            job: job.clone(),
            load: Load::Loading,
        });
        self.checks.log_scroll = UniformListScrollHandle::new();
        cx.notify();
        Self::spawn_blocking(
            move || ci::log_cached(&target, &caches, &job),
            move |this, result, cx| {
                let Some(view) = this.checks.log.as_mut().filter(|view| view.job.id == id) else {
                    return;
                };
                view.load = match result {
                    Ok(lines) => {
                        let lines: Vec<String> = lines.into_iter().map(clip).collect();
                        let widest = (0..lines.len()).max_by_key(|&at| lines[at].len());
                        let last = lines.len().saturating_sub(1);
                        // What went wrong is at the end, so the log opens there.
                        this.checks
                            .log_scroll
                            .scroll_to_item(last, ScrollStrategy::Bottom);
                        Load::Done(Arc::new(Lines { lines, widest }))
                    }
                    Err(message) => Load::Failed(message),
                };
                cx.notify();
            },
            cx,
        );
    }

    pub(super) fn log_is_open(&self) -> bool {
        self.checks.log.is_some()
    }

    pub(super) fn close_log(&mut self, cx: &mut Context<Self>) {
        self.checks.log = None;
        cx.notify();
    }

    /// A file's diff takes the graph's place from the log, since only one can.
    pub(super) fn drop_log(&mut self) {
        self.checks.log = None;
    }

    pub(super) fn forget_checks(&mut self) {
        self.checks = State::default();
    }

    fn status_color(&self, status: Status, cx: &App) -> Hsla {
        let theme = cx.theme();
        match status {
            Status::Passed => theme.success,
            Status::Failed => theme.danger,
            Status::Running => tokens(theme.is_dark()).subtext0,
            Status::Queued | Status::Cancelled | Status::Skipped => theme.muted_foreground,
        }
    }

    /// The column's cell for a commit: one mark, whose shape and tooltip say what it is, so
    /// no meaning rests on its colour.
    pub(super) fn render_checks_cell(
        &self,
        index: usize,
        oid: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cell = div()
            .id(("checks-cell", index))
            .w(px(WIDTH))
            .h_full()
            .flex_none()
            .h_flex();
        let Some(status) = self.checks.statuses.get(oid).copied() else {
            return cell.into_any_element();
        };
        let color = self.status_color(status, cx);
        let hint = format!("{}. Click for the jobs.", status.label());
        cell.cursor_pointer()
            .child(
                svg()
                    .path(status_icon_of(status).path())
                    .size(px(14.))
                    .text_color(color),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(hint.clone()).build(window, cx)
            })
            .tooltip_show_delay(Duration::ZERO)
            .on_click(cx.listener(|this, _, _, cx| this.set_inspector_tab(Tab::Checks, cx)))
            .into_any_element()
    }

    /// The Files and Checks chips over a commit's lists.
    pub(super) fn render_inspector_tabs(&self, cx: &mut Context<Self>) -> Div {
        let checks = match self
            .selected_commit()
            .and_then(|commit| self.checks.statuses.get(&*commit.oid))
        {
            Some(status) => format!("Checks · {}", status.label()),
            None => "Checks".to_owned(),
        };
        let tabs = [(Tab::Files, "Files".to_owned()), (Tab::Checks, checks)];
        div()
            .h_flex()
            .flex_none()
            .gap_1()
            .children(tabs.into_iter().map(|(tab, label)| {
                chip(
                    ("inspector-tab", tab as usize),
                    &label,
                    self.checks.tab == tab,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_inspector_tab(tab, cx)))
            }))
    }

    /// The Checks tab: each pipeline of the commit with its jobs, which open their logs.
    pub(super) fn render_checks(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, warn) = (theme.muted_foreground, tokens(theme.is_dark()).peach);
        let subtext = tokens(theme.is_dark()).subtext0;
        let forge = self.checks.target.as_ref().map(|t| t.forge);
        let body = div().v_flex().flex_1().min_h_0().gap_2();
        let selected = self.selected_commit().map(|commit| commit.oid.clone());
        let detail = self
            .checks
            .detail
            .as_ref()
            .filter(|detail| Some(&detail.oid) == selected.as_ref());
        let refresh = Button::new("checks-refresh")
            .ghost()
            .xsmall()
            .label("Refresh")
            .on_click(cx.listener(|this, _, _, cx| {
                this.checks.quiet_polls = 0;
                this.refresh_checks(cx);
                this.load_checks_detail(true, cx);
            }));
        let heading = section_label(muted)
            .h_flex()
            .h(px(24.))
            .justify_between()
            .child("Pipelines")
            .child(refresh);
        let body = body.child(heading);
        match detail.map(|detail| &detail.load) {
            None | Some(Load::Loading) => body
                .child(loading("checks-loading", "Loading checks…", 4, cx))
                .into_any_element(),
            Some(Load::Failed(message)) => body
                .child(div().text_color(warn).child(message.clone()))
                .into_any_element(),
            Some(Load::Done(pipelines)) if pipelines.is_empty() => body
                .child(
                    div()
                        .text_color(subtext)
                        .child(forge.map(ci::none_sentence).unwrap_or_default()),
                )
                .into_any_element(),
            Some(Load::Done(pipelines)) => {
                let rows: Vec<_> = pipelines
                    .iter()
                    .enumerate()
                    .map(|(at, pipeline)| self.render_pipeline(at, pipeline, cx))
                    .collect();
                body.child(
                    div()
                        .id("checks-list")
                        .v_flex()
                        .flex_1()
                        .min_h_0()
                        .gap_3()
                        .overflow_y_scrollbar()
                        .children(rows),
                )
                .into_any_element()
            }
        }
    }

    fn render_pipeline(
        &self,
        at: usize,
        pipeline: &Pipeline,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let foreground = theme.foreground;
        let active = selected_row(theme);
        let color = self.status_color(pipeline.status, cx);
        let url = pipeline.url.clone();
        let name = pipeline.name.clone();
        let jobs = pipeline.jobs.iter().enumerate().map(|(index, job)| {
            let job_color = self.status_color(job.status, cx);
            let (job_for_click, pipeline_name) = (job.clone(), name.clone());
            let opened = self
                .checks
                .log
                .as_ref()
                .is_some_and(|view| view.job.id == job.id);
            div()
                .id(("checks-job", at * 1000 + index))
                .h_flex()
                .h(px(28.))
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(TEXT_BODY)
                .when(opened, |row| row.bg(active))
                .when(!opened, |row| row.hover(|style| style.bg(hover)))
                .child(
                    svg()
                        .path(status_icon_of(job.status).path())
                        .size(px(14.))
                        .flex_none()
                        .text_color(job_color),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_flex()
                        .gap_2()
                        .child(div().truncate().child(job.name.clone()))
                        .when(!job.group.is_empty(), |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_size(TEXT_SMALL)
                                    .text_color(muted)
                                    .child(job.group.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TEXT_SMALL)
                        .text_color(muted)
                        .child(match job.seconds {
                            Some(seconds) if !job.status.is_active() => ci::duration(seconds),
                            _ => job.status.label().to_owned(),
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_job_log(pipeline_name.clone(), job_for_click.clone(), cx)
                }))
        });
        div()
            .v_flex()
            .gap(px(2.))
            .child(
                div()
                    .h_flex()
                    .h(px(28.))
                    .px_2()
                    .gap_2()
                    .child(
                        svg()
                            .path(status_icon_of(pipeline.status).path())
                            .size(px(14.))
                            .flex_none()
                            .text_color(color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_semibold()
                            .text_color(foreground)
                            .child(pipeline.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_SMALL)
                            .text_color(muted)
                            .child(pipeline.summary()),
                    )
                    .when(!url.is_empty(), |row| {
                        row.child(
                            Button::new(("checks-open", at))
                                .ghost()
                                .xsmall()
                                .label("Open")
                                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                        )
                    }),
            )
            .children(jobs)
            .into_any_element()
    }

    /// A job's log, drawn in place of the graph.
    pub(super) fn render_log(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.checks.log.as_ref()?;
        let theme = cx.theme();
        let (border, panel, muted) = (theme.border, theme.title_bar, theme.muted_foreground);
        let background = theme.background;
        let warn = tokens(theme.is_dark()).peach;
        let color = self.status_color(view.job.status, cx);
        let forge = self.checks.target.as_ref().map(|t| t.forge);
        let url = view.job.url.clone();
        let header = div()
            .h_flex()
            .h(px(40.))
            .flex_none()
            .bg(panel)
            .pl_4()
            .pr_2()
            .gap_3()
            .text_size(TEXT_SMALL)
            .border_b_1()
            .border_color(border)
            .child(
                svg()
                    .path(status_icon_of(view.job.status).path())
                    .size(px(14.))
                    .flex_none()
                    .text_color(color),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_flex()
                    .gap_2()
                    .child(
                        div()
                            .truncate()
                            .font_semibold()
                            .text_size(TEXT_BODY)
                            .child(view.job.name.clone()),
                    )
                    .child(div().truncate().text_color(muted).child(format!(
                        "{} · {}",
                        view.pipeline,
                        view.job.status.label()
                    ))),
            )
            .when(!url.is_empty(), |header| {
                let label = forge.map_or("Open".to_owned(), |f| format!("Open on {}", f.name()));
                header.child(
                    Button::new("log-open")
                        .ghost()
                        .xsmall()
                        .label(label)
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                )
            })
            .child(
                Button::new("close-log")
                    .ghost()
                    .xsmall()
                    .label("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close_log(cx))),
            );
        let body = match &view.load {
            Load::Loading => div()
                .p_4()
                .child(loading("log-loading", "Loading the log…", 6, cx))
                .into_any_element(),
            Load::Failed(message) => div()
                .p_4()
                .text_size(TEXT_BODY)
                .text_color(warn)
                .child(message.clone())
                .into_any_element(),
            Load::Done(lines) if lines.lines.is_empty() => div()
                .p_4()
                .text_size(TEXT_BODY)
                .text_color(muted)
                .child("This job has no log yet.")
                .into_any_element(),
            Load::Done(lines) => self.render_log_lines(lines.clone(), cx).into_any_element(),
        };
        Some(
            div()
                .v_flex()
                .flex_1()
                .min_h_0()
                .bg(background)
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_log_lines(&self, lines: Arc<Lines>, cx: &mut Context<Self>) -> impl IntoElement {
        let widest = lines.widest;
        let count = lines.lines.len();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "log-lines",
                    count,
                    cx.processor(move |_, range: Range<usize>, _, cx| log_rows(&lines, range, cx)),
                )
                .with_width_from_item(widest)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .track_scroll(&self.checks.log_scroll)
                .size_full(),
            )
            .scrollbar(&self.checks.log_scroll, ScrollbarAxis::Both)
    }
}

fn log_rows(lines: &Lines, range: Range<usize>, cx: &mut Context<GraphView>) -> Vec<Div> {
    let theme = cx.theme();
    let colors = tokens(theme.is_dark());
    let failed = theme.danger;
    range
        .filter(|at| *at < lines.lines.len())
        .map(|at| {
            let text = &lines.lines[at];
            // `error` and GitHub's `##[error]` mark what to look at; the word is in the
            // line, so the colour only helps.
            let bad = text.contains("##[error]") || text.starts_with("error");
            div()
                .h_flex()
                .h(LOG_ROW)
                .min_w_full()
                .px_3()
                .text_size(TEXT_SMALL)
                .whitespace_nowrap()
                .child(
                    div()
                        .w(LOG_NUMBER)
                        .flex_none()
                        .pr_3()
                        .text_right()
                        .text_color(colors.overlay1)
                        .child((at + 1).to_string()),
                )
                .child(
                    div()
                        .text_color(if bad { failed } else { colors.subtext0 })
                        .child(super::parts::expand_tabs(text)),
                )
        })
        .collect()
}

/// A line cut to what is worth laying out, with how much was left out.
fn clip(line: String) -> String {
    if line.chars().count() <= LINE_LIMIT {
        return line;
    }
    let kept: String = line.chars().take(LINE_LIMIT).collect();
    let more = line.chars().count() - LINE_LIMIT;
    format!("{kept}… {more} more characters")
}

/// The shape for a status. Shapes differ, so a person who can't tell the colours apart can
/// still read them.
fn status_icon_of(status: Status) -> Icon {
    match status {
        Status::Passed => Icon::CircleCheck,
        Status::Failed => Icon::CircleX,
        Status::Running => Icon::CircleDot,
        Status::Queued => Icon::CircleDashed,
        Status::Cancelled | Status::Skipped => Icon::CircleSlash,
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
    use super::{Icon, Status, clip, status_icon_of};

    #[test]
    fn long_lines_are_cut_and_say_how_much() {
        let long = "x".repeat(1010);
        assert_eq!(
            clip(long),
            format!("{}… 10 more characters", "x".repeat(1000))
        );
        assert_eq!(clip("short".into()), "short");
    }

    #[test]
    fn every_status_has_a_shape_of_its_own_where_the_meaning_differs() {
        let shapes: Vec<Icon> = [
            Status::Passed,
            Status::Failed,
            Status::Running,
            Status::Queued,
            Status::Skipped,
        ]
        .map(status_icon_of)
        .to_vec();
        for (at, shape) in shapes.iter().enumerate() {
            assert!(!shapes[..at].contains(shape));
        }
    }
}
