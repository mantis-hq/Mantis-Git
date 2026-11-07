//! The settings window: a side panel of pages, and for now one page, which edits the
//! user's git config without making them know its keys.
//!
//! There is no Save button. Toggles and choices apply when clicked, text applies when you
//! press Enter or leave the field, and the header says what happened.

use super::parts::{checkbox, header_bar, panel_row, section_label, title};
use crate::theme::{selected_row, tokens};
use gitclient_core::config::{self, Kind, Scope, Setting, Values};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const SIDEBAR_WIDTH: Pixels = px(220.);
const PAGE_WIDTH: Pixels = px(680.);
const INPUT_WIDTH: Pixels = px(260.);
const NAME_WIDTH: Pixels = px(140.);

actions!(settings, [OpenSettings]);

pub(super) fn bind_keys(cx: &mut App) {
    let key = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    cx.bind_keys([KeyBinding::new(key, OpenSettings, None)]);
    cx.on_action(|_: &OpenSettings, cx| open(cx));
}

struct SettingsWindow(AnyWindowHandle);

impl Global for SettingsWindow {}

/// Shows the settings window, bringing the one that is already open to the front.
pub fn open(cx: &mut App) {
    if let Some(SettingsWindow(window)) = cx.try_global::<SettingsWindow>() {
        let window = *window;
        if window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(960.), px(680.)), cx)),
        window_min_size: Some(size(px(640.), px(420.))),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        crate::theme::follow_system(window, cx);
        cx.new(|cx| Settings::new(Scope::Global, window, cx))
    });
    if let Ok((window, _)) = opened {
        cx.set_global(SettingsWindow(window));
    }
}

/// Whether the app should quit now that a window closed: it should unless a window
/// other than settings is still open, since settings is only ever an add-on.
pub fn should_quit(cx: &App) -> bool {
    let settings = cx
        .try_global::<SettingsWindow>()
        .map(|settings| settings.0.window_id());
    cx.windows()
        .iter()
        .all(|window| Some(window.window_id()) == settings)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    GitConfig,
}

impl Page {
    const ALL: [Page; 1] = [Page::GitConfig];

    fn name(self) -> &'static str {
        match self {
            Page::GitConfig => "Git config",
        }
    }
}

enum Feedback {
    None,
    Saved(String),
    Failed(String),
}

struct TextField {
    key: &'static str,
    input: Entity<InputState>,
    _events: Subscription,
}

struct AliasRow {
    name: String,
    input: Entity<InputState>,
    _events: Subscription,
}

pub struct Settings {
    scope: Scope,
    page: Page,
    /// What the file holds. `None` until it has been read.
    values: Option<Values>,
    load_error: Option<String>,
    fields: Vec<TextField>,
    aliases: Vec<AliasRow>,
    new_name: Entity<InputState>,
    new_command: Entity<InputState>,
    feedback: Feedback,
    _new_alias: Vec<Subscription>,
}

impl Settings {
    pub fn new(scope: Scope, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fields = Vec::new();
        for setting in config::sections().iter().flat_map(|s| s.settings) {
            if let Kind::Text { placeholder } = setting.kind {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                let key = setting.key;
                let events = cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        if commits(event) {
                            this.commit_text(key, window, cx);
                        }
                    },
                );
                fields.push(TextField {
                    key,
                    input,
                    _events: events,
                });
            }
        }

        let new_name = cx.new(|cx| InputState::new(window, cx).placeholder("co"));
        let new_command = cx.new(|cx| InputState::new(window, cx).placeholder("checkout"));
        let mut new_alias = Vec::new();
        for input in [&new_name, &new_command] {
            new_alias.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_alias(window, cx);
                    }
                    cx.notify();
                },
            ));
        }

        let mut this = Self {
            scope,
            page: Page::GitConfig,
            values: None,
            load_error: None,
            fields,
            aliases: Vec::new(),
            new_name,
            new_command,
            feedback: Feedback::None,
            _new_alias: new_alias,
        };
        this.load(window, cx);
        this
    }

    /// Runs `work` off the UI thread, then gives its result to `done`.
    fn blocking<T: Send + 'static>(
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            this.update_in(cx, |this, window, cx| done(this, result, window, cx))
                .ok();
        })
        .detach();
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self.scope.clone();
        Self::blocking(
            move || config::read(&scope),
            |this, result, window, cx| match result {
                Ok(values) => this.apply(values, window, cx),
                Err(message) => {
                    this.load_error = Some(message);
                    cx.notify();
                }
            },
            window,
            cx,
        );
    }

    /// Makes `change` to the file, then reads it back so the page shows what is true.
    fn change(
        &mut self,
        saved: String,
        change: impl FnOnce(&Scope) -> Result<(), String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = self.scope.clone();
        Self::blocking(
            move || change(&scope).and_then(|()| config::read(&scope)),
            move |this, result, window, cx| match result {
                Ok(values) => {
                    this.feedback = Feedback::Saved(saved);
                    this.apply(values, window, cx);
                }
                Err(message) => {
                    this.feedback = Feedback::Failed(message);
                    cx.notify();
                }
            },
            window,
            cx,
        );
    }

    /// Shows `values`. A field being typed in keeps what is in it.
    fn apply(&mut self, values: Values, window: &mut Window, cx: &mut Context<Self>) {
        for field in &self.fields {
            let shown = values.get(field.key).unwrap_or("").to_owned();
            sync(&field.input, shown, window, cx);
        }

        let wanted = values.aliases();
        self.aliases
            .retain(|row| wanted.iter().any(|alias| alias.name == row.name));
        for alias in wanted {
            match self.aliases.iter().find(|row| row.name == alias.name) {
                Some(row) => sync(&row.input, alias.command, window, cx),
                None => {
                    let command = alias.command;
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("What git runs")
                            .default_value(command)
                    });
                    let name = alias.name.clone();
                    let events = cx.subscribe_in(
                        &input,
                        window,
                        move |this, _, event: &InputEvent, window, cx| {
                            if commits(event) {
                                this.commit_alias(&name, window, cx);
                            }
                        },
                    );
                    self.aliases.push(AliasRow {
                        name: alias.name,
                        input,
                        _events: events,
                    });
                }
            }
        }
        self.aliases.sort_by(|a, b| a.name.cmp(&b.name));

        self.values = Some(values);
        self.load_error = None;
        cx.notify();
    }

    fn write(
        &mut self,
        key: &'static str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = config::saved_message(key, Some(value));
        let value = value.to_owned();
        self.change(
            saved,
            move |scope| config::set(scope, key, &value),
            window,
            cx,
        );
    }

    fn commit_text(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.fields.iter().find(|field| field.key == key) else {
            return;
        };
        let typed = field.input.read(cx).value().trim().to_owned();
        let current = self.values.as_ref().and_then(|values| values.get(key));
        if current.unwrap_or("") != typed {
            self.write(key, &typed, window, cx);
        }
    }

    fn commit_alias(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.aliases.iter().find(|row| row.name == name) else {
            return;
        };
        let typed = row.input.read(cx).value().trim().to_owned();
        let current = self.values.as_ref().and_then(|values| {
            values
                .aliases()
                .into_iter()
                .find(|alias| alias.name == name)
                .map(|alias| alias.command)
        });
        if current.as_deref() == Some(typed.as_str()) {
            return;
        }
        let name = name.to_owned();
        let saved = format!("Saved git {name} as {typed}.");
        self.change(
            saved,
            move |scope| config::set_alias(scope, &name, &typed),
            window,
            cx,
        );
    }

    fn add_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_owned();
        let command = self.new_command.read(cx).value().trim().to_owned();
        if let Some(problem) = config::alias_problem(&name) {
            self.feedback = Feedback::Failed(problem.to_owned());
            cx.notify();
            return;
        }
        let saved = format!("Added git {name}.");
        self.new_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.new_command
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.change(
            saved,
            move |scope| config::set_alias(scope, &name, &command),
            window,
            cx,
        );
    }

    fn remove_alias(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        let saved = format!("Removed git {name}.");
        self.change(
            saved,
            move |scope| config::remove_alias(scope, &name),
            window,
            cx,
        );
    }

    fn render_header(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (text, color) = match &self.feedback {
            Feedback::None => (
                format!(
                    "Editing your {}. Changes save as you make them.",
                    self.scope.name()
                ),
                tokens(theme.is_dark()).subtext0,
            ),
            Feedback::Saved(message) => (message.clone(), theme.success),
            Feedback::Failed(message) => (message.clone(), theme.danger),
        };
        header_bar(cx).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(15.))
                .text_color(color)
                .child(text),
        )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.list_head, theme.border, theme.muted_foreground);
        let pages = Page::ALL.into_iter().enumerate().map(|(index, page)| {
            panel_row(("page", index), self.page == page, cx)
                .h(px(32.))
                .text_size(px(14.))
                .child(page.name())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }))
        });
        div()
            .v_flex()
            .w(SIDEBAR_WIDTH)
            .flex_none()
            .p_2()
            .gap_1()
            .bg(panel)
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .child(section_label(muted).child("Settings")),
            )
            .children(pages)
    }

    fn render_git_config(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
        let subtext = tokens(cx.theme().is_dark()).subtext0;
        let mut page = div().v_flex().w_full().max_w(PAGE_WIDTH).gap_6().child(
            div()
                .v_flex()
                .gap_1()
                .child(title().child("Git config"))
                .child(
                    div()
                        .text_size(px(15.))
                        .line_height(px(20.))
                        .text_color(subtext)
                        .child("These apply to every repository on this computer."),
                ),
        );

        let Some(values) = self.values.clone() else {
            let (text, color) = match &self.load_error {
                Some(error) => (error.clone(), danger),
                None => ("Reading your git config…".to_owned(), muted),
            };
            return page.child(div().text_size(px(14.)).text_color(color).child(text));
        };

        for (index, section) in config::sections().iter().enumerate() {
            let rows: Vec<AnyElement> = section
                .settings
                .iter()
                .enumerate()
                .map(|(row, setting)| self.render_setting(index * 100 + row, setting, &values, cx))
                .collect();
            page = page.child(self.render_group(section.title, rows, cx));
        }
        page.child(self.render_aliases(cx))
    }

    /// A titled card holding rows, with a line between them.
    fn render_group(
        &self,
        name: &str,
        rows: Vec<AnyElement>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (panel, border, muted) = (theme.list_head, theme.border, theme.muted_foreground);
        let rows = rows.into_iter().enumerate().map(|(index, row)| {
            div()
                .when(index > 0, |row| row.border_t_1().border_color(border))
                .child(row)
        });
        div()
            .v_flex()
            .gap_2()
            .child(section_label(muted).child(name.to_owned()))
            .child(
                div()
                    .v_flex()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(border)
                    .bg(panel)
                    .overflow_hidden()
                    .children(rows),
            )
    }

    fn render_setting(
        &self,
        id: usize,
        setting: &Setting,
        values: &Values,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let setting = *setting;
        let muted = cx.theme().muted_foreground;
        let words = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .child(setting.label),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(muted)
                    .child(setting.hint),
            );
        let row = div().px_4().py_3().gap_3();

        match setting.kind {
            Kind::Text { .. } => {
                let input = self
                    .fields
                    .iter()
                    .find(|field| field.key == setting.key)
                    .map(|field| field.input.clone());
                row.h_flex()
                    .gap_4()
                    .child(words)
                    .children(
                        input.map(|input| {
                            div().w(INPUT_WIDTH).flex_none().child(Input::new(&input))
                        }),
                    )
                    .into_any_element()
            }
            Kind::Toggle { .. } => {
                let on = values.is_on(&setting);
                let key = setting.key;
                let reset = values.is_set(key).then(|| {
                    Button::new(("reset", id))
                        .ghost()
                        .compact()
                        .label("Use default")
                        .tooltip("Remove it from your config, so git decides")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.write(key, "", window, cx);
                        }))
                });
                row.h_flex()
                    .gap_4()
                    .child(words)
                    .children(reset)
                    .child(
                        checkbox(("toggle", id), on, false, cx).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.write(key, if on { "false" } else { "true" }, window, cx);
                            },
                        )),
                    )
                    .into_any_element()
            }
            Kind::Choice { options } => {
                let key = setting.key;
                let current = values.get(key);
                let unknown = current.filter(|value| !options.iter().any(|(v, _)| v == value));
                let mut chips = vec![
                    chip(("choice", id * 10), "Default", current.is_none(), cx).on_click(
                        cx.listener(move |this, _, window, cx| this.write(key, "", window, cx)),
                    ),
                ];
                for (position, (value, label)) in options.iter().enumerate() {
                    let on = current == Some(*value);
                    let value = *value;
                    chips.push(
                        chip(("choice", id * 10 + position + 1), label, on, cx).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.write(key, value, window, cx)
                            }),
                        ),
                    );
                }
                // Something set by hand, which no chip stands for, stays visible.
                if let Some(value) = unknown {
                    chips.push(chip(("choice", id * 10 + 9), value, true, cx));
                }
                row.v_flex()
                    .child(words)
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .into_any_element()
            }
        }
    }

    fn render_aliases(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        let listed = self
            .values
            .as_ref()
            .map_or(0, |values| values.aliases().len());
        if listed == 0 {
            rows.push(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(muted)
                    .child("No aliases yet. An alias named co that runs checkout lets you type git co.")
                    .into_any_element(),
            );
        }
        for (index, row) in self.aliases.iter().enumerate() {
            let name = row.name.clone();
            rows.push(
                div()
                    .h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .child(
                        div()
                            .w(NAME_WIDTH)
                            .flex_none()
                            .truncate()
                            .text_size(px(14.))
                            .child(format!("git {}", row.name)),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&row.input)))
                    .child(
                        Button::new(("remove-alias", index))
                            .ghost()
                            .compact()
                            .label("Remove")
                            .tooltip(format!("Delete the alias git {}", row.name))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_alias(&name, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }

        let ready = config::alias_problem(self.new_name.read(cx).value().trim()).is_none()
            && !self.new_command.read(cx).value().trim().is_empty();
        rows.push(
            div()
                .h_flex()
                .px_4()
                .py_2()
                .gap_3()
                .child(
                    div()
                        .w(NAME_WIDTH)
                        .flex_none()
                        .child(Input::new(&self.new_name)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.new_command)),
                )
                .child(
                    Button::new("add-alias")
                        .ghost()
                        .compact()
                        .label("Add alias")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, window, cx| this.add_alias(window, cx))),
                )
                .into_any_element(),
        );
        self.render_group("Aliases", rows, cx)
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, text) = (theme.background, theme.foreground);
        let page = match self.page {
            Page::GitConfig => self.render_git_config(cx),
        };
        div()
            .v_flex()
            .size_full()
            .bg(background)
            .text_color(text)
            .child(self.render_header(cx))
            .child(
                div()
                    .h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .id("settings-page")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .child(div().h_flex().justify_center().p_6().child(page)),
                    ),
            )
    }
}

/// Whether an input has finished being edited: Enter, or leaving it.
fn commits(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

/// Puts `text` in `input`, unless it is being typed in or already says that.
fn sync(input: &Entity<InputState>, text: String, window: &mut Window, cx: &mut Context<Settings>) {
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    if !focused && input.read(cx).value().as_ref() != text {
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    }
}

/// One option of a choice, with a tick when picked so that colour isn't the only cue.
fn chip(id: impl Into<ElementId>, label: &str, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (selected, hover) = (selected_row(theme), theme.list_hover);
    let (text, quiet) = (theme.foreground, theme.secondary_foreground);
    div()
        .id(id)
        .h_flex()
        .h(px(26.))
        .px_2()
        .gap_1()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .when(on, |chip| {
            chip.bg(selected).text_color(text).font_semibold()
        })
        .when(!on, |chip| {
            chip.text_color(quiet).hover(|chip| chip.bg(hover))
        })
        .when(on, |chip| chip.child("✓"))
        .child(label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Feedback, Scope, Settings, config};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    fn open(scope: Scope, cx: &mut TestAppContext) -> (Entity<Settings>, VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let window = cx.add_window(|window, cx| Settings::new(scope, window, cx));
        let view = window.root(cx).unwrap();
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        settle(&mut cx);
        (view, cx)
    }

    /// The tool runs on a real thread, so give it time to answer.
    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..50 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn changes_are_written_to_the_file_and_shown_back(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope::File(dir.path().join("gitconfig"));
        config::set(&scope, "user.name", "Ada").unwrap();
        let (view, mut cx) = open(scope.clone(), cx);

        let shown = view.read_with(&cx, |view, cx| {
            view.fields
                .iter()
                .find(|field| field.key == "user.name")
                .map(|field| field.input.read(cx).value().to_string())
        });
        assert_eq!(shown.as_deref(), Some("Ada"), "existing values are loaded");

        view.update_in(&mut cx, |view, window, cx| {
            view.write("fetch.prune", "true", window, cx);
        });
        settle(&mut cx);
        assert_eq!(
            config::read(&scope).unwrap().get("fetch.prune"),
            Some("true")
        );
        view.read_with(&cx, |view, _| {
            assert!(
                matches!(&view.feedback, Feedback::Saved(message) if message.contains("fetch.prune"))
            );
        });

        view.update_in(&mut cx, |view, window, cx| {
            view.write("fetch.prune", "", window, cx);
        });
        settle(&mut cx);
        assert!(!config::read(&scope).unwrap().is_set("fetch.prune"));
    }

    #[gpui_kit::test]
    fn a_bad_alias_says_why_and_writes_nothing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope::File(dir.path().join("gitconfig"));
        let (view, mut cx) = open(scope.clone(), cx);

        view.update_in(&mut cx, |view, window, cx| {
            view.new_name
                .update(cx, |input, cx| input.set_value("a b", window, cx));
            view.add_alias(window, cx);
        });
        settle(&mut cx);
        view.read_with(&cx, |view, _| {
            assert!(matches!(view.feedback, Feedback::Failed(_)));
        });
        assert!(config::read(&scope).unwrap().aliases().is_empty());
    }
}
