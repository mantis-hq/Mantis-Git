//! Themes as files. A theme is one `*.theme` file in the app's `themes` folder, and its file
//! name is its name. The file holds colours and nothing else:
//!
//! ```text
//! # A comment.
//! appearance = dark
//! Base = #1a1b26
//! Surface 0 = #292e42
//! ```
//!
//! A file is read whole or not at all: one bad line leaves the theme that was showing,
//! and the problem names the file and the line. A colour left out stays Catppuccin's.

use super::state;
use super::theme::{Overrides, Slot, appearance_name, builtin_name, check_hex, hex, luminance};
use std::path::{Path, PathBuf};

const EXTENSION: &str = "theme";
const FOLDER: &str = "themes";
/// The longest theme name, so it fits a chip.
const MAX_NAME: usize = 40;

/// The folder themes live in, beside the app's `state` file.
pub fn folder() -> Option<PathBuf> {
    Some(state::default_file()?.parent()?.join(FOLDER))
}

/// A theme read from a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Whether it is for the dark appearance.
    pub dark: bool,
    pub overrides: Overrides,
}

/// A file in the folder: its name, and what reading it gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub theme: Result<Theme, String>,
}

fn file_name(name: &str) -> String {
    format!("{name}.{EXTENSION}")
}

/// Reads a theme file. The error names the line.
pub fn parse(text: &str) -> Result<Theme, String> {
    let mut appearance = None;
    let mut colors: Vec<(Slot, u32)> = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "line {number}: write each colour as Name = #rrggbb."
            ));
        };
        // A note may follow the value: `Base = #1e1e2e  # the graph`.
        let value = value.split_whitespace().next().unwrap_or("");
        let key = key.trim().to_lowercase().replace(' ', "");
        if key == "appearance" {
            appearance = Some(match value.to_lowercase().as_str() {
                "dark" => true,
                "light" => false,
                _ => {
                    return Err(format!(
                        "line {number}: appearance is dark or light, not “{value}”."
                    ));
                }
            });
            continue;
        }
        let Some(slot) = Slot::ALL
            .into_iter()
            .find(|slot| slot.name().to_lowercase().replace(' ', "") == key)
        else {
            return Err(format!(
                "line {number}: “{}” isn't one of the colours. They are {}.",
                line.split('=').next().unwrap_or("").trim(),
                Slot::ALL.map(Slot::name).join(", ")
            ));
        };
        let color = check_hex(value).map_err(|error| format!("line {number}: {error}"))?;
        colors.push((slot, color));
    }
    if colors.is_empty() {
        return Err("it has no colours in it.".to_owned());
    }
    // Without an `appearance`, the background says which one it is for.
    let dark = appearance.unwrap_or_else(|| {
        colors
            .iter()
            .find(|(slot, _)| *slot == Slot::Base)
            .is_none_or(|(_, color)| luminance(*color) < 0.5)
    });
    let mut overrides = Overrides::default();
    for (slot, color) in colors {
        overrides.set(slot, color, dark);
    }
    Ok(Theme { dark, overrides })
}

/// The file for `overrides`: every colour, so it shows the format and can be edited from
/// where it stands.
pub fn render(name: &str, dark: bool, overrides: &Overrides) -> String {
    let mut text = format!(
        "# {name}: a Mantis Git theme. Save this file and the app follows.\n\
         # One colour per line as `Name = #rrggbb`. A line left out keeps {}'s colour.\n\
         appearance = {}\n\n",
        builtin_name(dark),
        appearance_name(dark)
    );
    for slot in Slot::ALL {
        text.push_str(&format!(
            "{:<9} = {}  # {}\n",
            slot.name(),
            hex(overrides.color(slot, dark)),
            slot.role()
        ));
    }
    text
}

/// Every theme file in `dir`, by name. A file that can't be used is listed with its
/// problem, so it is said and not skipped.
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(files) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = files
        .flatten()
        .filter_map(|file| {
            let path = file.path();
            (path.extension()? == EXTENSION && path.is_file()).then_some(path)
        })
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_owned();
            Some(Entry {
                theme: read(&path, &name),
                name,
            })
        })
        .collect();
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries
}

fn read(path: &Path, name: &str) -> Result<Theme, String> {
    let problem = |why: String| format!("{}, {why}", file_name(name));
    if is_builtin_name(name) {
        return Err(problem(
            "that name is the built-in theme's. Rename the file.".to_owned(),
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|error| problem(error.to_string()))?;
    parse(&text).map_err(problem)
}

fn is_builtin_name(name: &str) -> bool {
    [true, false]
        .into_iter()
        .any(|dark| builtin_name(dark).eq_ignore_ascii_case(name.trim()))
}

/// What the header says once a theme was saved.
pub fn saved_message(name: &str) -> String {
    format!(
        "Saved {}. Edit it in any editor and the app follows.",
        file_name(name)
    )
}

/// What the header says once a theme file was picked.
pub fn picked_message(name: &str, dark: bool) -> String {
    format!(
        "{} is the {} appearance now. Edit it in any editor and the app follows.",
        file_name(name),
        appearance_name(dark)
    )
}

/// What a typed theme name means, or why it can't be one.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the theme a name.".to_owned());
    }
    if name.chars().count() > MAX_NAME
        || name.starts_with('.')
        || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "“{name}” can't be a file's name. Use up to {MAX_NAME} letters, digits, spaces or dashes."
        ));
    }
    if is_builtin_name(name) {
        return Err(format!(
            "“{name}” is the built-in theme. Pick another name."
        ));
    }
    Ok(name.to_owned())
}

/// Writes `overrides` as the theme `name` in `dir`, making the folder if needed. A theme
/// that exists is only replaced when `overwrite` says so. Returns the file.
pub fn save(
    dir: &Path,
    name: &str,
    dark: bool,
    overrides: &Overrides,
    overwrite: bool,
) -> Result<PathBuf, String> {
    let name = check_name(name)?;
    let file = dir.join(file_name(&name));
    if file.exists() && !overwrite {
        return Err(format!("There is already a theme called “{name}”."));
    }
    let fail = |error: std::io::Error| format!("Couldn't write {}: {error}", file.display());
    std::fs::create_dir_all(dir).map_err(fail)?;
    let partial = file.with_extension("tmp");
    std::fs::write(&partial, render(&name, dark, overrides)).map_err(fail)?;
    std::fs::rename(&partial, &file).map_err(fail)?;
    Ok(file)
}

/// What is in force for one appearance: the picked theme file if one is picked, else the
/// colours changed on the page (`custom`). The second is what to say, if anything.
///
/// A file that can't be read leaves `previous` showing, so a half-typed save never
/// repaints the window. A picked file that is gone gives the built-in theme back.
pub fn in_force(
    dir: Option<&Path>,
    picked: Option<&str>,
    custom: &Overrides,
    dark: bool,
    previous: &Overrides,
) -> (Overrides, Option<String>) {
    let Some(name) = picked else {
        return (custom.clone(), None);
    };
    let path = dir.map(|dir| dir.join(file_name(name)));
    if !path.as_deref().is_some_and(Path::is_file) {
        return (
            Overrides::default(),
            Some(format!(
                "{} is gone, so {} is showing.",
                file_name(name),
                builtin_name(dark)
            )),
        );
    }
    match path.map(|path| read(&path, name)) {
        Some(Ok(theme)) if theme.dark == dark => (theme.overrides, None),
        Some(Ok(_)) => (
            previous.clone(),
            Some(format!(
                "{} is for the {} appearance.",
                file_name(name),
                appearance_name(!dark)
            )),
        ),
        Some(Err(problem)) => (previous.clone(), Some(problem)),
        None => (previous.clone(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NIGHT: &str = "# night\nappearance = dark\nBase = #1a1b26   # the graph\nsurface 0=#292E42\nBlue = #7aa2f7\n";

    #[test]
    fn a_file_is_read_by_the_designs_names_in_any_case_and_spacing() {
        let theme = parse(NIGHT).unwrap();
        assert!(theme.dark);
        assert_eq!(theme.overrides.color(Slot::Base, true), 0x1a1b26);
        assert_eq!(theme.overrides.color(Slot::Surface0, true), 0x292e42);
        assert_eq!(
            theme.overrides.color(Slot::Text, true),
            0xcdd6f4,
            "left out"
        );
        assert_eq!(theme.overrides.len(), 3);
    }

    #[test]
    fn without_an_appearance_the_background_says_which() {
        assert!(!parse("Base = #fafafa\n").unwrap().dark);
        assert!(parse("Base = #101010\n").unwrap().dark);
        assert!(parse("Blue = #3366ff\n").unwrap().dark);
        assert!(!parse("appearance = light\nBlue = #3366ff\n").unwrap().dark);
    }

    #[test]
    fn a_problem_names_the_line_and_a_bad_file_is_not_half_read() {
        assert!(
            parse("Base = #1a1b26\nBlue = #12345\n")
                .unwrap_err()
                .starts_with("line 2: “#12345”")
        );
        let unknown = parse("\nSurface 5 = #112233\n").unwrap_err();
        assert!(unknown.starts_with("line 2: “Surface 5” isn't one of the colours"));
        assert!(unknown.contains("Flamingo"));
        assert!(
            parse("Base #1a1b26\n")
                .unwrap_err()
                .starts_with("line 1: write each colour")
        );
        assert!(
            parse("appearance = dim\n")
                .unwrap_err()
                .contains("dark or light")
        );
        assert!(
            parse("# only a comment\n")
                .unwrap_err()
                .contains("no colours")
        );
        // Only colours and the appearance exist, so nothing else can be set from a file.
        assert!(parse("font = Serif\nBase = #111111\n").is_err());
    }

    #[test]
    fn a_rendered_file_is_a_complete_palette_that_reads_back_the_same() {
        let theme = parse(NIGHT).unwrap();
        let text = render("night", true, &theme.overrides);
        assert!(text.starts_with("# night: a Mantis Git theme."));
        assert!(text.contains("Surface 0 = #292e42  # borders, selected rows"));
        assert_eq!(
            text.lines()
                .filter(|l| !l.starts_with('#') && l.contains(" = #"))
                .count(),
            21
        );
        assert_eq!(parse(&text).unwrap(), theme);
        let light = render("day", false, &Overrides::default());
        assert!(light.contains("appearance = light") && light.contains("Base      = #eff1f5"));
    }

    #[test]
    fn the_folder_lists_themes_by_name_and_says_what_is_wrong_with_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(&dir.path().join("missing")).is_empty());
        std::fs::write(dir.path().join("night.theme"), NIGHT).unwrap();
        std::fs::write(dir.path().join("Broken.theme"), "Base = nope\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "Base = #111111\n").unwrap();
        std::fs::write(dir.path().join("Catppuccin Mocha.theme"), NIGHT).unwrap();
        let entries = list(dir.path());
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Broken", "Catppuccin Mocha", "night"]);
        assert!(
            entries[0]
                .theme
                .as_ref()
                .unwrap_err()
                .starts_with("Broken.theme, line 1")
        );
        assert!(entries[1].theme.as_ref().unwrap_err().contains("built-in"));
        assert!(entries[2].theme.is_ok());
    }

    #[test]
    fn saving_makes_the_folder_checks_the_name_and_keeps_what_is_there() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("themes");
        let mut colours = Overrides::default();
        colours.set(Slot::Blue, 0x7aa2f7, true);
        let file = save(&dir, " night ", true, &colours, false).unwrap();
        assert_eq!(file, dir.join("night.theme"));
        assert_eq!(list(&dir)[0].theme.as_ref().unwrap().overrides, colours);
        assert!(
            save(&dir, "night", true, &colours, false)
                .unwrap_err()
                .contains("already")
        );
        assert!(save(&dir, "night", true, &Overrides::default(), true).is_ok());
        for bad in [
            "",
            "  ",
            "a/b",
            "..x",
            ".hidden",
            "catppuccin mocha",
            "Catppuccin Latte",
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_name(&"x".repeat(41)).is_err());
    }

    #[test]
    fn what_is_in_force_follows_the_picked_file_and_falls_back_gently() {
        let dir = tempfile::tempdir().unwrap();
        let (dir, none) = (dir.path(), Overrides::default());
        let mut custom = Overrides::default();
        custom.set(Slot::Red, 0xff0000, true);
        let mut before = Overrides::default();
        before.set(Slot::Green, 0x00ff00, true);

        // Nothing picked: the colours changed on the page.
        assert_eq!(
            in_force(Some(dir), None, &custom, true, &before),
            (custom.clone(), None)
        );

        std::fs::write(dir.join("night.theme"), NIGHT).unwrap();
        let (shown, note) = in_force(Some(dir), Some("night"), &custom, true, &before);
        assert_eq!(shown.color(Slot::Blue, true), 0x7aa2f7);
        assert!(note.is_none());

        // A file that is half typed leaves what was showing, and says why.
        std::fs::write(dir.join("night.theme"), "Base = #1a1b26\nBlue = #7aa2f\n").unwrap();
        let (shown, note) = in_force(Some(dir), Some("night"), &custom, true, &before);
        assert_eq!(shown, before);
        assert!(note.unwrap().starts_with("night.theme, line 2"));

        // A theme for the other appearance is not applied to this one.
        std::fs::write(
            dir.join("night.theme"),
            "appearance = light\nBase = #fafafa\n",
        )
        .unwrap();
        let (shown, note) = in_force(Some(dir), Some("night"), &custom, true, &before);
        assert_eq!(shown, before);
        assert!(note.unwrap().contains("light appearance"));

        // A picked file that is gone gives the built-in theme back.
        std::fs::remove_file(dir.join("night.theme")).unwrap();
        let (shown, note) = in_force(Some(dir), Some("night"), &custom, true, &before);
        assert_eq!(shown, none);
        assert_eq!(
            note.unwrap(),
            "night.theme is gone, so Catppuccin Mocha is showing."
        );
        assert_eq!(
            in_force(None, Some("night"), &custom, false, &before).0,
            none
        );
    }
}
