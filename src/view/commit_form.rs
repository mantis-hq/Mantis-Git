//! The form at the foot of the inspector that turns what is staged into a commit.

use super::parts::{TEXT_CHIP, TEXT_SMALL, checkbox, section_label};
use super::{GraphView, Notice};
use crate::theme::tokens;
use gitclient_core::state::AppState;
use gitclient_core::tree::FileView;
use gitclient_core::working::{self, Bump, COMMIT_TYPES, Version};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

const SUBJECT_LIMIT: usize = 72;

#[derive(Default)]
pub(super) struct State {
    kind: usize,
    breaking: bool,
    /// How the files are listed, which every list in the inspector shares.
    pub(super) files_view: FileView,
    /// Where the way of listing is remembered for every repository. `None` in tests.
    state_file: Option<PathBuf>,
    subject: Option<Entity<InputState>>,
    details: Option<Entity<TextareaState>>,
    needs_clearing: bool,
    _inputs: Vec<Subscription>,
}

impl GraphView {
    /// Sets the file where the way of listing files is remembered.
    pub(super) fn set_state_file(&mut self, file: Option<PathBuf>) {
        self.commit.state_file = file;
    }

    /// Lists the files in `view` and remembers it for every repository. The file's other
    /// settings are left as they are.
    pub(super) fn set_files_view(&mut self, view: FileView, cx: &mut Context<Self>) {
        self.commit.files_view = view;
        self.reset_file_scrolls();
        if let Some(file) = &self.commit.state_file {
            let mut state = AppState::load(file);
            state.files_view = view;
            if let Err(error) = state.save(file) {
                self.notice = Some(Notice::Error(error.into()));
            }
        }
        cx.notify();
    }

    /// Reads the way of listing files again, since another tab may have changed it.
    pub(super) fn refresh_files_view(&mut self) {
        if let Some(file) = &self.commit.state_file {
            self.commit.files_view = AppState::load(file).files_view;
        }
    }

    /// Makes the form's inputs the first time they are needed.
    pub(super) fn ensure_commit_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let subject = self.subject_text(cx);
        if !working::can_commit(self.staged_count(), &subject) || self.busy.is_some() {
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
        let notice = working::committed_notice(&line);
        self.run_operation(
            "Committing",
            move |repo| working::commit(repo, &message).map(|_| Some(notice)),
            cx,
        );
    }

    /// Empties the form once a commit has landed. Until then the message stays, so that a
    /// failing hook doesn't cost anyone what they wrote.
    pub(super) fn clear_form_after_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || !self.commit.needs_clearing {
            return;
        }
        self.commit.needs_clearing = false;
        if matches!(self.notice, Some(Notice::Error(_))) {
            return;
        }
        if let Some(input) = self.commit.subject.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Some(input) = self.commit.details.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(super) fn render_commit_form(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (background, primary, on_primary) =
            (theme.background, theme.primary, theme.primary_foreground);
        let (quiet, danger) = (theme.secondary_foreground, theme.danger);
        let subtext = tokens(theme.is_dark()).subtext0;

        let kind = COMMIT_TYPES[self.commit.kind];
        let breaking = self.commit.breaking;
        let subject = self.subject_text(cx);
        let line = working::subject_line(kind, breaking, &subject);
        let length = line.chars().count();
        let staged = self.staged_count();
        let cannot = !working::can_commit(staged, &subject) || self.busy.is_some();
        let field_label = |text: &'static str| section_label(muted).child(text);

        let types: Vec<_> = COMMIT_TYPES
            .iter()
            .enumerate()
            .map(|(index, name)| {
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
            })
            .collect();

        let bump = self
            .refs
            .latest_version()
            .map(|version| (version, working::bump(kind, breaking, version)));
        let breaking_box =
            checkbox("breaking", breaking, false, cx).on_click(cx.listener(|this, _, _, cx| {
                this.commit.breaking = !this.commit.breaking;
                cx.notify();
            }));

        let mut form = div().v_flex().flex_none().gap_3().child(
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
                    .child(div().h(px(72.)).child(Textarea::new(input).h_full())),
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
                .text_size(TEXT_SMALL)
                .text_color(subtext)
                .child(line.clone()),
        )
        .child(
            Button::new("commit")
                .primary()
                .label(working::commit_label(staged))
                .disabled(cannot)
                .on_click(cx.listener(|this, _, _, cx| this.write_commit(cx))),
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
