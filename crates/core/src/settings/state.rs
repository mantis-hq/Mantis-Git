//! What the app remembers between runs: which repositories were open, and the few
//! preferences that belong to the app rather than to git. Git settings and identities
//! stay in git's own config, see `config.rs` and `identity.rs` beside this file.
//!
//! The file is plain `key=value` lines, so it can be read and mended by hand. Unknown
//! lines are ignored and kept out of the way of newer versions. Reading never fails: a
//! missing or broken file is the same as a fresh install.

use crate::cache::{DEFAULT_DISK_MB, DiskSettings};
use crate::history::avatar::Style as AvatarStyle;
use crate::history::columns::ColumnOrder;
use crate::install::update::Track;
use crate::text::count;
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
    /// Whether what git said about commits is kept on disk between runs.
    pub disk_cache: bool,
    /// How much that may take, in megabytes.
    pub cache_mb: u32,
    /// The most commits the graph loads from a repository.
    pub commit_limit: u32,
    /// How the commit screen lists changed files, for every repository.
    pub files_view: FileView,
    /// The order of the commit graph's columns, left to right.
    pub graph_columns: ColumnOrder,
    /// What the graph draws on commit dots to say who wrote them.
    pub avatars: AvatarStyle,
    /// A picture the user chose for their own commits, which beats a Gravatar.
    pub own_avatar: Option<PathBuf>,
    /// How the dock along the bottom of the window was left.
    pub dock: DockLayout,
    /// Whether the welcome panel has been shown. It shows once, and holds the choice of
    /// adding gitclient to the application menu; what the menu holds is read from disk.
    pub welcome_seen: bool,
    /// Whether a portable copy looks for a newer release when it starts.
    pub check_updates: bool,
    /// Whether updates are full releases or the nightly pre-release.
    pub update_track: Track,
    /// The folder the last repository was cloned into, which the clone form starts with.
    pub clone_dir: Option<PathBuf>,
}

/// Whether the dock was open, how many terminals it had and which of its tabs was in
/// front, so that it comes back the same way. Terminals come back as fresh shells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DockLayout {
    pub open: bool,
    pub terminals: usize,
    /// 0 is the log, then one per terminal.
    pub tab: usize,
}

impl DockLayout {
    /// The most terminals brought back, so a mangled file can't start hundreds of shells.
    pub const MAX_TERMINALS: usize = 16;

    /// The same layout with its numbers inside what can be brought back.
    pub fn bounded(self) -> Self {
        let terminals = self.terminals.min(Self::MAX_TERMINALS);
        Self {
            open: self.open,
            terminals,
            tab: self.tab.min(terminals),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            reopen_tabs: true,
            tabs: Vec::new(),
            active: 0,
            disk_cache: true,
            cache_mb: DEFAULT_DISK_MB,
            commit_limit: DEFAULT_COMMIT_LIMIT,
            files_view: FileView::List,
            graph_columns: ColumnOrder::default(),
            avatars: AvatarStyle::default(),
            own_avatar: None,
            dock: DockLayout::default(),
            welcome_seen: false,
            check_updates: true,
            update_track: Track::default(),
            clone_dir: None,
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

    /// Reads the file again, applies `change` and saves it, so that whatever else changed
    /// the file meanwhile (the tabs, from the main window) is kept. Returns the new state.
    pub fn change(file: &Path, change: impl FnOnce(&mut Self)) -> Result<Self, String> {
        let mut state = Self::load(file);
        change(&mut state);
        state.save(file)?;
        Ok(state)
    }

    pub fn parse(text: &str) -> Self {
        let mut state = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "reopen_tabs" => state.reopen_tabs = value.trim() != "false",
                "disk_cache" => state.disk_cache = value.trim() != "false",
                "cache_mb" => {
                    state.cache_mb = value.trim().parse().unwrap_or(DEFAULT_DISK_MB);
                }
                "commit_limit" => {
                    state.commit_limit = match value.trim().parse() {
                        Ok(limit) if limit > 0 => limit,
                        _ => DEFAULT_COMMIT_LIMIT,
                    };
                }
                "files_view" => state.files_view = FileView::from_key(value),
                "graph_columns" => state.graph_columns = ColumnOrder::parse(value),
                "avatars" => state.avatars = AvatarStyle::from_key(value),
                "own_avatar" if !value.trim().is_empty() => {
                    state.own_avatar = Some(PathBuf::from(value.trim()));
                }
                "welcome_seen" => state.welcome_seen = value.trim() == "true",
                "check_updates" => state.check_updates = value.trim() != "false",
                "update_track" => state.update_track = Track::from_key(value),
                "clone_dir" if !value.trim().is_empty() => {
                    state.clone_dir = Some(PathBuf::from(value.trim()));
                }
                "dock_open" => state.dock.open = value.trim() == "true",
                "dock_terminals" => state.dock.terminals = value.trim().parse().unwrap_or(0),
                "dock_tab" => state.dock.tab = value.trim().parse().unwrap_or(0),
                "active" => state.active = value.trim().parse().unwrap_or(0),
                "tab" if !value.is_empty() => state.tabs.push(PathBuf::from(value)),
                _ => {}
            }
        }
        state.dock = state.dock.bounded();
        state
    }

    pub fn render(&self) -> String {
        let mut text =
            String::from("# Remembered by gitclient. Safe to edit while it is closed.\n");
        text.push_str(&format!("reopen_tabs={}\n", self.reopen_tabs));
        text.push_str(&format!("disk_cache={}\n", self.disk_cache));
        text.push_str(&format!("cache_mb={}\n", self.cache_mb));
        text.push_str(&format!("commit_limit={}\n", self.commit_limit));
        text.push_str(&format!("files_view={}\n", self.files_view.key()));
        text.push_str(&format!("graph_columns={}\n", self.graph_columns.render()));
        text.push_str(&format!("avatars={}\n", self.avatars.key()));
        if let Some(file) = self.own_avatar.as_deref().and_then(Path::to_str)
            && !file.contains('\n')
        {
            text.push_str(&format!("own_avatar={file}\n"));
        }
        text.push_str(&format!("welcome_seen={}\n", self.welcome_seen));
        text.push_str(&format!("check_updates={}\n", self.check_updates));
        text.push_str(&format!("update_track={}\n", self.update_track.key()));
        if let Some(dir) = self.clone_dir.as_deref().and_then(Path::to_str)
            && !dir.contains('\n')
        {
            text.push_str(&format!("clone_dir={dir}\n"));
        }
        text.push_str(&format!("dock_open={}\n", self.dock.open));
        text.push_str(&format!("dock_terminals={}\n", self.dock.terminals));
        text.push_str(&format!("dock_tab={}\n", self.dock.tab));
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

    /// The settings the caches are run with.
    pub fn disk_settings(&self) -> DiskSettings {
        DiskSettings::from_mb(self.disk_cache, self.cache_mb)
    }

    /// The sentence under the cache switch, saying what it does now.
    pub fn disk_cache_sentence(&self) -> String {
        if self.disk_cache {
            format!(
                "What git says about commits is kept on disk, up to {}, so that a commit you \
                 have seen opens at once.",
                cache_size_label(self.cache_mb)
            )
        } else {
            "Commits are read from git each time you open gitclient.".to_owned()
        }
    }

    /// What the header says after the cache switch is flipped.
    pub fn disk_cache_saved_message(&self) -> String {
        if self.disk_cache {
            format!(
                "Saved. Commits you open are kept on disk, up to {}.",
                cache_size_label(self.cache_mb)
            )
        } else {
            "Saved. Commits are no longer kept on disk.".to_owned()
        }
    }

    /// What the header says after the cache size is changed.
    pub fn cache_size_saved_message(&self) -> String {
        format!(
            "Saved. The cache may take up to {} on disk.",
            cache_size_label(self.cache_mb)
        )
    }

    /// What the header says after the cache is cleared.
    pub fn cache_cleared_message() -> &'static str {
        "Cleared the cache. Commits are read from git again as you open them."
    }

    /// What the header says after the "Reopen repositories" switch is flipped.
    pub fn reopen_saved_message(&self) -> &'static str {
        if self.reopen_tabs {
            "Saved. gitclient will reopen your repositories when it starts."
        } else {
            "Saved. gitclient will start without reopening repositories."
        }
    }

    /// The sentence under the commit limit, saying what it does now.
    pub fn commit_limit_sentence(&self) -> String {
        format!(
            "The graph loads the newest {} of a repository, and the next as many as you scroll \
             down to them. A change applies the next time a repository loads.",
            count(self.commit_limit, "commit")
        )
    }

    /// What the header says after the commit limit is changed.
    pub fn commit_limit_saved_message(&self) -> String {
        format!(
            "Saved. The graph loads {} at a time from now on.",
            count(self.commit_limit, "commit")
        )
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

/// How many commits the graph loads unless the settings say otherwise.
pub const DEFAULT_COMMIT_LIMIT: u32 = 500;

/// The commit limits offered in the settings.
pub const COMMIT_LIMITS: [u32; 5] = [100, 500, 1000, 5000, 20000];

/// The sizes offered for the disk cache, in megabytes.
pub const CACHE_SIZES_MB: [u32; 4] = [64, 256, 1024, 4096];

/// A size in megabytes in words, such as `256 MB` or `1 GB`.
pub fn cache_size_label(megabytes: u32) -> String {
    if megabytes >= 1024 && megabytes.is_multiple_of(1024) {
        format!("{} GB", megabytes / 1024)
    } else {
        format!("{megabytes} MB")
    }
}

/// How much is on disk, in words, for the settings page.
pub fn bytes_label(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes < MB {
        "under 1 MB".to_owned()
    } else if bytes < 1024 * MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{:.1} GB", bytes as f64 / (1024 * MB) as f64)
    }
}

/// Where the app keeps its state on this system, or `None` if there is no home folder.
pub fn default_file() -> Option<PathBuf> {
    file_in(config_home(|name| std::env::var_os(name))?)
}

fn file_in(home: PathBuf) -> Option<PathBuf> {
    Some(home.join(FOLDER_NAME).join(FILE_NAME))
}

/// The folder for per-user app config, as it is on this system.
pub fn config_dir() -> Option<PathBuf> {
    config_home(|name| std::env::var_os(name))
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
    fn avatars_are_initials_unless_the_file_says_otherwise() {
        assert_eq!(AppState::default().avatars, AvatarStyle::Initials);
        assert_eq!(
            AppState::parse("avatars=gravatar\n").avatars,
            AvatarStyle::Gravatar
        );
        assert_eq!(AppState::parse("avatars=off\n").avatars, AvatarStyle::Off);
        assert_eq!(
            AppState::parse("avatars=what\n").avatars,
            AvatarStyle::Initials
        );
        assert_eq!(AppState::default().own_avatar, None);
    }

    #[test]
    fn the_chosen_avatar_survives_a_round_trip() {
        let state = AppState {
            avatars: AvatarStyle::Gravatar,
            own_avatar: Some(PathBuf::from("/home/ada/me.png")),
            ..AppState::default()
        };
        assert_eq!(AppState::parse(&state.render()), state);
        assert_eq!(AppState::parse("own_avatar=\n").own_avatar, None);
    }

    #[test]
    fn the_commit_limit_is_500_unless_the_file_says_otherwise() {
        assert_eq!(AppState::default().commit_limit, 500);
        assert_eq!(AppState::parse("commit_limit=1000\n").commit_limit, 1000);
        assert_eq!(AppState::parse("commit_limit=0\n").commit_limit, 500);
        assert_eq!(AppState::parse("commit_limit=many\n").commit_limit, 500);
    }

    #[test]
    fn the_disk_cache_is_on_at_256_mb_unless_the_file_says_otherwise() {
        let fresh = AppState::parse("");
        assert!(fresh.disk_cache);
        assert_eq!(fresh.cache_mb, 256);
        assert_eq!(fresh.disk_settings().max_bytes, 256 * 1024 * 1024);

        let set = AppState::parse("disk_cache=false\ncache_mb=64\n");
        assert!(!set.disk_cache && !set.disk_settings().enabled);
        assert_eq!(set.cache_mb, 64);
        // Nonsense falls back to the default instead of failing.
        assert_eq!(AppState::parse("cache_mb=lots").cache_mb, 256);
    }

    #[test]
    fn sizes_are_said_in_words() {
        assert_eq!(cache_size_label(256), "256 MB");
        assert_eq!(cache_size_label(1024), "1 GB");
        assert_eq!(bytes_label(10), "under 1 MB");
        assert_eq!(bytes_label(42 * 1024 * 1024), "42 MB");
        assert_eq!(bytes_label(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn state_survives_a_round_trip_through_a_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("nested").join("state");
        let state = AppState {
            reopen_tabs: false,
            tabs: vec!["/work/a".into(), "/work/with space/b".into()],
            active: 1,
            disk_cache: false,
            cache_mb: 1024,
            commit_limit: 1000,
            files_view: FileView::Tree,
            graph_columns: ColumnOrder::parse("date,subject"),
            avatars: AvatarStyle::Gravatar,
            own_avatar: Some("/home/ada/me.png".into()),
            dock: DockLayout {
                open: true,
                terminals: 2,
                tab: 1,
            },
            welcome_seen: true,
            check_updates: false,
            update_track: Track::Nightly,
            clone_dir: Some("/work/clones".into()),
        };
        state.save(&file).unwrap();
        assert_eq!(AppState::load(&file), state);
        assert!(
            !file.with_extension("tmp").exists(),
            "no partial file is left"
        );
    }

    #[test]
    fn the_welcome_shows_once_and_updates_are_checked_unless_turned_off() {
        let fresh = AppState::parse("");
        assert!(!fresh.welcome_seen);
        assert!(fresh.check_updates);
        assert!(AppState::parse("welcome_seen=true").welcome_seen);
        // Anything but a plain "true" doesn't count as having been welcomed.
        assert!(!AppState::parse("welcome_seen=yes").welcome_seen);
        assert!(!AppState::parse("check_updates=false").check_updates);
        assert!(AppState::parse("check_updates=maybe").check_updates);
    }

    #[test]
    fn updates_follow_full_releases_unless_nightly_is_chosen() {
        assert_eq!(AppState::parse("").update_track, Track::Stable);
        assert_eq!(
            AppState::parse("update_track=nightly").update_track,
            Track::Nightly
        );
        assert_eq!(
            AppState::parse("update_track=garbage").update_track,
            Track::Stable
        );
    }

    #[test]
    fn changing_one_setting_keeps_what_another_window_saved_meanwhile() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("state");
        AppState {
            tabs: vec!["/work/a".into()],
            ..AppState::default()
        }
        .save(&file)
        .unwrap();

        let state = AppState::change(&file, |state| state.welcome_seen = true).unwrap();

        assert!(state.welcome_seen);
        let saved = AppState::load(&file);
        assert!(saved.welcome_seen);
        assert_eq!(saved.tabs, [PathBuf::from("/work/a")]);
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
    fn the_dock_starts_closed_and_a_mangled_layout_is_kept_in_bounds() {
        assert_eq!(AppState::default().dock, DockLayout::default());
        let dock = AppState::parse("dock_open=true\ndock_terminals=2\ndock_tab=2\n").dock;
        assert_eq!(
            dock,
            DockLayout {
                open: true,
                terminals: 2,
                tab: 2
            }
        );
        // A tab past the last terminal falls back to the last one, and nonsense to none.
        assert_eq!(AppState::parse("dock_terminals=1\ndock_tab=5").dock.tab, 1);
        assert_eq!(AppState::parse("dock_terminals=lots").dock.terminals, 0);
        assert_eq!(
            AppState::parse("dock_terminals=900").dock.terminals,
            DockLayout::MAX_TERMINALS
        );
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
