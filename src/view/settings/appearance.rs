//! The Theme page: change the colours of the design, or take them from the Qt theme the
//! desktop uses. Dark and light are two themes, each a list of changes on top of
//! Catppuccin, and each colour is a hex input.

use super::*;
use gitclient_core::settings::theme::{self as palette, Overrides, Slot};

/// One colour's input, which saves when Enter is pressed or the input is left.
pub(super) struct ColorField {
    pub(super) slot: Slot,
    pub(super) input: Entity<InputState>,
    _events: Subscription,
}

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

impl Settings {
    /// The changes to the appearance being edited.
    fn theme_edited(&self) -> &Overrides {
        if self.editing_dark {
            &self.app.theme_dark
        } else {
            &self.app.theme_light
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

    /// Saves a change to the theme being edited and puts it on screen at once.
    fn change_theme(
        &mut self,
        change: impl FnOnce(&mut Overrides),
        said: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.state_file else {
            return;
        };
        let dark = self.editing_dark;
        let result = AppState::change(file, |state| {
            change(if dark {
                &mut state.theme_dark
            } else {
                &mut state.theme_light
            })
        });
        match result {
            Ok(state) => {
                crate::theme::apply(state.theme_dark.clone(), state.theme_light.clone(), cx);
                self.app = state;
                self.feedback = Feedback::Saved(said);
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        self.show_colors(window, cx);
        cx.notify();
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
                self.change_theme(|theme| theme.set(slot, color, dark), said, window, cx);
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
        self.change_theme(|theme| theme.clear(slot), said, window, cx);
    }

    pub(super) fn reset_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let said = palette::reset_message(self.editing_dark);
        self.change_theme(|theme| *theme = Overrides::default(), said, window, cx);
    }

    fn pick_appearance(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_dark != dark {
            self.editing_dark = dark;
            self.feedback = Feedback::None;
            self.show_colors(window, cx);
            cx.notify();
        }
    }

    /// Takes the colours of a desktop theme, for the appearance they suit. Only colours are
    /// read, and only the theme of that one appearance changes.
    pub(super) fn import_theme(
        &mut self,
        source: palette::Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let found = palette::Locations::this_system().and_then(|places| places.read(source));
        match found.ok_or_else(|| format!("gitclient found no {} to read.", source.name())) {
            Ok(found) => {
                self.editing_dark = found.dark;
                let said = found.saved_message();
                self.change_theme(|theme| *theme = found.overrides, said, window, cx);
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
                        "Takes only its colours, for the appearance its background suits.",
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
        let resetting = div()
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
                        "gitclient can't find a folder to keep this in.".to_owned()
                    }),
            )
            .into_any_element();
        // The built-in theme is always a choice, ticked while nothing is changed. Once
        // colours are changed a "Custom" chip says so.
        let custom = !self.theme_edited().is_empty();
        let builtin = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(self.row_words("Theme", "The built-in theme, or your own colours.", cx))
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(
                        chip("theme-builtin", palette::builtin_name(dark), !custom, cx).on_click(
                            cx.listener(|this, _, window, cx| this.reset_theme(window, cx)),
                        ),
                    )
                    .children(custom.then(|| chip("theme-custom", "Custom", true, cx))),
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
                "Change the colours of gitclient. Blue stays the one next action, and branches keep \
                 one colour each wherever they are drawn.",
                cx,
            ))
            .child(self.render_group(
                "Colours",
                [choosing, builtin, resetting]
                    .into_iter()
                    .chain(importing)
                    .collect(),
                cx,
            ))
            .child(self.render_group("Surfaces and text", rows(&SURFACES, cx), cx))
            .child(self.render_group("Meaning", rows(&MEANINGS, cx), cx))
            .child(self.render_group("Branch lanes", rows(&LANES, cx), cx))
            .into_any_element()
    }
}
