//! The colours a user can change. The design is Catppuccin; a user's theme is a short list
//! of overrides on top of it, one for dark and one for light, so a colour left alone stays
//! as designed. Colours are plain `0xRRGGBB` numbers here, and `src/theme.rs` turns them
//! into real ones.
//!
//! The overrides can be typed in, or imported from the Qt theme the desktop uses (KDE's
//! `kdeglobals`, or a `qt6ct`/`qt5ct` palette).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One colour of the design that can be changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Crust,
    Mantle,
    Base,
    Surface0,
    Surface1,
    Overlay1,
    Overlay2,
    Subtext0,
    Subtext1,
    Text,
    Blue,
    Green,
    Red,
    Yellow,
    Peach,
    Mauve,
    Teal,
    Pink,
    Sky,
    Rosewater,
    Flamingo,
}

impl Slot {
    pub const ALL: [Slot; 21] = [
        Slot::Crust,
        Slot::Mantle,
        Slot::Base,
        Slot::Surface0,
        Slot::Surface1,
        Slot::Overlay1,
        Slot::Overlay2,
        Slot::Subtext0,
        Slot::Subtext1,
        Slot::Text,
        Slot::Blue,
        Slot::Green,
        Slot::Red,
        Slot::Yellow,
        Slot::Peach,
        Slot::Mauve,
        Slot::Teal,
        Slot::Pink,
        Slot::Sky,
        Slot::Rosewater,
        Slot::Flamingo,
    ];

    /// The name shown, which is also the design's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Slot::Crust => "Crust",
            Slot::Mantle => "Mantle",
            Slot::Base => "Base",
            Slot::Surface0 => "Surface 0",
            Slot::Surface1 => "Surface 1",
            Slot::Overlay1 => "Overlay 1",
            Slot::Overlay2 => "Overlay 2",
            Slot::Subtext0 => "Subtext 0",
            Slot::Subtext1 => "Subtext 1",
            Slot::Text => "Text",
            Slot::Blue => "Blue",
            Slot::Green => "Green",
            Slot::Red => "Red",
            Slot::Yellow => "Yellow",
            Slot::Peach => "Peach",
            Slot::Mauve => "Mauve",
            Slot::Teal => "Teal",
            Slot::Pink => "Pink",
            Slot::Sky => "Sky",
            Slot::Rosewater => "Rosewater",
            Slot::Flamingo => "Flamingo",
        }
    }

    /// What the colour is for, in a few words.
    pub fn role(self) -> &'static str {
        match self {
            Slot::Crust => "text on filled blue",
            Slot::Mantle => "header, side panels, diffs",
            Slot::Base => "main content, the graph",
            Slot::Surface0 => "borders, selected rows",
            Slot::Surface1 => "input outlines",
            Slot::Overlay1 => "line numbers",
            Slot::Overlay2 => "muted text",
            Slot::Subtext0 => "secondary text",
            Slot::Subtext1 => "quiet buttons",
            Slot::Text => "primary text",
            Slot::Blue => "you: HEAD, the next action",
            Slot::Green => "added, finished",
            Slot::Red => "removed",
            Slot::Yellow => "modified, tags",
            Slot::Peach => "paused, needs a decision",
            Slot::Mauve
            | Slot::Teal
            | Slot::Pink
            | Slot::Sky
            | Slot::Rosewater
            | Slot::Flamingo => "a branch lane",
        }
    }

    fn key(self) -> String {
        self.name().to_lowercase().replace(' ', "")
    }

    fn from_key(key: &str) -> Option<Slot> {
        Slot::ALL.into_iter().find(|slot| slot.key() == key)
    }

    fn index(self) -> usize {
        Slot::ALL.iter().position(|slot| *slot == self).unwrap_or(0)
    }

    /// The Catppuccin value, which is the design.
    pub fn catppuccin(self, dark: bool) -> u32 {
        let (mocha, latte) = CATPPUCCIN[self.index()];
        if dark { mocha } else { latte }
    }
}

/// Mocha and Latte, in the order of `Slot::ALL`.
const CATPPUCCIN: [(u32, u32); 21] = [
    (0x11111b, 0xdce0e8),
    (0x181825, 0xe6e9ef),
    (0x1e1e2e, 0xeff1f5),
    (0x313244, 0xccd0da),
    (0x45475a, 0xbcc0cc),
    (0x7f849c, 0x8c8fa1),
    (0x9399b2, 0x7c7f93),
    (0xa6adc8, 0x6c6f85),
    (0xbac2de, 0x5c5f77),
    (0xcdd6f4, 0x4c4f69),
    (0x89b4fa, 0x1e66f5),
    (0xa6e3a1, 0x40a02b),
    (0xf38ba8, 0xd20f39),
    (0xf9e2af, 0xdf8e1d),
    (0xfab387, 0xfe640b),
    (0xcba6f7, 0x8839ef),
    (0x94e2d5, 0x179299),
    (0xf5c2e7, 0xea76cb),
    (0x89dceb, 0x04a5e5),
    (0xf5e0dc, 0xdc8a78),
    (0xf2cdcd, 0xdd7878),
];

/// The colours a user changed for one appearance, dark or light.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides(BTreeMap<Slot, u32>);

impl Overrides {
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The colour in `slot`: the user's, else Catppuccin's.
    pub fn color(&self, slot: Slot, dark: bool) -> u32 {
        self.0
            .get(&slot)
            .copied()
            .unwrap_or_else(|| slot.catppuccin(dark))
    }

    /// Whether the user changed `slot`.
    pub fn is_set(&self, slot: Slot) -> bool {
        self.0.contains_key(&slot)
    }

    /// Sets `slot`. Setting it to what the design already has clears it, so a theme only
    /// holds real changes.
    pub fn set(&mut self, slot: Slot, color: u32, dark: bool) {
        if color == slot.catppuccin(dark) {
            self.0.remove(&slot);
        } else {
            self.0.insert(slot, color);
        }
    }

    pub fn clear(&mut self, slot: Slot) {
        self.0.remove(&slot);
    }

    /// `value` of the state file: `base:1a1b26,blue:7aa2f7`. Unknown or damaged entries
    /// are skipped, so an edit by hand can't break the app.
    pub fn parse(value: &str) -> Self {
        let mut overrides = Self::default();
        for entry in value.split(',') {
            if let Some((key, hex)) = entry.split_once(':')
                && let Some(slot) = Slot::from_key(key.trim())
                && let Some(color) = parse_hex(hex)
            {
                overrides.0.insert(slot, color);
            }
        }
        overrides
    }

    pub fn render(&self) -> String {
        self.0
            .iter()
            .map(|(slot, color)| format!("{}:{color:06x}", slot.key()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The colour `color` becomes in this theme, when it is one of the design's own
    /// (`dark` says which appearance it is from). Anything else stays.
    pub fn recolor(&self, color: u32, dark: bool) -> u32 {
        Slot::ALL
            .into_iter()
            .find(|slot| slot.catppuccin(dark) == color)
            .map_or(color, |slot| self.color(slot, dark))
    }

    /// `themes`, the bundled theme file, with every colour of the design replaced by the
    /// user's. Alpha is kept (`#89b4fa4d`). A file that isn't JSON comes back as it was.
    pub fn recolor_themes(dark: &Self, light: &Self, themes: &str) -> String {
        let Ok(mut file) = serde_json::from_str::<serde_json::Value>(themes) else {
            return themes.to_owned();
        };
        let list = file.get_mut("themes").and_then(|list| list.as_array_mut());
        for theme in list.into_iter().flatten() {
            let is_dark = theme.get("mode").and_then(|mode| mode.as_str()) == Some("dark");
            let overrides = if is_dark { dark } else { light };
            if let Some(colors) = theme.get_mut("colors").and_then(|c| c.as_object_mut()) {
                for value in colors.values_mut() {
                    if let Some(text) = value.as_str() {
                        *value = overrides.recolor_text(text, is_dark).into();
                    }
                }
            }
        }
        file.to_string()
    }

    fn recolor_text(&self, text: &str, dark: bool) -> String {
        let hex = text.strip_prefix('#').unwrap_or("");
        if hex.len() != 6 && hex.len() != 8 {
            return text.to_owned();
        }
        match (parse_hex(&hex[..6]), hex.get(6..)) {
            (Some(color), alpha) => {
                format!("#{:06x}{}", self.recolor(color, dark), alpha.unwrap_or(""))
            }
            _ => text.to_owned(),
        }
    }
}

/// `#1a1b26` or `1a1b26`, in any case.
pub fn parse_hex(text: &str) -> Option<u32> {
    let digits = text.trim().trim_start_matches('#');
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

/// `0x1a1b26` as `#1a1b26`.
pub fn hex(color: u32) -> String {
    format!("#{color:06x}")
}

impl Imported {
    /// What the header says once these colours are in.
    pub fn saved_message(&self) -> String {
        format!(
            "Took {} from the {}, for the {} appearance. Catppuccin is one click away.",
            crate::text::count(self.overrides.len(), "colour"),
            self.source.name(),
            appearance_name(self.dark)
        )
    }
}

/// "dark" or "light".
pub fn appearance_name(dark: bool) -> &'static str {
    if dark { "dark" } else { "light" }
}

/// What the header says after one colour of the theme changed.
pub fn changed_message(slot: Slot, color: Option<u32>, dark: bool) -> String {
    match color {
        Some(color) => format!(
            "{} is {} in the {} appearance.",
            slot.name(),
            hex(color),
            appearance_name(dark)
        ),
        None => format!(
            "{} is Catppuccin's {} again.",
            slot.name(),
            hex(slot.catppuccin(dark))
        ),
    }
}

/// What the header says after the colours of one appearance were put back.
pub fn reset_message(dark: bool) -> String {
    format!(
        "The {} appearance is {} again.",
        appearance_name(dark),
        builtin_name(dark)
    )
}

/// The built-in theme of an appearance, which is always there to go back to.
pub fn builtin_name(dark: bool) -> &'static str {
    if dark {
        "Catppuccin Mocha"
    } else {
        "Catppuccin Latte"
    }
}

/// The sentence under the page's title.
pub fn describe(overrides: &Overrides, dark: bool) -> String {
    let appearance = appearance_name(dark);
    match overrides.len() {
        0 => format!("The {appearance} appearance is {}.", builtin_name(dark)),
        1 => format!("1 colour of the {appearance} appearance is changed from Catppuccin."),
        n => format!(
            "{} of the {appearance} appearance are changed from Catppuccin.",
            crate::text::count(n, "colour")
        ),
    }
}

/// What a typed colour means, or why it can't be used.
pub fn check_hex(text: &str) -> Result<u32, String> {
    parse_hex(text).ok_or_else(|| {
        format!(
            "“{}” isn't a colour. Write it as six digits, like #1e1e2e.",
            text.trim()
        )
    })
}

// Colour arithmetic, on 0xRRGGBB.

fn channels(color: u32) -> [f32; 3] {
    [
        (color >> 16 & 0xff) as f32,
        (color >> 8 & 0xff) as f32,
        (color & 0xff) as f32,
    ]
}

/// `from` moved `amount` (0 to 1) of the way to `to`.
fn mix(from: u32, to: u32, amount: f32) -> u32 {
    let (a, b) = (channels(from), channels(to));
    let part = |i: usize| (a[i] + (b[i] - a[i]) * amount).round() as u32;
    part(0) << 16 | part(1) << 8 | part(2)
}

fn luminance(color: u32) -> f32 {
    let [r, g, b] = channels(color);
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.
}

fn saturation(color: u32) -> f32 {
    let [r, g, b] = channels(color);
    let (high, low) = (r.max(g).max(b), r.min(g).min(b));
    if high == 0. { 0. } else { (high - low) / high }
}

/// A colour scheme read from the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    /// Where it came from, for the sentence that says what was imported.
    pub source: Source,
    /// Whether its background is dark, which says which appearance it is for.
    pub dark: bool,
    pub overrides: Overrides,
}

/// What a Qt theme says, before it is turned into the design's colours.
struct Scheme {
    base: u32,
    window: u32,
    alternate: Option<u32>,
    text: u32,
    accent: u32,
    on_accent: Option<u32>,
    positive: Option<u32>,
    negative: Option<u32>,
    neutral: Option<u32>,
}

impl Scheme {
    /// The design's colours the scheme has an opinion about. The greys between the
    /// background and the text are worked out from those two, so contrast is kept.
    fn into_import(self, source: Source) -> Imported {
        let dark = luminance(self.base) < 0.5;
        let far = if dark { 0x000000 } else { 0xffffff };
        let mantle = if self.window == self.base {
            mix(self.base, 0x000000, if dark { 0.2 } else { 0.06 })
        } else {
            self.window
        };
        let towards_text = |amount: f32| mix(self.base, self.text, amount);
        let surface0 = match self.alternate {
            Some(color) if color != self.base => color,
            _ => towards_text(0.1),
        };
        let mut overrides = Overrides::default();
        let mut put = |slot: Slot, color: u32| overrides.set(slot, color, dark);
        put(Slot::Base, self.base);
        put(Slot::Mantle, mantle);
        put(
            Slot::Crust,
            self.on_accent
                .unwrap_or(if dark { mix(mantle, far, 0.5) } else { far }),
        );
        put(Slot::Surface0, surface0);
        put(Slot::Surface1, towards_text(0.2));
        put(Slot::Overlay1, towards_text(0.45));
        put(Slot::Overlay2, towards_text(0.6));
        put(Slot::Subtext0, towards_text(0.75));
        put(Slot::Subtext1, towards_text(0.85));
        put(Slot::Text, self.text);
        put(Slot::Blue, self.accent);
        // Desktops fill these in carelessly (a grey for "positive" is common), and a grey
        // can't say added or removed, so only a real colour is taken.
        for (slot, color) in [
            (Slot::Green, self.positive),
            (Slot::Red, self.negative),
            (Slot::Peach, self.neutral),
        ] {
            if let Some(color) = color
                && saturation(color) >= 0.4
            {
                put(slot, color);
            }
        }
        Imported {
            source,
            dark,
            overrides,
        }
    }
}

/// `r,g,b` as KDE writes it.
fn kde_color(value: &str) -> Option<u32> {
    let mut parts = value.split(',').map(|n| n.trim().parse::<u8>().ok());
    let (r, g, b) = (parts.next()??, parts.next()??, parts.next()??);
    Some((r as u32) << 16 | (g as u32) << 8 | b as u32)
}

/// Reads KDE's colour scheme format, as `kdeglobals` and `~/.local/share/color-schemes`
/// files use it. `None` when it has no colours.
pub fn from_kde(text: &str) -> Option<Imported> {
    let mut sections: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    let mut current = "";
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name;
        } else if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current)
                .or_default()
                .insert(key.trim(), value.trim());
        }
    }
    let get = |section: &str, key: &str| {
        sections
            .get(section)
            .and_then(|keys| keys.get(key))
            .and_then(|value| kde_color(value))
    };
    let base = get("Colors:View", "BackgroundNormal")?;
    let text_color = get("Colors:View", "ForegroundNormal")?;
    Some(
        Scheme {
            base,
            window: get("Colors:Window", "BackgroundNormal").unwrap_or(base),
            alternate: get("Colors:View", "BackgroundAlternate"),
            text: text_color,
            accent: get("Colors:View", "DecorationFocus")
                .or_else(|| get("Colors:Selection", "BackgroundNormal"))?,
            on_accent: get("Colors:Selection", "ForegroundNormal"),
            positive: get("Colors:View", "ForegroundPositive"),
            negative: get("Colors:View", "ForegroundNegative"),
            neutral: get("Colors:View", "ForegroundNeutral"),
        }
        .into_import(Source::Kde),
    )
}

/// Reads a `qt5ct`/`qt6ct` colour scheme, which is a Qt palette: its `active_colors` run
/// in Qt's own order. `None` when it has none.
pub fn from_qt_palette(text: &str, source: Source) -> Option<Imported> {
    let line = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("active_colors="))?;
    // Qt writes `#aarrggbb`, with the alpha first.
    let colors: Vec<u32> = line
        .split(',')
        .map(|entry| {
            let digits = entry.trim().trim_start_matches('#');
            let rgb = digits.get(digits.len().saturating_sub(6)..).unwrap_or("");
            parse_hex(rgb).unwrap_or(0)
        })
        .collect();
    // Text 6, Base 9, Window 10, Highlight 12, HighlightedText 13, AlternateBase 16.
    let at = |i: usize| colors.get(i).copied();
    Some(
        Scheme {
            base: at(9)?,
            window: at(10)?,
            alternate: at(16),
            text: at(6)?,
            accent: at(12)?,
            on_accent: at(13),
            positive: None,
            negative: None,
            neutral: None,
        }
        .into_import(source),
    )
}

/// Where a colour scheme can be read from. Only colours are ever read from them: no fonts,
/// icons, sizes or other settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Kde,
    Qt6ct,
    Qt5ct,
    Gtk4,
    Gtk3,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::Kde,
        Source::Qt6ct,
        Source::Qt5ct,
        Source::Gtk4,
        Source::Gtk3,
    ];

    /// What it is called in a sentence ("from the KDE colours").
    pub fn name(self) -> &'static str {
        match self {
            Source::Kde => "KDE colours",
            Source::Qt6ct => "qt6ct palette",
            Source::Qt5ct => "qt5ct palette",
            Source::Gtk4 => "GTK 4 theme",
            Source::Gtk3 => "GTK 3 theme",
        }
    }

    /// The button that imports it.
    pub fn button(self) -> String {
        format!("Import {}", self.name())
    }
}

/// The folders a desktop keeps its themes in.
#[derive(Clone, Debug)]
pub struct Locations {
    /// The user's config folder (`~/.config`).
    pub config: PathBuf,
    /// Folders holding installed GTK themes, each with a folder per theme.
    pub themes: Vec<PathBuf>,
}

impl Locations {
    /// Where this system keeps them, or `None` where there is no desktop theme to read
    /// (Windows) or no config folder.
    pub fn this_system() -> Option<Self> {
        if cfg!(windows) {
            return None;
        }
        let config = super::state::config_dir()?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
        let mut themes: Vec<PathBuf> = home.iter().map(|home| home.join(".themes")).collect();
        themes.extend(data.map(|data| data.join("themes")));
        themes.push("/usr/share/themes".into());
        Some(Self { config, themes })
    }

    /// The colours `source` holds here, if it holds any.
    pub fn read(&self, source: Source) -> Option<Imported> {
        let file = |path: PathBuf| std::fs::read_to_string(path).ok();
        match source {
            Source::Kde => from_kde(&file(self.config.join("kdeglobals"))?),
            Source::Qt6ct | Source::Qt5ct => {
                let tool = if source == Source::Qt6ct {
                    "qt6ct"
                } else {
                    "qt5ct"
                };
                let conf = file(self.config.join(tool).join(format!("{tool}.conf")))?;
                from_qt_palette(&file(qt_scheme_path(&conf)?)?, source)
            }
            Source::Gtk4 => self.read_gtk(source, "gtk-4.0"),
            Source::Gtk3 => self.read_gtk(source, "gtk-3.0"),
        }
    }

    /// Which of the sources have colours here, in the order to offer them.
    pub fn found(&self) -> Vec<Source> {
        Source::ALL
            .into_iter()
            .filter(|source| self.read(*source).is_some())
            .collect()
    }

    /// The user's `gtk.css`, else the CSS of the theme `settings.ini` names.
    fn read_gtk(&self, source: Source, folder: &str) -> Option<Imported> {
        let user = self.config.join(folder);
        let mut defined = Defined::default();
        defined.read(&user.join("gtk.css"), 0);
        if let Some(found) = from_gtk(&defined, source) {
            return Some(found);
        }
        let ini = std::fs::read_to_string(user.join("settings.ini")).ok()?;
        let name = ini
            .lines()
            .find_map(|line| line.trim().strip_prefix("gtk-theme-name="))?
            .trim();
        // A name that points out of the themes folder is not a theme.
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return None;
        }
        for themes in &self.themes {
            let mut defined = Defined::default();
            defined.read(&themes.join(name).join(folder).join("gtk.css"), 0);
            if let Some(found) = from_gtk(&defined, source) {
                return Some(found);
            }
        }
        None
    }
}

/// The colours a GTK stylesheet names with `@define-color`.
#[derive(Default)]
struct Defined(BTreeMap<String, String>);

impl Defined {
    /// Reads `@define-color name value;` lines, and the stylesheets `@import`ed beside
    /// them, a few levels deep. Nothing else in the CSS is looked at.
    fn read(&mut self, file: &Path, depth: u32) {
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        let folder = file.parent().unwrap_or(Path::new("."));
        for statement in text.split(';') {
            let statement = statement.trim();
            let statement = statement.rsplit("*/").next().unwrap_or(statement).trim();
            if let Some(rest) = statement.strip_prefix("@define-color") {
                let mut parts = rest.trim().splitn(2, char::is_whitespace);
                if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                    self.0
                        .entry(name.trim().to_owned())
                        .or_insert_with(|| value.trim().to_owned());
                }
            } else if let Some(rest) = statement.strip_prefix("@import")
                && depth < 3
            {
                let target = rest
                    .trim()
                    .trim_start_matches("url(")
                    .trim_end_matches(')')
                    .trim_matches(['"', '\'']);
                // Only a file next to the stylesheet or below it.
                if !target.is_empty()
                    && !target.contains("..")
                    && !target.starts_with('/')
                    && !target.contains("://")
                {
                    self.read(&folder.join(target), depth + 1);
                }
            }
        }
    }

    /// The colour `name` stands for, following `@other` references.
    fn color(&self, name: &str) -> Option<u32> {
        let mut value = self.0.get(name)?.as_str();
        for _ in 0..6 {
            match value.strip_prefix('@') {
                Some(other) => value = self.0.get(other.trim())?.as_str(),
                None => return css_color(value),
            }
        }
        None
    }

    fn first(&self, names: &[&str]) -> Option<u32> {
        names.iter().find_map(|name| self.color(name))
    }
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)` or `rgba(r, g, b, a)`. Anything that has
/// to be worked out (`mix()`, `shade()`) is not understood.
fn css_color(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(digits) = value.strip_prefix('#') {
        return match digits.len() {
            3 => {
                let wide: String = digits.chars().flat_map(|c| [c, c]).collect();
                parse_hex(&wide)
            }
            6 | 8 => parse_hex(&digits[..6]),
            _ => None,
        };
    }
    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let mut parts = inner
        .split([',', '/', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| part.trim().parse::<f32>().ok());
    let mut channel = || Some(parts.next()??.clamp(0., 255.).round() as u32);
    Some(channel()? << 16 | channel()? << 8 | channel()?)
}

/// Reads GTK's named colours. libadwaita (GTK 4) and the older GTK 3 themes name them
/// differently, so each has its own list. `None` when the basics are missing.
fn from_gtk(defined: &Defined, source: Source) -> Option<Imported> {
    let scheme = match source {
        Source::Gtk3 => Scheme {
            base: defined.first(&["theme_base_color", "base_color"])?,
            window: defined.first(&["theme_bg_color", "bg_color"])?,
            alternate: None,
            text: defined.first(&["theme_text_color", "theme_fg_color", "fg_color"])?,
            accent: defined.first(&["theme_selected_bg_color", "selected_bg_color"])?,
            on_accent: defined.first(&["theme_selected_fg_color", "selected_fg_color"]),
            positive: defined.first(&["success_color"]),
            negative: defined.first(&["error_color"]),
            neutral: defined.first(&["warning_color"]),
        },
        _ => Scheme {
            base: defined.first(&["view_bg_color"])?,
            window: defined.first(&["window_bg_color"])?,
            alternate: defined.first(&["card_bg_color", "headerbar_bg_color"]),
            text: defined.first(&["view_fg_color", "window_fg_color"])?,
            accent: defined.first(&["accent_bg_color", "accent_color"])?,
            on_accent: defined.first(&["accent_fg_color"]),
            positive: defined.first(&["success_color", "success_bg_color"]),
            negative: defined.first(&["error_color", "destructive_color"]),
            neutral: defined.first(&["warning_color", "warning_bg_color"]),
        },
    };
    Some(scheme.into_import(source))
}

/// The palette file a `qt5ct`/`qt6ct` config uses, when it uses a custom one.
fn qt_scheme_path(conf: &str) -> Option<PathBuf> {
    let value = |key: &str| {
        conf.lines()
            .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    (value("custom_palette")? == "true")
        .then(|| value("color_scheme_path").map(PathBuf::from))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KDE: &str = "[Colors:View]\nBackgroundNormal=26,27,38\nBackgroundAlternate=36,40,59\nForegroundNormal=192,202,245\nDecorationFocus=122,162,247\nForegroundPositive=158,206,106\nForegroundNegative=247,118,142\nForegroundNeutral=93,108,139\n[Colors:Window]\nBackgroundNormal=22,22,30\n[Colors:Selection]\nForegroundNormal=22,22,30\n";

    #[test]
    fn hex_is_read_with_or_without_a_hash_and_in_any_case() {
        assert_eq!(parse_hex("#1E1e2E"), Some(0x1e1e2e));
        assert_eq!(parse_hex(" 1e1e2e "), Some(0x1e1e2e));
        assert_eq!(parse_hex("#1e1e2"), None);
        assert_eq!(parse_hex("blue!!"), None);
        assert!(check_hex("nope").unwrap_err().contains("nope"));
        assert_eq!(hex(0x1e1e2e), "#1e1e2e");
    }

    #[test]
    fn a_theme_keeps_only_real_changes_and_survives_the_file() {
        let mut theme = Overrides::default();
        theme.set(Slot::Base, 0x1a1b26, true);
        theme.set(Slot::Blue, Slot::Blue.catppuccin(true), true);
        assert_eq!(theme.len(), 1);
        theme.set(Slot::Surface0, 0x292e42, true);
        let text = theme.render();
        assert_eq!(text, "base:1a1b26,surface0:292e42");
        assert_eq!(Overrides::parse(&text), theme);
        assert_eq!(
            Overrides::parse("base:1a1b26,what:ffffff,blue:zzz,,text").len(),
            1
        );
        theme.clear(Slot::Base);
        assert_eq!(theme.color(Slot::Base, true), 0x1e1e2e);
    }

    #[test]
    fn recolouring_swaps_the_designs_colours_and_keeps_alpha() {
        let mut dark = Overrides::default();
        dark.set(Slot::Blue, 0x7aa2f7, true);
        let themes = r##"{"themes":[
            {"name":"d","mode":"dark","colors":{"ring":"#89b4fa","selection.background":"#89b4fa4d","other":"#123456","word":"hello"}},
            {"name":"l","mode":"light","colors":{"ring":"#1e66f5"}}]}"##;
        let out = Overrides::recolor_themes(&dark, &Overrides::default(), themes);
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        let colors = &out["themes"][0]["colors"];
        assert_eq!(colors["ring"], "#7aa2f7");
        assert_eq!(colors["selection.background"], "#7aa2f74d");
        assert_eq!(colors["other"], "#123456");
        assert_eq!(colors["word"], "hello");
        assert_eq!(out["themes"][1]["colors"]["ring"], "#1e66f5");
        assert_eq!(
            Overrides::recolor_themes(&dark, &dark, "not json"),
            "not json"
        );
    }

    #[test]
    fn the_bundled_themes_use_only_colours_the_slots_cover_or_neutral_ones() {
        // Every design colour in the file must be a slot, or changing it would miss spots.
        let themes = include_str!("../../../../src/catppuccin.json");
        let file: serde_json::Value = serde_json::from_str(themes).unwrap();
        for theme in file["themes"].as_array().unwrap() {
            let dark = theme["mode"] == "dark";
            for (name, value) in theme["colors"].as_object().unwrap() {
                let digits = value.as_str().unwrap().trim_start_matches('#');
                let color = parse_hex(&digits[..6]).unwrap();
                let covered = Slot::ALL.iter().any(|s| s.catppuccin(dark) == color);
                assert!(
                    covered || color == 0,
                    "{name} = {digits} in {} is no slot's colour",
                    theme["name"]
                );
            }
        }
    }

    #[test]
    fn a_kde_scheme_becomes_overrides_and_picks_its_appearance() {
        let found = from_kde(KDE).unwrap();
        assert!(found.dark);
        let theme = &found.overrides;
        assert_eq!(theme.color(Slot::Base, true), 0x1a1b26);
        assert_eq!(theme.color(Slot::Mantle, true), 0x16161e);
        assert_eq!(theme.color(Slot::Surface0, true), 0x24283b);
        assert_eq!(theme.color(Slot::Text, true), 0xc0caf5);
        assert_eq!(theme.color(Slot::Blue, true), 0x7aa2f7);
        assert_eq!(theme.color(Slot::Crust, true), 0x16161e);
        assert_eq!(theme.color(Slot::Green, true), 0x9ece6a);
        assert_eq!(theme.color(Slot::Red, true), 0xf7768e);
        // A grey "neutral" can't say paused, so Peach stays as designed.
        assert!(!theme.is_set(Slot::Peach));
        // The lanes are the design's own.
        assert!(!theme.is_set(Slot::Mauve));
    }

    #[test]
    fn a_light_scheme_is_for_the_light_appearance() {
        let text = "[Colors:View]\nBackgroundNormal=250,250,250\nForegroundNormal=30,30,30\nDecorationFocus=0,100,200\n";
        let found = from_kde(text).unwrap();
        assert!(!found.dark);
        // With no separate window colour, the side panels are a step off the background.
        assert!(found.overrides.color(Slot::Mantle, false) > 0xc8c8c8);
        assert!(found.overrides.color(Slot::Mantle, false) < 0xfafafa);
        assert!(from_kde("[General]\nName=x\n").is_none());
    }

    #[test]
    fn a_qt_palette_reads_qts_order() {
        let colors = [
            "#c0caf5", "#16161e", "#000000", "#000000", "#000000", "#000000", "#c0caf5", "#ffffff",
            "#c0caf5", "#1a1b26", "#16161e", "#000000", "#7aa2f7", "#16161e", "#7aa2f7", "#7aa2f7",
            "#24283b",
        ];
        let text = format!("[ColorScheme]\nactive_colors={}\n", colors.join(", "));
        let found = from_qt_palette(&text, Source::Qt6ct).unwrap();
        assert!(found.dark);
        assert_eq!(found.overrides.color(Slot::Base, true), 0x1a1b26);
        assert_eq!(found.overrides.color(Slot::Mantle, true), 0x16161e);
        assert_eq!(found.overrides.color(Slot::Blue, true), 0x7aa2f7);
        assert_eq!(found.overrides.color(Slot::Surface0, true), 0x24283b);
        // Qt writes alpha first.
        let alpha = text.replace("#1a1b26", "#ff1a1b26");
        assert_eq!(from_qt_palette(&alpha, Source::Qt6ct).unwrap(), found);
    }

    fn here(config: &Path, themes: &Path) -> Locations {
        Locations {
            config: config.to_owned(),
            themes: vec![themes.to_owned()],
        }
    }

    #[test]
    fn nothing_is_found_where_no_desktop_has_a_theme() {
        let (config, themes) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert!(here(config.path(), themes.path()).found().is_empty());
    }

    #[test]
    fn kde_and_qt_palettes_are_found_in_the_config_folder() {
        let (home, themes) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let places = here(home.path(), themes.path());
        std::fs::write(home.path().join("kdeglobals"), KDE).unwrap();
        assert_eq!(places.found(), vec![Source::Kde]);
        assert_eq!(places.read(Source::Kde).unwrap().source, Source::Kde);

        let palette = home.path().join("p.conf");
        std::fs::write(&palette, "active_colors=#c0caf5, #16161e, #0, #0, #0, #0, #c0caf5, #0, #0, #1a1b26, #16161e, #0, #7aa2f7, #16161e, #0, #0, #24283b\n").unwrap();
        std::fs::create_dir(home.path().join("qt6ct")).unwrap();
        std::fs::write(
            home.path().join("qt6ct/qt6ct.conf"),
            format!(
                "[Appearance]\ncolor_scheme_path={}\ncustom_palette=true\n",
                palette.display()
            ),
        )
        .unwrap();
        assert_eq!(places.found(), vec![Source::Kde, Source::Qt6ct]);
    }

    const ADWAITA: &str = "@define-color window_bg_color #1e1e2e;\n@define-color view_bg_color #181825;\n@define-color view_fg_color #cdd6f4;\n@define-color accent_bg_color rgb(137, 180, 250);\n@define-color accent_fg_color @window_bg_color;\n@define-color success_color #a6e3a1;\n@define-color error_color #f38ba8;\n";

    #[test]
    fn a_gtk_4_stylesheet_gives_its_named_colours_and_follows_references() {
        let (home, themes) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir(home.path().join("gtk-4.0")).unwrap();
        // The colours may sit in a file the stylesheet imports; other CSS is ignored.
        std::fs::write(
            home.path().join("gtk-4.0/gtk.css"),
            "@import url(\"colors.css\");\nwindow { font-family: serif; background: red; }\n",
        )
        .unwrap();
        std::fs::write(home.path().join("gtk-4.0/colors.css"), ADWAITA).unwrap();
        let places = here(home.path(), themes.path());
        assert_eq!(places.found(), vec![Source::Gtk4]);
        let found = places.read(Source::Gtk4).unwrap();
        assert!(found.dark);
        let theme = &found.overrides;
        assert_eq!(theme.color(Slot::Base, true), 0x181825);
        assert_eq!(theme.color(Slot::Mantle, true), 0x1e1e2e);
        assert_eq!(theme.color(Slot::Text, true), 0xcdd6f4);
        assert_eq!(theme.color(Slot::Blue, true), 0x89b4fa);
        assert_eq!(theme.color(Slot::Crust, true), 0x1e1e2e, "@window_bg_color");
        assert_eq!(theme.color(Slot::Green, true), 0xa6e3a1);
        assert_eq!(theme.color(Slot::Red, true), 0xf38ba8);
    }

    #[test]
    fn the_theme_settings_ini_names_is_read_and_a_path_is_not_followed() {
        let (home, themes) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(home.path().join("gtk-3.0")).unwrap();
        std::fs::write(
            home.path().join("gtk-3.0/settings.ini"),
            "[Settings]\ngtk-theme-name=Night\ngtk-font-name=Serif 40\n",
        )
        .unwrap();
        std::fs::create_dir_all(themes.path().join("Night/gtk-3.0")).unwrap();
        std::fs::write(
            themes.path().join("Night/gtk-3.0/gtk.css"),
            "@define-color theme_base_color #101010;\n@define-color theme_bg_color #202020;\n@define-color theme_text_color #eeeeee;\n@define-color theme_selected_bg_color #3366ff;\n",
        )
        .unwrap();
        let places = here(home.path(), themes.path());
        let found = places.read(Source::Gtk3).unwrap();
        assert_eq!(found.overrides.color(Slot::Base, true), 0x101010);
        assert_eq!(found.overrides.color(Slot::Blue, true), 0x3366ff);

        std::fs::write(
            home.path().join("gtk-3.0/settings.ini"),
            "gtk-theme-name=../Night\n",
        )
        .unwrap();
        assert!(places.read(Source::Gtk3).is_none());
    }

    #[test]
    fn css_colours_in_the_forms_themes_use() {
        assert_eq!(css_color("#fff"), Some(0xffffff));
        assert_eq!(css_color("#11223380"), Some(0x112233));
        assert_eq!(css_color("rgba(1, 2, 3, 0.5)"), Some(0x010203));
        assert_eq!(css_color("rgb(300 0 0)"), Some(0xff0000));
        assert_eq!(css_color("mix(@a, @b, 0.5)"), None);
        assert_eq!(css_color("shade(#fff, 0.8)"), None);
    }

    #[test]
    fn only_colours_are_taken_from_a_scheme() {
        // What an import can change is the theme: the overrides hold slots and colours.
        let found = from_kde(KDE).unwrap();
        let text = found.overrides.render();
        assert!(
            text.split(',')
                .all(|entry| entry.split_once(':').is_some_and(|(key, hex)| {
                    Slot::from_key(key).is_some() && parse_hex(hex).is_some()
                }))
        );
    }
}
