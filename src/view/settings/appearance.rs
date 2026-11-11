//! The Theme page: pick a theme, change the colours of the design, take them from the
//! desktop, and keep themes as files in a folder. Dark and light are two themes, each
//! either a file picked from the folder or a list of changes on top of Catppuccin, and each
//! colour is a hex input.

use super::*;
use mantisgit_core::settings::theme::{self as palette, Overrides, Slot};
use mantisgit_core::settings::themes;
use std::time::Duration;

/// One colour's input, which saves when Enter is pressed or the input is left.
pub(super) struct ColorField {
    pub(super) slot: Slot,
    pub(super) input: Entity<InputState>,
    _events: Subscription,
}

/// How often the page looks at the themes folder again while it is showing.
const LOOK_EVERY: Duration = Duration::from_millis(1500);

const SURFACES: [Slot; 10] = [
    Slot::Base,
    Slot::Mantle,
    Slot::Crust,
    Slot::Surface0,
    Slot::Surface1,
    Slot::Overlay1,
    Slot::Overlay2,
    Slot::Subtext0,
    Slot::Subtext1,
    Slot::Text,
];
const MEANINGS: [Slot; 5] = [
    Slot::Blue,
    Slot::Green,
    Slot::Red,
    Slot::Yellow,
    Slot::Peach,
];
const LANES: [Slot; 6] = [
    Slot::Mauve,
    Slot::Teal,
    Slot::Pink,
    Slot::Sky,
    Slot::Rosewater,
    Slot::Flamingo,
];

pub(super) fn color_fields(window: &mut Window, cx: &mut Context<Settings>) -> Vec<ColorField> {
    Slot::ALL
        .into_iter()
        .map(|slot| {
            let input = field("#rrggbb", "", window, cx);
            let events = cx.subscribe_in(
                &input,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    if this.edited(&format!("color:{}", slot.name()), event) {
                        this.commit_color(slot, window, cx);
                    }
                },
            );
            ColorField {
                slot,
                input,
                _events: events,
            }
        })
        .collect()
}

/// The name input of "Save these colours as a theme", which saves on Enter.
pub(super) fn name_input(
    window: &mut Window,
    cx: &mut Context<Settings>,
) -> (Entity<InputState>, Subscription) {
    let input = field("night", "", window, cx);
    let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::PressEnter { .. }) {
            this.save_theme_file(window, cx);
        }
        cx.notify();
    });
    (input, events)
}

/// Looks at the themes folder again whenever the window is used again, and every moment
/// while the page shows, so a theme saved in an editor is seen without any action.
pub(super) fn watch(window: &mut Window, cx: &mut Context<Settings>) -> (Subscription, Task<()>) {
    let activation = cx.observe_window_activation(window, |this, window, cx| {
        if window.is_window_active() {
            this.refresh_themes(window, cx);
        }
    });
    let looking = cx.spawn_in(window, async move |this, cx| {
        loop {
            cx.background_executor().timer(LOOK_EVERY).await;
            let seen = this.update_in(cx, |this, window, cx| {
                if this.page == Page::Theme {
                    this.refresh_themes(window, cx);
                }
            });
            if seen.is_err() {
                break;
            }
        }
    });
    (activation, looking)
}

/// The theme file picked for one appearance, and the colours changed on the page.
fn target(state: &mut AppState, dark: bool) -> (&mut Overrides, &mut Option<String>) {
    if dark {
        (&mut state.theme_dark, &mut state.theme_dark_file)
    } else {
        (&mut state.theme_light, &mut state.theme_light_file)
    }
}

impl Settings {
    /// What is in force for the appearance being edited.
    fn theme_edited(&self) -> &Overrides {
        if self.editing_dark {
            &self.shown.0
        } else {
            &self.shown.1
        }
    }

    /// The theme file picked for the appearance being edited.
    pub(super) fn picked_file(&self) -> Option<&str> {
        if self.editing_dark {
            self.app.theme_dark_file.as_deref()
        } else {
            self.app.theme_light_file.as_deref()
        }
    }

    /// Fills the inputs from the theme being edited.
    pub(super) fn show_colors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (theme, dark) = (self.theme_edited().clone(), self.editing_dark);
        for field in &self.color_fields {
            let text = palette::hex(theme.color(field.slot, dark));
            self.dirty.remove(&format!("color:{}", field.slot.name()));
            field
                .input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// Reads the themes folder again and puts in force what the settings and the files say.
    /// A file that can't be read leaves the theme that was showing and is said on the page.
    pub(super) fn refresh_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder = self.themes_dir.as_deref();
        self.theme_entries = folder.map(themes::list).unwrap_or_default();
        let (dark, dark_note) = themes::in_force(
            folder,
            self.app.theme_dark_file.as_deref(),
            &self.app.theme_dark,
            true,
            &self.shown.0,
        );
        let (light, light_note) = themes::in_force(
            folder,
            self.app.theme_light_file.as_deref(),
            &self.app.theme_light,
            false,
            &self.shown.1,
        );
        let mut notes: Vec<String> = dark_note.into_iter().chain(light_note).collect();
        for entry in &self.theme_entries {
            if let Err(problem) = &entry.theme
                && !notes.contains(problem)
            {
                notes.push(problem.clone());
            }
        }
        self.theme_notes = notes;
        // Only a window that keeps settings puts colours in force, as in `change_theme`.
        if self.state_file.is_some() {
            crate::theme::sync(&self.app, folder, cx);
        }
        let typing = self.dirty.iter().any(|id| id.starts_with("color:"));
        if dark != self.shown.0 || light != self.shown.1 {
            self.shown = (dark, light);
            if !typing {
                self.show_colors(window, cx);
            }
        }
        cx.notify();
    }

    /// Saves a change to the settings and puts it on screen at once.
    fn change_theme(
        &mut self,
        change: impl FnOnce(&mut AppState),
        said: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.state_file else {
            return;
        };
        match AppState::change(file, change) {
            Ok(state) => {
                self.app = state;
                self.feedback = Feedback::Saved(said);
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        self.refresh_themes(window, cx);
        self.show_colors(window, cx);
    }

    /// Changes one colour. When a theme file is picked the change starts from its colours
    /// and leaves the file alone, so the theme becomes "Custom" and nothing is overwritten.
    fn change_colors(
        &mut self,
        change: impl FnOnce(&mut Overrides) + 'static,
        said: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (dark, shown) = (self.editing_dark, self.theme_edited().clone());
        self.change_theme(
            move |state| {
                let (custom, file) = target(state, dark);
                if file.take().is_some() {
                    *custom = shown;
                }
                change(custom);
            },
            said,
            window,
            cx,
        );
    }

    fn commit_color(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.color_fields.iter().find(|field| field.slot == slot) else {
            return;
        };
        let text = field.input.read(cx).value().to_string();
        let dark = self.editing_dark;
        match palette::check_hex(&text) {
            Ok(color) if self.theme_edited().color(slot, dark) == color => {}
            Ok(color) => {
                let said = palette::changed_message(slot, Some(color), dark);
                self.change_colors(move |theme| theme.set(slot, color, dark), said, window, cx);
            }
            Err(error) => {
                self.feedback = Feedback::Failed(error);
                self.show_colors(window, cx);
                cx.notify();
            }
        }
    }

    fn reset_color(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        let said = palette::changed_message(slot, None, self.editing_dark);
        self.change_colors(move |theme| theme.clear(slot), said, window, cx);
    }

    /// Puts the built-in theme of the appearance being edited in force.
    pub(super) fn reset_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (dark, said) = (self.editing_dark, palette::reset_message(self.editing_dark));
        self.change_theme(
            move |state| {
                let (custom, file) = target(state, dark);
                *custom = Overrides::default();
                *file = None;
            },
            said,
            window,
            cx,
        );
    }

    /// Picks a theme file for the appearance being edited.
    pub(super) fn pick_theme_file(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dark = self.editing_dark;
        let said = themes::picked_message(&name, dark);
        self.change_theme(
            move |state| {
                let (custom, file) = target(state, dark);
                *custom = Overrides::default();
                *file = Some(name);
            },
            said,
            window,
            cx,
        );
    }

    /// Writes the colours on show as a theme file, then picks it.
    pub(super) fn save_theme_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(folder), Some(_)) = (self.themes_dir.clone(), &self.state_file) else {
            return;
        };
        let name = self.new_theme_name.read(cx).value().trim().to_owned();
        let (dark, shown) = (self.editing_dark, self.theme_edited().clone());
        match themes::save(&folder, &name, dark, &shown, false) {
            Ok(_) => {
                self.new_theme_name
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.pick_theme_file(name, window, cx);
            }
            Err(error) => {
                self.feedback = Feedback::Failed(error);
                cx.notify();
            }
        }
    }

    /// Opens the themes folder in the system's file manager, making it first.
    fn open_themes_folder(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = &self.themes_dir else {
            return;
        };
        match std::fs::create_dir_all(folder) {
            Ok(()) => cx.reveal_path(folder),
            Err(error) => {
                self.feedback =
                    Feedback::Failed(format!("Couldn't make {}: {error}", folder.display()));
                cx.notify();
            }
        }
    }

    fn pick_appearance(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_dark != dark {
            self.editing_dark = dark;
            self.feedback = Feedback::None;
            self.show_colors(window, cx);
            cx.notify();
        }
    }

    /// Takes the colours of a desktop theme into a theme file and picks it, for the
    /// appearance they suit. Only colours are read, and only that appearance changes.
    pub(super) fn import_theme(
        &mut self,
        source: palette::Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(folder) = self.themes_dir.clone() else {
            return;
        };
        let found = palette::Locations::this_system().and_then(|places| places.read(source));
        let Some(found) = found else {
            self.feedback =
                Feedback::Failed(format!("Mantis Git found no {} to read.", source.name()));
            cx.notify();
            return;
        };
        let stem = source.file_stem();
        match themes::save(&folder, stem, found.dark, &found.overrides, true) {
            Ok(_) => {
                self.editing_dark = found.dark;
                let said = found.saved_message();
                let dark = found.dark;
                self.change_theme(
                    move |state| {
                        let (custom, file) = target(state, dark);
                        *custom = Overrides::default();
                        *file = Some(stem.to_owned());
                    },
                    said,
                    window,
                    cx,
                );
            }
            Err(error) => {
                self.feedback = Feedback::Failed(error);
                cx.notify();
            }
        }
    }

    fn render_color_row(&self, slot: Slot, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let dark = self.editing_dark;
        let set = self.theme_edited().is_set(slot);
        let swatch = rgb(self.theme_edited().color(slot, dark));
        let input = self
            .color_fields
            .iter()
            .find(|field| field.slot == slot)
            .map(|field| field.input.clone());
        div()
            .h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(border)
                    .bg(swatch),
            )
            .child(self.row_words(slot.name(), slot.role(), cx))
            .child(
                div()
                    .w(px(120.))
                    .flex_none()
                    .children(input.map(|input| Input::new(&input))),
            )
            .child(div().w(px(60.)).flex_none().children(set.then(|| {
                Button::new(("reset-color", slot as usize))
                    .ghost()
                    .small()
                    .label("Reset")
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.reset_color(slot, window, cx)),
                    )
            })))
            .into_any_element()
    }

    /// The "Theme" row: the built-in theme first and always, then each theme file for this
    /// appearance, then "Custom" while colours were changed on the page. Anything wrong
    /// with a file is said under it.
    fn render_theme_choice(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = self.editing_dark;
        let peach = tokens(cx.theme().is_dark()).peach;
        let picked = self.picked_file().map(str::to_owned);
        let custom = picked.is_none() && !self.theme_edited().is_empty();
        let files: Vec<String> = self
            .theme_entries
            .iter()
            .filter(|entry| entry.theme.as_ref().is_ok_and(|theme| theme.dark == dark))
            .map(|entry| entry.name.clone())
            .collect();
        let chips = div()
            .h_flex()
            .flex_wrap()
            .gap_1()
            .child(
                chip(
                    "theme-builtin",
                    palette::builtin_name(dark),
                    picked.is_none() && !custom,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.reset_theme(window, cx))),
            )
            .children(files.into_iter().enumerate().map(|(index, name)| {
                let on = picked.as_deref() == Some(name.as_str());
                let chosen = name.clone();
                chip(("theme-file", index), &name, on, cx).on_click(cx.listener(
                    move |this, _, window, cx| this.pick_theme_file(chosen.clone(), window, cx),
                ))
            }))
            .children(custom.then(|| chip("theme-custom", "Custom", true, cx)));
        div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(self.row_words(
                "Theme",
                "The built-in theme, a theme file from the themes folder, or your own colours.",
                cx,
            ))
            .child(chips)
            .children(self.theme_notes.iter().map(|note| {
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(peach)
                    .child(note.clone())
            }))
            .into_any_element()
    }

    /// The "Theme files" card: saving the colours on show as a file, and the folder.
    fn render_theme_files(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let folder = self.themes_dir.as_ref()?;
        let saving = div()
            .h_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(self.row_words(
                "Save these colours as a theme",
                "Writes the colours you see to a file named for this, and picks it.",
                cx,
            ))
            .child(
                div()
                    .w(px(160.))
                    .flex_none()
                    .child(Input::new(&self.new_theme_name)),
            )
            .child(
                Button::new("save-theme-file")
                    .ghost()
                    .label("Save")
                    .on_click(cx.listener(|this, _, window, cx| this.save_theme_file(window, cx))),
            )
            .into_any_element();
        let opening = div()
            .h_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(self.row_words(
                "Themes folder",
                &format!(
                    "{}. Each .theme file is a theme: one Name = #rrggbb line per colour. \
                     Save one in any editor and the app follows.",
                    folder.display()
                ),
                cx,
            ))
            .child(
                Button::new("open-themes-folder")
                    .ghost()
                    .label("Open themes folder")
                    .on_click(cx.listener(|this, _, _, cx| this.open_themes_folder(cx))),
            )
            .into_any_element();
        Some(self.render_group("Theme files", vec![saving, opening], cx))
    }

    pub(super) fn render_theme(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let can_save = self.state_file.is_some();
        let dark = self.editing_dark;
        let appearance = |id: &'static str, label: &str, is_dark: bool| {
            chip(id, label, dark == is_dark, cx).on_click(
                cx.listener(move |this, _, window, cx| this.pick_appearance(is_dark, window, cx)),
            )
        };
        // One row per desktop theme found on this system, so none is drawn where there is
        // nothing to read, as on Windows.
        let importing: Vec<AnyElement> = self
            .theme_sources
            .iter()
            .map(|&source| {
                div()
                    .h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(self.row_words(
                        &source.button(),
                        "Takes only its colours, into a theme file, for the appearance its \
                         background suits.",
                        cx,
                    ))
                    .child(
                        Button::new(("import-theme", source as usize))
                            .ghost()
                            .label("Import")
                            .disabled(!can_save)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.import_theme(source, window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let choosing = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(self.row_words(
                "Appearance",
                "The app follows the OS, so dark and light each have their own colours.",
                cx,
            ))
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(appearance("theme-dark", "Dark", true))
                    .child(appearance("theme-light", "Light", false)),
            )
            .into_any_element();
        let describing = div()
            .h_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(if can_save {
                        palette::describe(self.theme_edited(), dark)
                    } else {
                        "Mantis Git can't find a folder to keep this in.".to_owned()
                    }),
            )
            .into_any_element();
        let rows = |slots: &[Slot], cx: &mut Context<Self>| {
            slots
                .iter()
                .map(|slot| self.render_color_row(*slot, cx))
                .collect::<Vec<_>>()
        };
        div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head(
                "Theme",
                "Change the colours of Mantis Git. Blue stays the one next action, and branches keep \
                 one colour each wherever they are drawn.",
                cx,
            ))
            .child(self.render_group(
                "Colours",
                [choosing, self.render_theme_choice(cx), describing]
                    .into_iter()
                    .chain(importing)
                    .collect(),
                cx,
            ))
            .children(self.render_theme_files(cx))
            .child(self.render_group("Surfaces and text", rows(&SURFACES, cx), cx))
            .child(self.render_group("Meaning", rows(&MEANINGS, cx), cx))
            .child(self.render_group("Branch lanes", rows(&LANES, cx), cx))
            .into_any_element()
    }
}
