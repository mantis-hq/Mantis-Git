//! The form at the foot of the inspector that turns what is staged into a commit.

use crate::theme::tokens;
use crate::view::parts::{TEXT_CHIP, TEXT_SMALL, checkbox, section_label};
use crate::view::{GraphView, Notice};
use gitclient_core::history::columns::Column;
use gitclient_core::menu::command::Command;
use gitclient_core::repo::stash;
use gitclient_core::settings::state::{self, AppState};
use gitclient_core::tree::FileView;
use gitclient_core::working;
use gitclient_core::working::message::{self, COMMIT_TYPES};
use gitclient_core::working::version::{self, Bump, Version};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

const SUBJECT_LIMIT: usize = 72;

#[derive(Default)]
pub(in crate::view) struct State {
    kind: usize,
    breaking: bool,
    /// How the files are listed, which every list in the inspector shares.
    pub(in crate::view) files_view: FileView,
    /// Where the way of listing is remembered for every repository. `None` in tests.
    state_file: Option<PathBuf>,
    subject: Option<Entity<InputState>>,
    details: Option<Entity<TextareaState>>,
    needs_clearing: bool,
    /// Whether the form is folded down to its heading, to give the lists the room.
    collapsed: bool,
    _inputs: Vec<Subscription>,
}

impl GraphView {
    /// Sets the file where the way of listing files is remembered.
    pub(in crate::view) fn set_state_file(&mut self, file: Option<PathBuf>) {
        if let Some(file) = &file {
            self.columns = AppState::load(file).graph_columns;
        }
        self.commit.state_file = file;
    }

    /// Puts `column` where `target` stands and remembers the order for every repository.
    /// The file's other settings are left as they are.
    pub(in crate::view) fn move_column(
        &mut self,
        column: Column,
        target: Column,
        cx: &mut Context<Self>,
    ) {
        self.columns.move_to(column, target);
        if let Some(file) = &self.commit.state_file {
            let order = self.columns;
            if let Err(error) = AppState::change(file, |state| state.graph_columns = order) {
                self.notice = Some(Notice::Error(error.into()));
            }
        }
        cx.notify();
    }

    /// The file where the app's own settings are remembered, if there is one.
    pub(in crate::view) fn state_file(&self) -> Option<&std::path::Path> {
        self.commit.state_file.as_deref()
    }

    /// How many commits the graph loads, as the app settings say.
    pub(in crate::view) fn commit_limit(&self) -> u32 {
        self.commit
            .state_file
            .as_deref()
            .map_or(state::DEFAULT_COMMIT_LIMIT, |file| {
                AppState::load(file).commit_limit
            })
    }

    /// Whether the app settings let it fetch by itself. On when there is no settings file.
    pub(in crate::view) fn auto_fetch_on(&self) -> bool {
        self.commit
            .state_file
            .as_deref()
            .is_none_or(|file| AppState::load(file).auto_fetch)
    }

    /// Lists the files in `view` and remembers it for every repository. The file's other
    /// settings are left as they are.
    pub(in crate::view) fn set_files_view(&mut self, view: FileView, cx: &mut Context<Self>) {
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
    pub(in crate::view) fn refresh_files_view(&mut self) {
        if let Some(file) = &self.commit.state_file {
            self.commit.files_view = AppState::load(file).files_view;
        }
    }

    /// Makes the form's inputs the first time they are needed.
    pub(in crate::view) fn ensure_commit_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if !message::can_commit(self.staged_count(), &subject) || self.busy.is_some() {
            return;
        }
        let details = self
            .commit
            .details
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string());
        let kind = COMMIT_TYPES[self.commit.kind];
        let message = message::message(kind, self.commit.breaking, &subject, &details);
        let line = message::subject_line(kind, self.commit.breaking, &subject);
        self.commit.needs_clearing = true;
        let notice = message::committed_notice(&line);
        self.run_operation(
            "Committing",
            move |repo| working::commit(repo, &message).map(|_| Some(notice)),
            cx,
        );
    }

    /// Sets what is staged aside. A typed summary names the stash.
    fn stash_staged(&mut self, cx: &mut Context<Self>) {
        let staged = self.staged_count();
        if staged == 0 || self.busy.is_some() {
            return;
        }
        let message = Some(self.subject_text(cx)).filter(|text| !text.trim().is_empty());
        self.run_command(Command::StashStaged { staged, message }, cx);
    }

    /// Empties the form once a commit has landed. Until then the message stays, so that a
    /// failing hook doesn't cost anyone what they wrote.
    pub(in crate::view) fn clear_form_after_commit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(in crate::view) fn render_commit_form(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (background, primary, on_primary) =
            (theme.background, theme.primary, theme.primary_foreground);
        let (quiet, danger) = (theme.secondary_foreground, theme.danger);
        let subtext = tokens(theme.is_dark()).subtext0;

        let kind = COMMIT_TYPES[self.commit.kind];
        let breaking = self.commit.breaking;
        let subject = self.subject_text(cx);
        let line = message::subject_line(kind, breaking, &subject);
        let length = line.chars().count();
        let staged = self.staged_count();
        let cannot = !message::can_commit(staged, &subject) || self.busy.is_some();
        let collapsed = self.commit.collapsed;

        // The kinds of change in one line, wrapping only if the panel is dragged narrow.
        let types: Vec<_> = COMMIT_TYPES
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let on = self.commit.kind == index;
                div()
                    .id(("type", index))
                    .h_flex()
                    .h(px(24.))
                    .px(px(6.))
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
            .map(|version| (version, version::bump(kind, breaking, version)));
        let breaking_box =
            checkbox("breaking", breaking, false, cx).on_click(cx.listener(|this, _, _, cx| {
                this.commit.breaking = !this.commit.breaking;
                cx.notify();
            }));

        // The heading holds the button, so that committing never needs a scroll. While the
        // form is folded away there is no message to commit, so the button goes too.
        let heading = div()
            .h_flex()
            .h(px(32.))
            .gap_2()
            .child(
                div()
                    .id("commit-form-toggle")
                    .h_flex()
                    .flex_1()
                    .h_full()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.commit.collapsed = !this.commit.collapsed;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(if collapsed { "▸" } else { "▾" }),
                    )
                    .child(section_label(muted).child("Commit"))
                    .when(collapsed, |row| {
                        row.child(
                            div()
                                .text_size(TEXT_CHIP)
                                .text_color(muted)
                                .child(format!("{staged} staged")),
                        )
                    }),
            )
            .when(!collapsed, |row| {
                row.child(
                    Button::new("stash-staged")
                        .ghost()
                        .label(stash::push_label(staged))
                        .disabled(staged == 0 || self.busy.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.stash_staged(cx))),
                )
                .child(
                    Button::new("commit")
                        .primary()
                        .label(message::commit_label(staged))
                        .disabled(cannot)
                        .on_click(cx.listener(|this, _, _, cx| this.write_commit(cx))),
                )
            });

        let mut body = div()
            .v_flex()
            .flex_none()
            .gap_2()
            .child(div().flex().flex_wrap().gap(px(2.)).children(types));
        if let Some(input) = &self.commit.subject {
            body = body.child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        // The line as it will be written, and how long it is.
                        div()
                            .h_flex()
                            .justify_between()
                            .gap_2()
                            .text_size(TEXT_CHIP)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(subtext)
                                    .child(line.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
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
            body = body.child(div().h(px(52.)).child(Textarea::new(input).h_full()));
        }
        let body = body
            .when(message::can_break(kind), |body| {
                body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .text_color(quiet)
                        .child(breaking_box)
                        .child("Breaking change"),
                )
            })
            .when_some(bump, |body, (version, bump)| {
                body.child(render_bump(version, bump, cx))
            });

        div()
            .v_flex()
            .gap_2()
            .child(heading)
            .when(!collapsed, |root| root.child(body))
    }
}

/// What landing this commit on main would do to the version, in one line.
fn render_bump(version: Version, bump: Bump, cx: &App) -> Div {
    let theme = cx.theme();
    let (background, border) = (theme.background, theme.border);
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
        .h_flex()
        .h(px(30.))
        .px_3()
        .justify_between()
        .gap_2()
        .rounded(px(8.))
        .bg(background)
        .border_1()
        .border_color(border)
        .text_size(TEXT_SMALL)
        .child(div().font_semibold().text_color(color).child(name))
        .child(div().text_color(quiet).child(change))
}
