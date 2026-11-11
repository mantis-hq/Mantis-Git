//! The app's look, per `docs/design.md`: Catppuccin colours, the bundled Maple Mono, and
//! following the OS appearance.

use gitclient_core::history::lanes::LaneColor;
use gitclient_core::settings::theme::{Overrides, Slot};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeRegistry};
use gpui_kit::{App, Hsla, Window, rgb};
use std::borrow::Cow;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};

static GENERATION: AtomicUsize = AtomicUsize::new(0);

const THEMES: &str = include_str!("catppuccin.json");
const LIGHT_THEME: &str = "Catppuccin Latte Blue";
const DARK_THEME: &str = "Catppuccin Mocha Blue";

const FONT: &str = "Maple Mono";
const FONT_FILES: [&[u8]; 3] = [
    include_bytes!("fonts/MapleMono-Regular.ttf"),
    include_bytes!("fonts/MapleMono-Medium.ttf"),
    include_bytes!("fonts/MapleMono-SemiBold.ttf"),
];

/// Mauve, Teal, Pink, Sky, Rosewater, Flamingo. Never a semantic colour.
const LANES: [Slot; 6] = [
    Slot::Mauve,
    Slot::Teal,
    Slot::Pink,
    Slot::Sky,
    Slot::Rosewater,
    Slot::Flamingo,
];

/// What the user changed, for dark and for light. The colours are read from here by
/// everything that draws without going through the gpui-kit theme.
static USER: RwLock<(Overrides, Overrides)> = RwLock::new((Overrides::new(), Overrides::new()));

/// The user's colours for one appearance.
fn user(is_dark: bool) -> Overrides {
    let both = USER.read().unwrap_or_else(|e| e.into_inner());
    if is_dark {
        both.0.clone()
    } else {
        both.1.clone()
    }
}

fn slot_color(slot: Slot, is_dark: bool) -> Hsla {
    rgb(user(is_dark).color(slot, is_dark)).into()
}

/// Design tokens gpui-kit's theme has no field for. Everything else comes from `cx.theme()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Paused, needs a decision.
    pub peach: Hsla,
    /// Secondary text.
    pub subtext0: Hsla,
    /// Line numbers and idle checkbox outlines. Too low in contrast for body text.
    pub overlay1: Hsla,
}

pub fn tokens(is_dark: bool) -> Tokens {
    Tokens {
        peach: slot_color(Slot::Peach, is_dark),
        subtext0: slot_color(Slot::Subtext0, is_dark),
        overlay1: slot_color(Slot::Overlay1, is_dark),
    }
}

/// Installs the themes and font. Call once, after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    cx.text_system()
        .add_fonts(FONT_FILES.map(Cow::Borrowed).to_vec())
        .expect("bundled fonts load");
    install(cx);
    Theme::update(cx, |theme| {
        theme.font_family = FONT.into();
        theme.mono_font_family = FONT.into();
    });
}

/// Loads the bundled themes with the user's colours put in, and switches to them.
fn install(cx: &mut App) {
    let (dark, light) = (user(true), user(false));
    // The registry keeps the first theme it was given under a name, so each time the
    // colours change they go in under a new one.
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let name = |base: &str| format!("{base} {generation}");
    let themes = Overrides::recolor_themes(&dark, &light, THEMES)
        .replace(LIGHT_THEME, &name(LIGHT_THEME))
        .replace(DARK_THEME, &name(DARK_THEME));
    ThemeRegistry::global_mut(cx)
        .load_themes_from_str(&themes)
        .expect("bundled themes parse");
    let themes = ThemeRegistry::global(cx).themes();
    let (light, dark) = (
        themes[name(LIGHT_THEME).as_str()].clone(),
        themes[name(DARK_THEME).as_str()].clone(),
    );
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    Theme::change(cx.theme().mode, None, cx);
}

/// Puts the user's colours in everywhere, in every open window, at once.
pub fn apply(dark: Overrides, light: Overrides, cx: &mut App) {
    *USER.write().unwrap_or_else(|e| e.into_inner()) = (dark, light);
    install(cx);
    cx.refresh_windows();
}

/// Matches the theme to the OS appearance now and whenever it changes.
pub fn follow_system(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    window
        .observe_window_appearance(|window, cx| Theme::sync_system_appearance(Some(window), cx))
        .detach();
}

/// The selected row's fill. gpui-kit clamps `list_active` to 20% opacity whatever the
/// theme file says, and the design calls for solid Surface 0.
pub fn selected_row(theme: &Theme) -> Hsla {
    Hsla {
        a: 1.,
        ..theme.list_active
    }
}

/// The colour of a lane: Blue for you, otherwise the palette, repeating as it runs out.
pub fn lane_color(color: LaneColor, is_dark: bool) -> Hsla {
    match color {
        LaneColor::Head => slot_color(Slot::Blue, is_dark),
        LaneColor::Branch(index) => slot_color(LANES[index as usize % LANES.len()], is_dark),
    }
}

/// Catppuccin's terminal colours: black, red, green, yellow, blue, pink, teal, white, then
/// the bright set.
const DARK_ANSI: [u32; 16] = [
    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70,
    0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
];
const LIGHT_ANSI: [u32; 16] = [
    0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85,
    0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
];

/// A colour a terminal program asked for by number. The first 16 are the theme's; the rest
/// are the standard 6×6×6 cube and grey ramp, which programs rely on being the same
/// everywhere.
pub fn ansi_color(index: u8, is_dark: bool) -> Hsla {
    let named = if is_dark { DARK_ANSI } else { LIGHT_ANSI };
    match index {
        // Catppuccin's terminal colours, in the user's version of each.
        0..=15 => rgb(user(is_dark).recolor(named[index as usize], is_dark)).into(),
        16..=231 => {
            let cube = index - 16;
            let level = |n: u8| if n == 0 { 0 } else { 55 + 40 * n as u32 };
            let (r, g, b) = (level(cube / 36), level(cube / 6 % 6), level(cube % 6));
            rgb(r << 16 | g << 8 | b).into()
        }
        _ => {
            let grey = 8 + 10 * (index as u32 - 232);
            rgb(grey << 16 | grey << 8 | grey).into()
        }
    }
}

/// A colour a terminal program asked for by its red, green and blue.
pub fn rgb_color(red: u8, green: u8, blue: u8) -> Hsla {
    rgb((red as u32) << 16 | (green as u32) << 8 | blue as u32).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[test]
    fn terminal_colours_use_the_theme_then_the_standard_cube() {
        assert_eq!(ansi_color(1, true), color(0xf38ba8));
        assert_eq!(ansi_color(1, false), color(0xd20f39));
        assert_eq!(ansi_color(16, true), color(0x000000));
        assert_eq!(ansi_color(196, true), color(0xff0000));
        assert_eq!(ansi_color(231, true), color(0xffffff));
        assert_eq!(ansi_color(232, true), color(0x080808));
        assert_eq!(rgb_color(1, 2, 3), color(0x010203));
    }
    use gpui_kit::component::ThemeMode;

    fn color(hex: u32) -> Hsla {
        rgb(hex).into()
    }

    #[gpui_kit::test]
    fn dark_mode_matches_the_design_tokens(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(cx);
            Theme::change(ThemeMode::Dark, None, cx);

            let theme = cx.theme();
            assert_eq!(theme.background, color(0x1e1e2e));
            assert_eq!(theme.foreground, color(0xcdd6f4));
            assert_eq!(theme.muted_foreground, color(0x9399b2));
            assert_eq!(theme.border, color(0x313244));
            assert_eq!(theme.list_head, color(0x181825));
            assert_eq!(selected_row(theme), color(0x313244));
            assert_eq!(theme.primary, color(0x89b4fa));
            assert_eq!(theme.primary_foreground, color(0x11111b));
            assert_eq!(theme.secondary_foreground, color(0xbac2de));
            assert!(!theme.shadow);
        });
    }

    #[gpui_kit::test]
    fn light_mode_uses_latte_in_the_same_roles(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(cx);
            Theme::change(ThemeMode::Light, None, cx);

            let theme = cx.theme();
            assert_eq!(theme.background, color(0xeff1f5));
            assert_eq!(theme.muted_foreground, color(0x7c7f93));
            assert_eq!(theme.primary, color(0x1e66f5));
        });
    }

    #[gpui_kit::test]
    fn switching_mode_keeps_the_font(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            assert_eq!(cx.theme().font_family.as_ref(), FONT);
            assert_eq!(cx.theme().mono_font_family.as_ref(), FONT);
        });
    }

    #[test]
    fn lanes_start_at_mauve_and_repeat() {
        let first = LaneColor::Branch(0);
        assert_eq!(lane_color(first, true), color(0xcba6f7));
        assert_eq!(
            lane_color(LaneColor::Branch(6), true),
            lane_color(first, true)
        );
        assert_eq!(lane_color(first, false), color(0x8839ef));
        assert_eq!(lane_color(LaneColor::Head, true), color(0x89b4fa));
    }
}
