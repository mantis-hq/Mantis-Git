//! The task screen for writing a commit: choose hunks, say what they do.

use super::parts::{
    Edge, LineStyle, checkbox, expand_tabs, file_path, fit, grip, header_bar, header_divider,
    line_style, on_resize, panel_row, section_label, status_color,
};
use super::parts::{TEXT_CHIP, TEXT_SMALL, TEXT_STATUS};
use super::{GraphView, Notice, Screen};
use crate::theme::tokens;
use gitclient_core::patch::Hunk;
use gitclient_core::text::count;
use gitclient_core::working::{
    self, Bump, COMMIT_TYPES, Included, Selection, Version, WorkingFile,
};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::sync::Arc;

const FILES_WIDTH: Pixels = px(320.);
const FORM_WIDTH: Pixels = px(400.);
const SUBJECT_LIMIT: usize = 72;
/// Drawing every line of a huge change would stall the screen, and a hunk that long gets
/// ticked as a whole anyway.
const MAX_HUNK_LINES: usize = 200;

#[derive(Default)]
pub(super) struct State {
    files: Arc<Vec<WorkingFile>>,
    selection: Selection,
    file: usize,
    kind: usize,
    breaking: bool,
    /// The widths the user dragged the file list and the form to.
    files_width: Option<f32>,
    form_width: Option<f32>,
    /// Scroll of the file list, which draws only the rows in view.
    files_scroll: UniformListScrollHandle,
    subject: Option<Entity<InputState>>,
    details: Option<Entity<TextareaState>>,
    committed: Option<String>,
    needs_clearing: bool,
    _inputs: Vec<Subscription>,
}

impl GraphView {
    pub fn open_commit_screen(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        self.commit.committed = None;
        self.show_screen(Screen::Commit, cx);
        Self::spawn_blocking(
            move || working::read(&repo),
            |this, files, cx| match files {
                Ok(files) => {
                    this.commit.selection = Selection::starting(&files);
                    this.commit.files = Arc::new(files);
                    this.commit.file = 0;
                    cx.notify();
                }
                Err(message) => {
                    this.notice = Some(Notice::Error(message.into()));
                    this.show_screen(Screen::Graph, cx);
                }
            },
            cx,
        );
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit.subject.is_some() {
            return;
        }
        let subject =
            cx.new(|cx| InputState::new(window, cx).placeholder("What does this commit do?"));
        let details = cx.new(|cx| TextareaState::new(window, cx).placeholder("Why this change?"));
        let notify = cx.subscribe(&subject, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        self.commit._inputs = vec![notify];
        self.commit.subject = Some(subject);
        self.commit.details = Some(details);
    }

    fn subject_text(&self, cx: &App) -> String {
        self.commit
            .subject
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string())
    }

    fn write_commit(&mut self, cx: &mut Context<Self>) {
        let included = self.commit.selection.included(&self.commit.files);
        let subject = self.subject_text(cx);
        if !working::can_commit(&included, &subject) {
            return;
        }
        let details = self
            .commit
            .details
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string());
        let kind = COMMIT_TYPES[self.commit.kind];
        let message = working::message(kind, self.commit.breaking, &subject, &details);
        let line = working::subject_line(kind, self.commit.breaking, &subject);
        self.commit.needs_clearing = true;
        self.run_commit(included, message, line, cx);
    }

    /// Empties the form once a commit has landed. Until then the message stays, so that a
    /// failing hook doesn't cost anyone what they wrote.
    fn clear_form_after_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.commit.needs_clearing {
            return;
        }
        self.commit.needs_clearing = false;
        if matches!(self.notice, Some(Notice::Error(_))) {
            self.commit.committed = None;
            return;
        }
        if let Some(input) = self.commit.subject.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Some(input) = self.commit.details.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn run_commit(
        &mut self,
        included: Vec<Included>,
        message: String,
        line: String,
        cx: &mut Context<Self>,
    ) {
        self.commit.committed = Some(line);
        self.run_operation(
            "Committing",
            move |repo| working::commit(repo, &included, &message).map(|_| None),
            cx,
        );
    }

    pub(super) fn render_commit_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_inputs(window, cx);
        self.clear_form_after_commit(window, cx);
        let files = self.render_changed_files(cx);
        let hunks = self.render_hunks(cx);
        let form = self.render_form(cx);

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .child(self.render_commit_header(cx))
            .child(self.resizable_columns(files, hunks, form, cx))
            .into_any_element()
    }

    fn render_commit_header(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let head = theme.primary;
        let subtext = tokens(theme.is_dark()).subtext0;
        let branch = self.refs.head_name().to_owned();
        let sentence = match (&self.commit.committed, self.commit.files.is_empty()) {
            (Some(line), _) => format!("Committed “{line}”"),
            (None, true) => "Nothing has changed".to_owned(),
            (None, false) => self.commit.selection.sentence(),
        };

        header_bar(cx)
            .pr_4()
            .child(
                Button::new("back-to-graph")
                    .ghost()
                    .label("Graph")
                    .on_click(cx.listener(|this, _, _, cx| this.show_screen(Screen::Graph, cx))),
            )
            .child(header_divider(cx))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .text_size(TEXT_STATUS)
                    .text_color(subtext)
                    .child(div().min_w_0().truncate().child(sentence))
                    .child("on")
                    .child(
                        div()
                            .flex_none()
                            .font_semibold()
                            .text_color(head)
                            .child(branch),
                    ),
            )
    }

    /// The file list, the hunks and the form, with the two outer edges draggable.
    fn resizable_columns(
        &self,
        files: Stateful<Div>,
        hunks: impl IntoElement,
        form: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Div {
        const HUNKS_MIN: f32 = 320.;
        let row = div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .child(files)
            .child(grip(Edge::Files, true, cx))
            .child(hunks)
            .child(grip(Edge::Form, true, cx))
            .child(form);
        let row = on_resize(row, Edge::Files, cx, |this, pointer, bounds| {
            let other = this.commit.form_width.unwrap_or(f32::from(FORM_WIDTH));
            let max = f32::from(bounds.size.width) - other - HUNKS_MIN;
            this.commit.files_width = Some(fit(pointer.x - bounds.left(), 200., max));
        });
        on_resize(row, Edge::Form, cx, |this, pointer, bounds| {
            let other = this.commit.files_width.unwrap_or(f32::from(FILES_WIDTH));
            let max = f32::from(bounds.size.width) - other - HUNKS_MIN;
            this.commit.form_width = Some(fit(bounds.right() - pointer.x, 280., max));
        })
    }

    fn render_changed_files(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let total = self.commit.files.len();
        div()
            .id("changed-files")
            .v_flex()
            .w(self.commit.files_width.map_or(FILES_WIDTH, px))
            .flex_none()
            .px_2()
            .py_3()
            .gap(px(2.))
            .bg(panel)
            .border_r_1()
            .border_color(border)
            .child(
                section_label(muted)
                    .h_flex()
                    .h(px(24.))
                    .px_2()
                    .child("Tick what goes in this commit"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "changed-file-rows",
                            total,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .filter(|index| *index < this.commit.files.len())
                                    .map(|index| this.render_changed_file(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.commit.files_scroll)
                        .size_full(),
                    )
                    .scrollbar(&self.commit.files_scroll, ScrollbarAxis::Vertical),
            )
    }

    fn render_changed_file(&self, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let muted = cx.theme().muted_foreground;
        let file = &self.commit.files[index];
        let status = file.patch.status();
        let (all, some) = self.commit.selection.file_state(index);
        let shown = self.commit.file == index;

        panel_row(("changed-file", index), shown, cx)
            .h(px(30.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.commit.file = index;
                cx.notify();
            }))
            .child(
                checkbox(("tick-file", index), all, some, cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.commit.selection.toggle_file(index);
                        cx.notify();
                    },
                )),
            )
            .child(file_path(&file.path, muted))
            .child(
                div()
                    .flex_none()
                    .text_size(TEXT_CHIP)
                    .text_color(status_color(status, cx))
                    .child(status.to_string()),
            )
    }

    fn render_hunks(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let total = self.commit.files.len();
        let shown_first = self.commit.file.min(total.saturating_sub(1));
        let order = (0..total).filter(|file| *file != shown_first);
        let order = std::iter::once(shown_first)
            .filter(|_| total > 0)
            .chain(order);

        let mut cards = Vec::new();
        for file in order.collect::<Vec<_>>() {
            for hunk in 0..self.commit.selection.hunks(file) {
                cards.push(self.render_hunk(file, hunk, cx));
            }
        }
        div()
            .id("hunks")
            .v_flex()
            .flex_1()
            .min_w_0()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .children(cards)
    }

    fn render_hunk(&self, file: usize, hunk: usize, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let subtext = tokens(theme.is_dark()).subtext0;
        let working = &self.commit.files[file];
        let on = self.commit.selection.is_on(file, hunk);
        let part = working.patch.hunks.get(hunk);
        let title = match part {
            Some(part) if !part.title.is_empty() => format!("{}  {}", working.path, part.title),
            _ => working.path.clone(),
        };

        div()
            .v_flex()
            .flex_none()
            .rounded(px(8.))
            .overflow_hidden()
            .bg(panel)
            .border_1()
            .border_color(border)
            .when(!on, |card| card.opacity(0.62))
            .child(
                div()
                    .h_flex()
                    .h(px(34.))
                    .px_3()
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        checkbox(("tick-hunk", file * 10_000 + hunk), on, false, cx).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.commit.selection.toggle_hunk(file, hunk);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_SMALL)
                            .text_color(subtext)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(if on { "in this commit" } else { "left out" }),
                    ),
            )
            .child(match part {
                Some(part) => render_hunk_lines(part, cx).into_any_element(),
                None => div()
                    .p_3()
                    .text_size(TEXT_SMALL)
                    .text_color(muted)
                    .child("This file goes in whole, because it has no text to choose from.")
                    .into_any_element(),
            })
    }

    fn render_form(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let (background, primary, on_primary) =
            (theme.background, theme.primary, theme.primary_foreground);
        let (quiet, danger) = (theme.secondary_foreground, theme.danger);
        let subtext = tokens(theme.is_dark()).subtext0;

        let kind = COMMIT_TYPES[self.commit.kind];
        let breaking = self.commit.breaking;
        let subject = self.subject_text(cx);
        let line = working::subject_line(kind, breaking, &subject);
        let length = line.chars().count();
        let included = self.commit.selection.included(&self.commit.files);
        let files = included.len();
        let cannot = !working::can_commit(&included, &subject) || self.busy.is_some();
        let label = match files {
            0 => "Tick a file to commit".to_owned(),
            files => format!("Commit {}", count(files, "file")),
        };
        let field_label = |text: &'static str| section_label(muted).child(text);

        let types = COMMIT_TYPES.iter().enumerate().map(|(index, name)| {
            let on = self.commit.kind == index;
            div()
                .id(("type", index))
                .h_flex()
                .h(px(26.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(TEXT_SMALL)
                .bg(if on { primary } else { background })
                .text_color(if on { on_primary } else { quiet })
                .child(*name)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.commit.kind = index;
                    cx.notify();
                }))
        });
        let types: Vec<_> = types.collect();

        let bump = self
            .refs
            .latest_version()
            .map(|version| (version, working::bump(kind, breaking, version)));
        let breaking_box = checkbox("breaking", self.commit.breaking, false, cx).on_click(
            cx.listener(|this, _, _, cx| {
                this.commit.breaking = !this.commit.breaking;
                cx.notify();
            }),
        );

        let mut form = div()
            .id("commit-form")
            .v_flex()
            .w(self.commit.form_width.map_or(FORM_WIDTH, px))
            .flex_none()
            .p_4()
            .gap_4()
            .bg(panel)
            .border_l_1()
            .border_color(border)
            .overflow_y_scroll()
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(field_label("Type of change"))
                    .child(div().flex().flex_wrap().gap_1().children(types)),
            );
        if let Some(input) = &self.commit.subject {
            form = form.child(
                div()
                    .v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .child(field_label("Summary"))
                            .child(
                                div()
                                    .text_size(TEXT_CHIP)
                                    .text_color(if length > SUBJECT_LIMIT {
                                        danger
                                    } else {
                                        muted
                                    })
                                    .child(format!("{length} / {SUBJECT_LIMIT}")),
                            ),
                    )
                    .child(Input::new(input)),
            );
        }
        if let Some(input) = &self.commit.details {
            form = form.child(
                div()
                    .v_flex()
                    .gap_1p5()
                    .child(field_label("Details (optional)"))
                    .child(div().h(px(92.)).child(Textarea::new(input).h_full())),
            );
        }
        form.when(working::can_break(kind), |form| {
            form.child(
                div()
                    .h_flex()
                    .gap_2()
                    .text_color(quiet)
                    .child(breaking_box)
                    .child("Breaking change"),
            )
        })
        .when_some(bump, |form, (version, bump)| {
            form.child(render_bump(version, bump, cx))
        })
        .child(
            div()
                .v_flex()
                .mt_auto()
                .gap_2()
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .text_color(subtext)
                        .child(line.clone()),
                )
                .when_some(
                    self.commit
                        .committed
                        .clone()
                        .filter(|_| self.busy.is_none()),
                    |column, _| {
                        column.child(
                            Button::new("committed")
                                .ghost()
                                .label("Committed. Back to the graph")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_screen(Screen::Graph, cx);
                                })),
                        )
                    },
                )
                .when(
                    self.commit.committed.is_none() || self.busy.is_some(),
                    |column| {
                        column.child(
                            Button::new("commit")
                                .primary()
                                .label(label)
                                .disabled(cannot)
                                .on_click(cx.listener(|this, _, _, cx| this.write_commit(cx))),
                        )
                    },
                ),
        )
    }
}

fn render_bump(version: Version, bump: Bump, cx: &App) -> Div {
    let theme = cx.theme();
    let (background, border, muted) = (theme.background, theme.border, theme.muted_foreground);
    let (success, quiet) = (theme.success, theme.secondary_foreground);
    let (name, next) = match bump {
        Bump::None => ("No release", None),
        Bump::Patch(next) => ("Patch release", Some(next)),
        Bump::Minor(next) => ("Minor release", Some(next)),
        Bump::Major(next) => ("Major release", Some(next)),
        Bump::BreakingMinor(next) => ("Minor release, breaking", Some(next)),
    };
    let (change, color) = match next {
        Some(next) => (format!("{version} → {next}"), success),
        None => (format!("stays {version}"), quiet),
    };
    div()
        .v_flex()
        .p_3()
        .gap_1()
        .rounded(px(8.))
        .bg(background)
        .border_1()
        .border_color(border)
        .child(section_label(muted).child("When this reaches main"))
        .child(
            div()
                .h_flex()
                .justify_between()
                .gap_2()
                .child(div().font_semibold().text_color(color).child(name))
                .child(div().text_size(TEXT_SMALL).text_color(quiet).child(change)),
        )
}

fn render_hunk_lines(hunk: &Hunk, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let colors = tokens(theme.is_dark());
    let hidden = hunk.lines.len().saturating_sub(MAX_HUNK_LINES);
    let rows = hunk.lines.iter().take(MAX_HUNK_LINES).map(|line| {
        let LineStyle {
            wash,
            sign,
            sign_color,
            text,
        } = line_style(line.kind, cx);
        div()
            .h_flex()
            .h(px(22.))
            .whitespace_nowrap()
            .when_some(wash, |row, wash| row.bg(wash))
            .child(
                div()
                    .w(px(36.))
                    .flex_none()
                    .text_right()
                    .text_color(colors.overlay1)
                    .child(
                        line.new
                            .or(line.old)
                            .map(|n| n.to_string())
                            .unwrap_or_default(),
                    ),
            )
            .child(
                div()
                    .w(px(28.))
                    .flex_none()
                    .text_center()
                    .text_color(sign_color)
                    .child(sign),
            )
            .child(div().text_color(text).child(expand_tabs(&line.text)))
    });
    div()
        .v_flex()
        .py(px(6.))
        .overflow_hidden()
        .text_size(TEXT_SMALL)
        .children(rows)
        .when(hidden > 0, |lines| {
            lines.child(
                div()
                    .px_3()
                    .pt_1()
                    .text_color(muted)
                    .child(format!("{hidden} more lines in this change")),
            )
        })
}
