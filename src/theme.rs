//! The app's look, per `docs/design.md`: Catppuccin colours, the bundled Maple Mono, and
//! following the OS appearance.

use gitclient_core::graph::LaneColor;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeRegistry};
use gpui_kit::{App, Hsla, Window, rgb};
use std::borrow::Cow;

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
const DARK_LANES: [u32; 6] = [0xcba6f7, 0x94e2d5, 0xf5c2e7, 0x89dceb, 0xf5e0dc, 0xf2cdcd];
const LIGHT_LANES: [u32; 6] = [0x8839ef, 0x179299, 0xea76cb, 0x04a5e5, 0xdc8a78, 0xdd7878];

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
    let (peach, subtext0, overlay1) = if is_dark {
        (0xfab387, 0xa6adc8, 0x7f849c)
    } else {
        (0xfe640b, 0x6c6f85, 0x8c8fa1)
    };
    Tokens {
        peach: rgb(peach).into(),
        subtext0: rgb(subtext0).into(),
        overlay1: rgb(overlay1).into(),
    }
}

/// Installs the themes and font. Call once, after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    cx.text_system()
        .add_fonts(FONT_FILES.map(Cow::Borrowed).to_vec())
        .expect("bundled fonts load");
    ThemeRegistry::global_mut(cx)
        .load_themes_from_str(THEMES)
        .expect("bundled themes parse");
    let themes = ThemeRegistry::global(cx).themes();
    let light = themes[LIGHT_THEME].clone();
    let dark = themes[DARK_THEME].clone();

    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
        theme.font_family = FONT.into();
        theme.mono_font_family = FONT.into();
    });
    Theme::change(cx.theme().mode, None, cx);
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
    let lanes = if is_dark { DARK_LANES } else { LIGHT_LANES };
    match color {
        LaneColor::Head if is_dark => rgb(0x89b4fa).into(),
        LaneColor::Head => rgb(0x1e66f5).into(),
        LaneColor::Branch(index) => rgb(lanes[index as usize % lanes.len()]).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
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
