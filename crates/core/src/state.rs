//! What the app remembers between runs: which repositories were open, and the few
//! preferences that belong to the app rather than to git. Git settings and identities
//! stay in git's own config, see `config.rs` and `identity.rs`.
//!
//! The file is plain `key=value` lines, so it can be read and mended by hand. Unknown
//! lines are ignored and kept out of the way of newer versions. Reading never fails: a
//! missing or broken file is the same as a fresh install.

use crate::tree::FileView;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The file's name inside the app's config folder.
const FILE_NAME: &str = "state";
const FOLDER_NAME: &str = "gitclient";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppState {
    /// Whether the repositories that were open come back on launch.
    pub reopen_tabs: bool,
    /// The repositories open when the app last changed, in tab order.
    pub tabs: Vec<PathBuf>,
    /// The position in `tabs` of the tab that was in front.
    pub active: usize,
    /// How the commit screen lists changed files, for every repository.
    pub files_view: FileView,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            reopen_tabs: true,
            tabs: Vec::new(),
            active: 0,
            files_view: FileView::List,
        }
    }
}

/// The tabs to open at launch, and which one comes forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Startup {
    pub tabs: Vec<PathBuf>,
    pub active: usize,
}

impl AppState {
    /// Reads the state at `file`. A missing or unreadable file gives the defaults.
    pub fn load(file: &Path) -> Self {
        std::fs::read_to_string(file)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Writes the state to `file`, making its folder if needed. The file is replaced in
    /// one step, so a crash can't leave half of it.
    pub fn save(&self, file: &Path) -> Result<(), String> {
        let fail = |error: std::io::Error| {
            format!("Couldn't save app settings to {}: {error}", file.display())
        };
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(fail)?;
        }
        let partial = file.with_extension("tmp");
        std::fs::write(&partial, self.render()).map_err(fail)?;
        std::fs::rename(&partial, file).map_err(fail)
    }

    pub fn parse(text: &str) -> Self {
        let mut state = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "reopen_tabs" => state.reopen_tabs = value.trim() != "false",
                "files_view" => state.files_view = FileView::from_key(value),
                "active" => state.active = value.trim().parse().unwrap_or(0),
                "tab" if !value.is_empty() => state.tabs.push(PathBuf::from(value)),
                _ => {}
            }
        }
        state
    }

    pub fn render(&self) -> String {
        let mut text =
            String::from("# Remembered by gitclient. Safe to edit while it is closed.\n");
        text.push_str(&format!("reopen_tabs={}\n", self.reopen_tabs));
        text.push_str(&format!("files_view={}\n", self.files_view.key()));
        text.push_str(&format!("active={}\n", self.active));
        for tab in &self.tabs {
            // A path that isn't text can't be written to a text file, so it isn't kept.
            if let Some(path) = tab.to_str().filter(|path| !path.contains('\n')) {
                text.push_str(&format!("tab={path}\n"));
            }
        }
        text
    }

    /// Which repositories to open at launch. Ones named on the command line win, then
    /// the remembered ones that are still there, then `fallback` (the current folder).
    pub fn startup(&self, given: &[PathBuf], fallback: Option<PathBuf>) -> Startup {
        if !given.is_empty() {
            return Startup {
                tabs: given.to_vec(),
                active: given.len() - 1,
            };
        }
        if self.reopen_tabs {
            let mut tabs: Vec<PathBuf> = Vec::new();
            let mut active = 0;
            for (position, tab) in self.tabs.iter().enumerate() {
                if !tab.is_dir() || tabs.contains(tab) {
                    continue;
                }
                // The tab in front, or the nearest one after it if it is gone.
                if position < self.active {
                    active = tabs.len() + 1;
                }
                tabs.push(tab.clone());
            }
            if !tabs.is_empty() {
                return Startup {
                    active: active.min(tabs.len() - 1),
                    tabs,
                };
            }
        }
        Startup {
            tabs: fallback.into_iter().collect(),
            active: 0,
        }
    }

    /// What the header says after the "Reopen repositories" switch is flipped.
    pub fn reopen_saved_message(&self) -> &'static str {
        if self.reopen_tabs {
            "Saved. gitclient will reopen your repositories when it starts."
        } else {
            "Saved. gitclient will start without reopening repositories."
        }
    }

    /// The sentence under the "Reopen repositories" switch, saying what it does now.
    pub fn reopen_sentence(&self) -> &'static str {
        if self.reopen_tabs {
            "The repositories you leave open come back when you start gitclient."
        } else {
            "gitclient starts with the folder it was launched from, or an empty tab."
        }
    }
}

/// Where the app keeps its state on this system, or `None` if there is no home folder.
pub fn default_file() -> Option<PathBuf> {
    file_in(config_home(|name| std::env::var_os(name))?)
}

fn file_in(home: PathBuf) -> Option<PathBuf> {
    Some(home.join(FOLDER_NAME).join(FILE_NAME))
}

/// The folder for per-user app config: `%APPDATA%`, `~/Library/Application Support`, or
/// `$XDG_CONFIG_HOME` with `~/.config` as its default. `env` looks up a variable.
fn config_home(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| {
        env(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        set("APPDATA")
    } else if cfg!(target_os = "macos") {
        set("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        // The spec says a relative XDG_CONFIG_HOME is to be ignored.
        set("XDG_CONFIG_HOME")
            .filter(|path| path.is_absolute())
            .or_else(|| set("HOME").map(|home| home.join(".config")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let root = tempfile::tempdir().unwrap();
        let paths = names
            .iter()
            .map(|name| {
                let path = root.path().join(name);
                std::fs::create_dir(&path).unwrap();
                path
            })
            .collect();
        (root, paths)
    }

    #[test]
    fn state_survives_a_round_trip_through_a_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("nested").join("state");
        let state = AppState {
            reopen_tabs: false,
            tabs: vec!["/work/a".into(), "/work/with space/b".into()],
            active: 1,
            files_view: FileView::Tree,
        };
        state.save(&file).unwrap();
        assert_eq!(AppState::load(&file), state);
        assert!(
            !file.with_extension("tmp").exists(),
            "no partial file is left"
        );
    }

    #[test]
    fn a_missing_or_broken_file_is_a_fresh_install() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            AppState::load(&root.path().join("nope")),
            AppState::default()
        );
        let state = AppState::parse("garbage\nactive=x\nfuture_key=1\ntab=\nreopen_tabs=true");
        assert_eq!(state, AppState::default());
    }

    #[test]
    fn reopening_is_on_unless_turned_off() {
        assert!(AppState::default().reopen_tabs);
        assert!(!AppState::parse("reopen_tabs=false").reopen_tabs);
    }

    #[test]
    fn the_command_line_beats_what_was_remembered() {
        let (_root, paths) = folders(&["a", "b"]);
        let state = AppState {
            tabs: vec![paths[0].clone()],
            ..Default::default()
        };
        let startup = state.startup(&[paths[1].clone()], None);
        assert_eq!(startup.tabs, vec![paths[1].clone()]);
        assert_eq!(startup.active, 0);
    }

    #[test]
    fn remembered_tabs_come_back_with_the_same_one_in_front() {
        let (_root, paths) = folders(&["a", "b", "c"]);
        let state = AppState {
            tabs: paths.clone(),
            active: 1,
            ..Default::default()
        };
        let startup = state.startup(&[], Some("/cwd".into()));
        assert_eq!((startup.tabs, startup.active), (paths, 1));
    }

    #[test]
    fn folders_that_are_gone_or_repeated_are_dropped_and_the_front_tab_follows() {
        let (root, paths) = folders(&["a", "b", "c"]);
        let gone = root.path().join("gone");
        let state = AppState {
            tabs: vec![
                paths[0].clone(),
                gone,
                paths[1].clone(),
                paths[1].clone(),
                paths[2].clone(),
            ],
            active: 4,
            ..Default::default()
        };
        let startup = state.startup(&[], None);
        assert_eq!(startup.tabs, paths);
        assert_eq!(startup.active, 2, "still on c");

        // When the front tab is gone, the one that followed it comes forward.
        let state = AppState {
            tabs: vec![paths[0].clone(), root.path().join("gone"), paths[1].clone()],
            active: 1,
            ..Default::default()
        };
        assert_eq!(state.startup(&[], None).active, 1);

        // And when it was the last one, the new last comes forward.
        let state = AppState {
            tabs: vec![paths[0].clone(), root.path().join("gone")],
            active: 1,
            ..Default::default()
        };
        assert_eq!(state.startup(&[], None).active, 0);
    }

    #[test]
    fn with_nothing_to_reopen_or_reopening_off_the_current_folder_opens() {
        let (_root, paths) = folders(&["a"]);
        let fallback = Some(PathBuf::from("/cwd"));
        let remembered = AppState {
            tabs: paths,
            reopen_tabs: false,
            ..Default::default()
        };
        assert_eq!(
            remembered.startup(&[], fallback.clone()).tabs,
            vec![PathBuf::from("/cwd")]
        );
        assert_eq!(
            AppState::default().startup(&[], fallback).tabs,
            vec![PathBuf::from("/cwd")]
        );
        assert!(AppState::default().startup(&[], None).tabs.is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_file_lives_under_xdg_config_home_else_dot_config() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert_eq!(
            config_home(env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")])),
            Some("/x".into())
        );
        assert_eq!(
            config_home(env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/h")])),
            Some("/h/.config".into())
        );
        assert_eq!(
            config_home(env(&[("HOME", "/h")])),
            Some("/h/.config".into())
        );
        assert_eq!(config_home(env(&[])), None);
        assert_eq!(
            file_in("/h/.config".into()),
            Some("/h/.config/gitclient/state".into())
        );
    }
}
