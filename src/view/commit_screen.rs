//! The task screen for writing a commit: choose hunks, say what they do.

use super::{GraphView, Notice, Screen};
use crate::patch::{Hunk, LineKind};
use crate::theme::tokens;
use crate::working::{self, Bump, COMMIT_TYPES, Included, Version, WorkingFile};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
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
    /// By file, then by hunk: whether it goes in. A file without hunks has one entry.
    included: Vec<Vec<bool>>,
    file: usize,
    kind: usize,
    breaking: bool,
    subject: Option<Entity<InputState>>,
    details: Option<Entity<TextareaState>>,
    committed: Option<String>,
    needs_clearing: bool,
    _inputs: Vec<Subscription>,
}

/// What ticking a file does to its hunks: all on, unless all were on already.
fn toggled(hunks: &[bool]) -> Vec<bool> {
    let all_on = hunks.iter().all(|on| *on);
    vec![!all_on; hunks.len()]
}

/// The starting ticks. If anything was staged, that choice carries over by file.
fn starting_ticks(files: &[WorkingFile]) -> Vec<Vec<bool>> {
    let any_staged = files.iter().any(|file| file.staged);
    files
        .iter()
        .map(|file| vec![!any_staged || file.staged; file.patch.hunks.len().max(1)])
        .collect()
}

/// Turns the ticks into what `working::commit` wants.
fn chosen(files: &[WorkingFile], ticks: &[Vec<bool>]) -> Vec<Included> {
    files
        .iter()
        .zip(ticks)
        .filter(|(_, hunks)| hunks.iter().any(|on| *on))
        .map(|(file, hunks)| Included {
            file: file.clone(),
            hunks: (0..file.patch.hunks.len())
                .filter(|&hunk| hunks[hunk])
                .collect(),
        })
        .collect()
}

impl GraphView {
    pub fn open_commit_screen(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        self.commit.committed = None;
        self.show_screen(Screen::Commit, cx);
        cx.spawn(async move |this, cx| {
            let files = cx
                .background_spawn(async move { working::read(&repo) })
                .await;
            this.update(cx, |this, cx| match files {
                Ok(files) => {
                    this.commit.included = starting_ticks(&files);
                    this.commit.files = Arc::new(files);
                    this.commit.file = 0;
                    cx.notify();
                }
                Err(message) => {
                    this.notice = Some(Notice::Error(message.into()));
                    this.show_screen(Screen::Graph, cx);
                }
            })
            .ok();
        })
        .detach();
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
        let included = chosen(&self.commit.files, &self.commit.included);
        let subject = self.subject_text(cx);
        if included.is_empty() || subject.trim().is_empty() {
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

    fn file_ticks(&self, file: usize) -> (bool, bool) {
        let hunks = &self.commit.included[file];
        let on = hunks.iter().filter(|on| **on).count();
        (on == hunks.len(), on > 0 && on < hunks.len())
    }

    /// Says how the commit stands, in plain words.
    fn commit_sentence(&self) -> String {
        let total = self.commit.files.len();
        let going_in = (0..total)
            .filter(|&file| self.commit.included[file].iter().any(|on| *on))
            .count();
        let left_out: usize = self
            .commit
            .included
            .iter()
            .map(|hunks| hunks.iter().filter(|on| !**on).count())
            .sum();
        let changes = if left_out == 1 { "change" } else { "changes" };
        let files = if total == 1 { "file" } else { "files" };
        if going_in == 0 {
            format!("{total} {files} changed, nothing ticked yet")
        } else {
            format!("{going_in} of {total} {files} going in, {left_out} {changes} left out")
        }
    }

    fn latest_version(&self) -> Option<Version> {
        let tags = self.refs.by_commit.values().flatten();
        working::latest_version(
            tags.filter(|r| r.kind == crate::git::RefKind::Tag)
                .map(|r| r.name.as_str()),
        )
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
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(files)
                    .child(hunks)
                    .child(form),
            )
            .into_any_element()
    }

    fn render_commit_header(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, header, head) = (theme.border, theme.title_bar, theme.primary);
        let subtext = tokens(theme.is_dark()).subtext0;
        let branch = self
            .refs
            .head_branch
            .clone()
            .unwrap_or_else(|| "HEAD".into());
        let sentence = match (&self.commit.committed, self.commit.files.is_empty()) {
            (Some(line), _) => format!("Committed “{line}”"),
            (None, true) => "Nothing has changed".to_owned(),
            (None, false) => self.commit_sentence(),
        };

        div()
            .id("commit-header")
            .h_flex()
            .h(px(52.))
            .flex_none()
            .px_4()
            .gap_4()
            .bg(header)
            .border_b_1()
            .border_color(border)
            .child(
                Button::new("back-to-graph")
                    .ghost()
                    .label("Graph")
                    .on_click(cx.listener(|this, _, _, cx| this.show_screen(Screen::Graph, cx))),
            )
            .child(div().flex_none().w(px(1.)).h(px(20.)).bg(border))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .text_size(px(15.))
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

    fn render_changed_files(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let mut rows = Vec::with_capacity(self.commit.files.len());
        for index in 0..self.commit.files.len() {
            rows.push(self.render_changed_file(index, cx));
        }
        div()
            .id("changed-files")
            .v_flex()
            .w(FILES_WIDTH)
            .flex_none()
            .px_2()
            .py_3()
            .gap(px(2.))
            .bg(panel)
            .border_r_1()
            .border_color(border)
            .overflow_y_scroll()
            .child(
                div()
                    .h_flex()
                    .h(px(24.))
                    .px_2()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(muted)
                    .child("Tick what goes in this commit"),
            )
            .children(rows)
    }

    fn render_changed_file(&self, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let active = crate::theme::selected_row(theme);
        let file = &self.commit.files[index];
        let status_color = match file.patch.status() {
            'A' => theme.success,
            'D' => theme.danger,
            _ => theme.warning,
        };
        let split = file.path.rfind('/').map_or(0, |slash| slash + 1);
        let (folder, name) = file.path.split_at(split);
        let (all, some) = self.file_ticks(index);
        let shown = self.commit.file == index;

        div()
            .id(("changed-file", index))
            .h_flex()
            .h(px(30.))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .when(shown, |row| row.bg(active))
            .when(!shown, |row| row.hover(|style| style.bg(hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.commit.file = index;
                cx.notify();
            }))
            .child(
                checkbox(("tick-file", index), all, some, cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        let hunks = &mut this.commit.included[index];
                        *hunks = toggled(hunks);
                        cx.notify();
                    },
                )),
            )
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(div().flex_none().text_color(muted).child(folder.to_owned()))
                    .child(div().min_w_0().truncate().child(name.to_owned())),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(status_color)
                    .child(file.patch.status().to_string()),
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
            let count = self.commit.included[file].len();
            for hunk in 0..count {
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
        let on = self.commit.included[file][hunk];
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
                                let tick = &mut this.commit.included[file][hunk];
                                *tick = !*tick;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(subtext)
                            .child(title),
                    )
                    .child(div().flex_none().text_xs().text_color(muted).child(if on {
                        "in this commit"
                    } else {
                        "left out"
                    })),
            )
            .child(match part {
                Some(part) => render_hunk_lines(part, cx).into_any_element(),
                None => div()
                    .p_3()
                    .text_size(px(13.))
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
        let breaking = self.commit.breaking && kind == "feat";
        let subject = self.subject_text(cx);
        let line = working::subject_line(kind, breaking, &subject);
        let length = line.chars().count();
        let included = chosen(&self.commit.files, &self.commit.included);
        let files = included.len();
        let cannot = files == 0 || subject.trim().is_empty() || self.busy.is_some();
        let label = match files {
            0 => "Tick a file to commit".to_owned(),
            1 => "Commit 1 file".to_owned(),
            count => format!("Commit {count} files"),
        };
        let field_label = |text: &'static str| {
            div()
                .text_size(px(13.))
                .font_medium()
                .text_color(muted)
                .child(text)
        };

        let types = COMMIT_TYPES.iter().enumerate().map(|(index, name)| {
            let on = self.commit.kind == index;
            div()
                .id(("type", index))
                .h_flex()
                .h(px(26.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
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
            .w(FORM_WIDTH)
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
                                    .text_xs()
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
        form.when(kind == "feat", |form| {
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
                        .text_size(px(13.))
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
    let (name, change, color) = match bump {
        Bump::None => ("No release".to_owned(), format!("stays {version}"), quiet),
        Bump::Patch(next) => (
            "Patch release".to_owned(),
            format!("{version} → {next}"),
            success,
        ),
        Bump::Minor(next) => (
            "Minor release".to_owned(),
            format!("{version} → {next}"),
            success,
        ),
        Bump::Major(next) => (
            "Major release".to_owned(),
            format!("{version} → {next}"),
            success,
        ),
        Bump::BreakingMinor(next) => (
            "Minor release, breaking".to_owned(),
            format!("{version} → {next}"),
            success,
        ),
    };
    div()
        .v_flex()
        .p_3()
        .gap_1()
        .rounded(px(8.))
        .bg(background)
        .border_1()
        .border_color(border)
        .child(
            div()
                .text_size(px(13.))
                .font_medium()
                .text_color(muted)
                .child("When this reaches main"),
        )
        .child(
            div()
                .h_flex()
                .justify_between()
                .gap_2()
                .child(div().font_semibold().text_color(color).child(name))
                .child(div().text_size(px(13.)).text_color(quiet).child(change)),
        )
}

fn render_hunk_lines(hunk: &Hunk, cx: &App) -> Div {
    let theme = cx.theme();
    let (foreground, added, removed) = (theme.foreground, theme.success, theme.danger);
    let muted = theme.muted_foreground;
    let colors = tokens(theme.is_dark());
    let hidden = hunk.lines.len().saturating_sub(MAX_HUNK_LINES);
    let rows = hunk.lines.iter().take(MAX_HUNK_LINES).map(|line| {
        let (wash, sign, sign_color, text) = match line.kind {
            LineKind::Added => (Some(added), "+", added, foreground),
            LineKind::Removed => (Some(removed), "-", removed, foreground),
            LineKind::Context => (None, "", muted, colors.subtext0),
        };
        div()
            .h_flex()
            .h(px(22.))
            .whitespace_nowrap()
            .when_some(wash, |row, wash| row.bg(wash.opacity(0.10)))
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
            .child(
                div()
                    .text_color(text)
                    .child(line.text.replace('\t', "    ")),
            )
    });
    div()
        .v_flex()
        .py(px(6.))
        .overflow_hidden()
        .text_size(px(13.))
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

/// A 16 px box, with a tick or a dash so its state doesn't rest on colour alone.
fn checkbox(id: impl Into<ElementId>, on: bool, partly: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (primary, on_primary) = (theme.primary, theme.primary_foreground);
    let idle = tokens(theme.is_dark()).overlay1;
    let filled = on || partly;
    div()
        .id(id)
        .h_flex()
        .justify_center()
        .size(px(16.))
        .flex_none()
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(px(11.))
        .font_semibold()
        .text_color(on_primary)
        .when(filled, |tick| tick.bg(primary))
        .when(!filled, |tick| tick.border(px(1.5)).border_color(idle))
        .when(on, |tick| tick.child("✓"))
        .when(partly, |tick| tick.child("–"))
}

#[cfg(test)]
mod tests {
    use super::{chosen, starting_ticks, toggled};
    use crate::patch;
    use crate::working::WorkingFile;

    const TWO_HUNKS: &str = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
@@ -9 +9 @@
-c
+d
";

    fn file(path: &str, staged: bool) -> WorkingFile {
        let mut patch = patch::parse(TWO_HUNKS).remove(0);
        patch.path = path.to_owned();
        WorkingFile {
            path: path.to_owned(),
            patch,
            staged,
        }
    }

    #[test]
    fn everything_starts_ticked_when_nothing_is_staged() {
        let ticks = starting_ticks(&[file("a", false), file("b", false)]);
        assert_eq!(ticks, [[true, true], [true, true]]);
    }

    #[test]
    fn staged_files_carry_over_as_the_starting_choice() {
        let ticks = starting_ticks(&[file("a", true), file("b", false)]);
        assert_eq!(ticks, [[true, true], [false, false]]);
    }

    #[test]
    fn a_file_with_no_hunks_still_gets_one_tick() {
        let mut binary = file("a", false);
        binary.patch.hunks.clear();
        assert_eq!(starting_ticks(&[binary]), [[true]]);
    }

    #[test]
    fn ticking_a_partly_ticked_file_turns_it_all_on() {
        assert_eq!(toggled(&[true, false]), [true, true]);
        assert_eq!(toggled(&[true, true]), [false, false]);
        assert_eq!(toggled(&[false, false]), [true, true]);
    }

    #[test]
    fn only_ticked_hunks_are_chosen() {
        let files = [file("a", false), file("b", false)];
        let parts = chosen(&files, &[vec![false, true], vec![false, false]]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].hunks, [1]);
    }
}
