//! The user's git config, as a short list of settings worth changing rather than every
//! key git knows. This file says which settings there are, what they are called, and how
//! to read and write them. A view only draws what it finds here.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::ops::{git_anywhere, run_command};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which config file is being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The user's own config, applying to every repository.
    Global,
    /// One repository's own config, which wins over the global one.
    Repository(PathBuf),
    /// One file, for tests.
    File(PathBuf),
}

impl Scope {
    /// What a person calls this config.
    pub fn name(&self) -> &'static str {
        match self {
            Scope::Global => "global git config",
            Scope::Repository(_) => "git config of this repository",
            Scope::File(_) => "git config file",
        }
    }

    /// The button that removes a value, named by its result.
    pub fn reset_label(&self) -> &'static str {
        match self {
            Scope::Repository(_) => "Inherit",
            _ => "Use default",
        }
    }

    /// The choice that leaves a setting to something else: git, or the global config.
    pub fn reset_label_for_choice(&self) -> &'static str {
        match self {
            Scope::Repository(_) => "Inherit",
            _ => "Default",
        }
    }

    pub fn reset_hint(&self) -> &'static str {
        match self {
            Scope::Repository(_) => "Remove it here, so the global config decides",
            _ => "Remove it from your config, so git decides",
        }
    }

    /// How git names this scope in `--show-scope`.
    fn git_scope(&self) -> Option<&'static str> {
        match self {
            Scope::Global => Some("global"),
            Scope::Repository(_) => Some("local"),
            Scope::File(_) => None,
        }
    }

    fn command(&self) -> Command {
        let mut command = git_anywhere();
        match self {
            Scope::Global => command.args(["config", "--global"]),
            Scope::Repository(repo) => command.arg("-C").arg(repo).args(["config", "--local"]),
            Scope::File(path) => command.arg("config").arg("--file").arg(path),
        };
        command
    }

    pub(crate) fn run(&self, args: &[&str]) -> Result<String, String> {
        run_command(self.command().args(args), None)
    }
}

/// The version of the installed git.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32);

impl Version {
    pub fn parse(output: &str) -> Option<Version> {
        let number = output.trim().strip_prefix("git version ")?;
        let mut parts = number.split('.').map(|part| part.parse::<u32>());
        Some(Version(parts.next()?.ok()?, parts.next()?.ok()?))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.0, self.1)
    }
}

/// Asks the installed git for its version. Fails with a sentence when there is none,
/// which a window launched from a menu, with a short `PATH`, can easily meet.
pub fn git_version() -> Result<Version, String> {
    let output = run_command(git_anywhere().arg("--version"), None).map_err(|_| {
        "Git wasn't found. Install git, or start gitclient from a place where git is on the PATH."
            .to_owned()
    })?;
    Version::parse(&output).ok_or_else(|| format!("Couldn't tell which git this is: {output}"))
}

/// How a setting is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Free text. Empty means unset.
    Text { placeholder: &'static str },
    /// On or off, with what git does when nothing is set.
    Toggle { default: bool },
    /// One of a few values, or none to leave it to git.
    Choice {
        options: &'static [(&'static str, &'static str)],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub key: &'static str,
    /// For a toggle, the result of turning it on.
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: Kind,
    /// The first git that knows this setting, so an older one isn't offered it.
    pub since: Version,
}

impl Setting {
    const fn since(mut self, major: u32, minor: u32) -> Self {
        self.since = Version(major, minor);
        self
    }

    /// Why this can't be used with `git`, if it can't.
    pub fn unavailable(&self, git: Version) -> Option<String> {
        (git < self.since).then(|| format!("Needs git {} or newer. You have {git}.", self.since))
    }
}

/// Whether `git` knows `value` as a choice for `key`.
pub fn option_available(key: &str, value: &str, git: Version) -> bool {
    match (key, value) {
        ("merge.conflictStyle", "zdiff3") => git >= Version(2, 35),
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub settings: &'static [Setting],
}

const fn text(
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    placeholder: &'static str,
) -> Setting {
    Setting {
        key,
        label,
        hint,
        kind: Kind::Text { placeholder },
        since: Version(0, 0),
    }
}

const fn toggle(key: &'static str, label: &'static str, hint: &'static str) -> Setting {
    Setting {
        key,
        label,
        hint,
        kind: Kind::Toggle { default: false },
        since: Version(0, 0),
    }
}

const fn choice(
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    options: &'static [(&'static str, &'static str)],
) -> Setting {
    Setting {
        key,
        label,
        hint,
        kind: Kind::Choice { options },
        since: Version(0, 0),
    }
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Identity",
        settings: &[
            text(
                "user.name",
                "Name",
                "Shown as the author of your commits.",
                "Ada Lovelace",
            ),
            text(
                "user.email",
                "Email",
                "Shown next to your name on every commit.",
                "ada@example.com",
            ),
            text(
                "user.signingkey",
                "Signing key",
                "The key that signs commits and tags.",
                "Key ID or path",
            ),
            toggle(
                "commit.gpgsign",
                "Sign every commit",
                "Needs a signing key.",
            ),
        ],
    },
    Section {
        title: "Branches",
        settings: &[
            text(
                "init.defaultBranch",
                "First branch of a new repository",
                "Used by git init and clone of empty repositories.",
                "master",
            ),
            choice(
                "pull.rebase",
                "When a pull finds new commits",
                "How your commits and the incoming ones are joined.",
                &[("false", "Merge"), ("true", "Rebase")],
            ),
            choice(
                "push.default",
                "What a plain push sends",
                "Which branch git push updates with no branch given.",
                &[
                    ("simple", "Same name upstream"),
                    ("current", "Same name"),
                    ("upstream", "Tracked branch"),
                ],
            ),
            toggle(
                "push.autoSetupRemote",
                "Track a new branch on its first push",
                "Saves the --set-upstream the first time.",
            )
            .since(2, 37),
            toggle(
                "fetch.prune",
                "Forget remote branches that were deleted",
                "Applies whenever you fetch or pull.",
            ),
        ],
    },
    Section {
        title: "Merging",
        settings: &[
            choice(
                "merge.conflictStyle",
                "Conflict markers show",
                "With the base, you can see what both sides started from.",
                &[
                    ("merge", "Two sides"),
                    ("diff3", "Both and base"),
                    ("zdiff3", "Both and base, trimmed"),
                ],
            ),
            toggle(
                "rerere.enabled",
                "Remember how conflicts were resolved",
                "The same conflict is resolved for you the next time.",
            ),
            toggle(
                "rebase.autoStash",
                "Stash changes around a rebase",
                "So a rebase can start with uncommitted work.",
            ),
        ],
    },
    Section {
        title: "Files",
        settings: &[
            text(
                "core.editor",
                "Editor for commit messages",
                "The command git opens when it needs you to write.",
                "code --wait",
            ),
            choice(
                "core.autocrlf",
                "Line endings",
                "Windows endings only matter if you share files with Windows.",
                &[
                    ("false", "Leave as they are"),
                    ("input", "LF when committing"),
                    ("true", "CRLF when checking out"),
                ],
            ),
            text(
                "core.excludesFile",
                "Ignore file for every repository",
                "Patterns here are ignored everywhere, like .gitignore.",
                "~/.gitignore",
            ),
        ],
    },
];

/// Every setting, grouped for the page that edits them.
pub fn sections() -> &'static [Section] {
    SECTIONS
}

/// An alias, which lets `git <name>` run a longer command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    pub command: String,
}

/// What is set in a config file. Keys are lower case, as git lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Values {
    entries: Vec<(String, String)>,
}

impl Values {
    /// The value of `key`, or the last one if it is set more than once.
    pub fn get(&self, key: &str) -> Option<&str> {
        let key = normalize_key(key);
        self.entries
            .iter()
            .rev()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Every value of `key`, for those that can be set more than once, in file order.
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        let key = normalize_key(key);
        self.entries
            .iter()
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    /// Every key and value, with the key as `normalize_key` writes it.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// Whether a toggle is on, counting what git does when it is unset.
    pub fn is_on(&self, setting: &Setting) -> bool {
        let default = matches!(setting.kind, Kind::Toggle { default: true });
        self.get(setting.key).map_or(default, parse_bool)
    }

    /// The aliases, in name order.
    pub fn aliases(&self) -> Vec<Alias> {
        let mut aliases: Vec<Alias> = Vec::new();
        for (key, value) in &self.entries {
            let Some(name) = key.strip_prefix("alias.") else {
                continue;
            };
            // A name set twice means the later one.
            aliases.retain(|alias| alias.name != name);
            aliases.push(Alias {
                name: name.to_owned(),
                command: value.clone(),
            });
        }
        aliases.sort_by(|a, b| a.name.cmp(&b.name));
        aliases
    }
}

/// A key as git compares it: the section and the name in lower case, and whatever sits
/// between them, such as a remote's name or an `includeIf` condition, exactly as written.
pub(crate) fn normalize_key(key: &str) -> String {
    match (key.find('.'), key.rfind('.')) {
        (Some(first), Some(last)) if first != last => format!(
            "{}{}{}",
            key[..first].to_ascii_lowercase(),
            &key[first..=last],
            key[last + 1..].to_ascii_lowercase()
        ),
        _ => key.to_ascii_lowercase(),
    }
}

/// The user's home folder, on any system.
pub(crate) fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// How git reads a boolean: `yes`, `on`, `1` and `true` are on, all else is off.
fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}

/// Reads everything set in `scope`. A missing file is an empty config.
pub fn read(scope: &Scope) -> Result<Values, String> {
    let output = match scope.run(&["--list", "-z"]) {
        Ok(output) => output,
        Err(message) if missing_file(&message) => String::new(),
        Err(message) => return Err(message),
    };
    Ok(parse(&output))
}

/// Where one value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Git's name for the config: `system`, `global`, `local`, `worktree` or `command`.
    pub scope: String,
    /// `file:` and a path, or what git says for the command line.
    pub origin: String,
    pub value: String,
}

/// Every value git would use here, from all the configs it reads, with where each came
/// from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    entries: Vec<(String, Source)>,
}

impl Sources {
    /// The value git uses for `key`: the last one set.
    pub fn get(&self, key: &str) -> Option<&Source> {
        let key = normalize_key(key);
        self.entries
            .iter()
            .rev()
            .find(|(name, _)| *name == key)
            .map(|(_, source)| source)
    }

    /// Whether a toggle is on for git, wherever that was decided.
    pub fn is_on(&self, setting: &Setting) -> bool {
        let default = matches!(setting.kind, Kind::Toggle { default: true });
        self.get(setting.key)
            .map_or(default, |source| parse_bool(&source.value))
    }

    /// The value `key` has from a config other than `scope`'s own, to show when
    /// `scope` leaves it unset, with a sentence saying where it comes from.
    pub fn inherited(&self, key: &str, own: &Values, scope: &Scope) -> Option<(String, String)> {
        if own.is_set(key) {
            return None;
        }
        let source = self.get(key)?;
        if Some(source.scope.as_str()) == scope.git_scope() {
            return None;
        }
        let from = match source.scope.as_str() {
            "system" => "the system config",
            "global" => "your global config",
            "local" => "this repository's config",
            "worktree" => "this worktree's config",
            _ => "the command line",
        };
        Some((source.value.clone(), format!("Using {from}.")))
    }
}

/// Reads what git would use in `scope`. For a repository that is everything it reads, so
/// that a setting it leaves alone can show what it inherits.
pub fn read_sources(scope: &Scope) -> Result<Sources, String> {
    let mut command = match scope {
        Scope::Repository(repo) => {
            // `--local` would show only this file, and includes need a repository to
            // decide whether they apply.
            let mut command = git_anywhere();
            command.arg("-C").arg(repo).arg("config").arg("--includes");
            command
        }
        _ => scope.command(),
    };
    command.args(["--list", "--show-origin", "--show-scope", "-z"]);
    let output = match run_command(&mut command, None) {
        Ok(output) => output,
        Err(message) if missing_file(&message) => String::new(),
        Err(message) => return Err(message),
    };
    Ok(parse_sources(&output))
}

/// Parses `--show-scope --show-origin -z`: scope, origin, then `key`, a newline and the
/// value, each ended by a NUL.
pub(crate) fn parse_sources(output: &str) -> Sources {
    let parts: Vec<&str> = output.split('\0').collect();
    let entries = parts
        .as_chunks::<3>()
        .0
        .iter()
        .map(|chunk| {
            let (key, value) = chunk[2].split_once('\n').unwrap_or((chunk[2], "true"));
            (
                normalize_key(key),
                Source {
                    scope: chunk[0].to_owned(),
                    origin: chunk[1].to_owned(),
                    value: value.to_owned(),
                },
            )
        })
        .collect();
    Sources { entries }
}

/// Whether git's error means the file isn't there, so nothing is set.
fn missing_file(message: &str) -> bool {
    message.contains("No such file")
        || message.contains("cannot read")
        // Listing a config that doesn't exist exits without saying anything.
        || message.starts_with("git failed without")
}

/// Where the global config file is, if there is one. Git decides, so this is right on
/// every system.
pub fn config_directory(scope: &Scope) -> Result<PathBuf, String> {
    match scope {
        Scope::File(path) => Ok(path.parent().map_or_else(PathBuf::new, Path::to_path_buf)),
        Scope::Global => {
            let output = Scope::Global.run(&["--list", "--show-origin", "-z"])?;
            let origin = output
                .split('\0')
                .next()
                .and_then(|entry| entry.strip_prefix("file:"))
                .ok_or("There is no global git config file yet.")?;
            Path::new(origin)
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| "The global git config has no folder.".to_owned())
        }
        Scope::Repository(_) => Err("A repository's config has no place for identities.".into()),
    }
}

/// Parses `git config --list -z`: `key`, a newline, the value, then a NUL.
fn parse(output: &str) -> Values {
    let entries = output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry.split_once('\n') {
            Some((key, value)) => (normalize_key(key), value.to_owned()),
            // A key written with no `=` has no value at all, which git reads as on.
            None => (normalize_key(entry), "true".to_owned()),
        })
        .collect();
    Values { entries }
}

/// Sets `key`, or removes it when `value` is empty, so that git's own default applies.
pub fn set(scope: &Scope, key: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return unset(scope, key);
    }
    scope.run(&[key, value]).map(drop)
}

/// Removes `key`. A key that isn't there is already removed.
pub fn unset(scope: &Scope, key: &str) -> Result<(), String> {
    match scope.run(&["--unset", key]) {
        Ok(_) => Ok(()),
        Err(message) => {
            if read(scope)?.is_set(key) {
                Err(message)
            } else {
                Ok(())
            }
        }
    }
}

/// Why `name` can't be an alias, if it can't.
pub fn alias_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("Give the alias a name.")
    } else if name.chars().any(|c| c.is_whitespace() || c == '.') {
        Some("An alias name can't contain spaces or dots.")
    } else if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some("Use letters, numbers, - and _ in an alias name.")
    } else {
        None
    }
}

/// Adds or replaces the alias `name`.
pub fn set_alias(scope: &Scope, name: &str, command: &str) -> Result<(), String> {
    if let Some(problem) = alias_problem(name) {
        return Err(problem.to_owned());
    }
    if command.trim().is_empty() {
        return Err("Say what git ".to_owned() + name + " should run.");
    }
    set(scope, &format!("alias.{name}"), command)
}

pub fn remove_alias(scope: &Scope, name: &str) -> Result<(), String> {
    unset(scope, &format!("alias.{name}"))
}

/// The line that says a change landed. `value` is `None` after a reset.
pub fn saved_message(key: &str, value: Option<&str>) -> String {
    match value {
        Some(value) if !value.trim().is_empty() => format!("Saved {key} as {}.", value.trim()),
        _ => format!("Cleared {key}, so git uses its own default."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn scope() -> (TempDir, Scope) {
        let dir = TempDir::new().unwrap();
        let scope = Scope::File(dir.path().join("gitconfig"));
        (dir, scope)
    }

    #[test]
    fn nothing_is_set_before_the_file_exists() {
        let (_dir, scope) = scope();
        assert_eq!(read(&scope).unwrap(), Values::default());
    }

    #[test]
    fn a_value_reads_back_and_empty_removes_it() {
        let (_dir, scope) = scope();
        set(&scope, "user.name", "  Ada  ").unwrap();
        assert_eq!(read(&scope).unwrap().get("user.name"), Some("Ada"));
        set(&scope, "user.name", "").unwrap();
        assert!(!read(&scope).unwrap().is_set("user.name"));
    }

    #[test]
    fn keys_match_whatever_their_case() {
        let (_dir, scope) = scope();
        set(&scope, "init.defaultBranch", "main").unwrap();
        let values = read(&scope).unwrap();
        assert_eq!(values.get("init.defaultBranch"), Some("main"));
        assert_eq!(values.get("INIT.DEFAULTBRANCH"), Some("main"));
    }

    #[test]
    fn removing_what_is_not_set_is_fine() {
        let (_dir, scope) = scope();
        unset(&scope, "user.email").unwrap();
        set(&scope, "user.name", "Ada").unwrap();
        unset(&scope, "user.email").unwrap();
        assert_eq!(read(&scope).unwrap().get("user.name"), Some("Ada"));
    }

    #[test]
    fn multiline_values_survive() {
        let (_dir, scope) = scope();
        set(&scope, "alias.both", "!echo one\necho two").unwrap();
        assert_eq!(
            read(&scope).unwrap().get("alias.both"),
            Some("!echo one\necho two")
        );
    }

    #[test]
    fn toggles_use_gits_own_reading_and_default() {
        let (_dir, scope) = scope();
        let setting = sections()
            .iter()
            .flat_map(|section| section.settings)
            .find(|setting| setting.key == "fetch.prune")
            .unwrap();
        assert!(!read(&scope).unwrap().is_on(setting));
        for (written, on) in [
            ("true", true),
            ("yes", true),
            ("false", false),
            ("0", false),
        ] {
            set(&scope, setting.key, written).unwrap();
            assert_eq!(read(&scope).unwrap().is_on(setting), on, "{written}");
        }
    }

    #[test]
    fn a_key_with_no_equals_sign_is_on() {
        let (dir, scope) = scope();
        std::fs::write(dir.path().join("gitconfig"), "[fetch]\n\tprune\n").unwrap();
        assert_eq!(read(&scope).unwrap().get("fetch.prune"), Some("true"));
    }

    #[test]
    fn aliases_list_by_name_and_validate() {
        let (_dir, scope) = scope();
        set_alias(&scope, "st", "status -sb").unwrap();
        set_alias(&scope, "co", "checkout").unwrap();
        set_alias(&scope, "co", "checkout --").unwrap();
        let aliases = read(&scope).unwrap().aliases();
        assert_eq!(
            aliases,
            [
                Alias {
                    name: "co".into(),
                    command: "checkout --".into()
                },
                Alias {
                    name: "st".into(),
                    command: "status -sb".into()
                },
            ]
        );
        remove_alias(&scope, "co").unwrap();
        assert_eq!(read(&scope).unwrap().aliases().len(), 1);

        assert!(set_alias(&scope, "", "x").is_err());
        assert!(set_alias(&scope, "a b", "x").is_err());
        assert!(set_alias(&scope, "a.b", "x").is_err());
        assert!(set_alias(&scope, "ok", "  ").is_err());
    }

    #[test]
    fn every_setting_has_a_unique_key_and_choices_to_pick() {
        let mut keys: Vec<String> = sections()
            .iter()
            .flat_map(|section| section.settings)
            .map(|setting| setting.key.to_ascii_lowercase())
            .collect();
        let total = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), total);
        for setting in sections().iter().flat_map(|section| section.settings) {
            if let Kind::Choice { options } = setting.kind {
                assert!(options.len() >= 2, "{}", setting.key);
            }
        }
    }

    #[test]
    fn versions_are_read_from_whatever_git_prints() {
        assert_eq!(Version::parse("git version 2.43.0\n"), Some(Version(2, 43)));
        assert_eq!(
            Version::parse("git version 2.39.3 (Apple Git-146)"),
            Some(Version(2, 39))
        );
        assert_eq!(
            Version::parse("git version 2.45.1.windows.1"),
            Some(Version(2, 45))
        );
        assert_eq!(Version::parse("hello"), None);
        assert!(Version(2, 9) < Version(2, 35));
        assert!(git_version().is_ok(), "the tests run with git installed");
    }

    #[test]
    fn settings_a_git_is_too_old_for_say_so() {
        let setting = sections()
            .iter()
            .flat_map(|section| section.settings)
            .find(|setting| setting.key == "push.autoSetupRemote")
            .unwrap();
        assert!(setting.unavailable(Version(2, 37)).is_none());
        let reason = setting.unavailable(Version(2, 30)).unwrap();
        assert_eq!(reason, "Needs git 2.37 or newer. You have 2.30.");
        assert!(!option_available(
            "merge.conflictStyle",
            "zdiff3",
            Version(2, 30)
        ));
        assert!(option_available(
            "merge.conflictStyle",
            "diff3",
            Version(2, 30)
        ));
    }

    #[test]
    fn only_the_section_and_name_of_a_key_ignore_case() {
        assert_eq!(normalize_key("User.Name"), "user.name");
        assert_eq!(
            normalize_key("includeIf.gitdir:~/Work/.Path"),
            "includeif.gitdir:~/Work/.path"
        );
        assert_eq!(normalize_key("remote.Origin.URL"), "remote.Origin.url");
    }

    #[test]
    fn a_repository_inherits_what_it_leaves_unset() {
        let output = [
            "global\0file:/home/u/.config/git/config\0user.name\nAda\0",
            "global\0file:/home/u/.config/git/config\0fetch.prune\ntrue\0",
            "local\0file:/r/.git/config\0user.name\nAda at work\0",
        ]
        .concat();
        let sources = parse_sources(&output);
        let repo = Scope::Repository("/r".into());

        // Set in the repository itself: nothing is inherited.
        let own = Values {
            entries: vec![("user.name".into(), "Ada at work".into())],
        };
        assert_eq!(sources.inherited("user.name", &own, &repo), None);

        // Left unset there, so it shows the global value and says so.
        let none = Values::default();
        let (value, note) = sources.inherited("fetch.prune", &none, &repo).unwrap();
        assert_eq!(
            (value.as_str(), note.as_str()),
            ("true", "Using your global config.")
        );
        assert_eq!(sources.inherited("user.email", &none, &repo), None);

        // The global scope has nothing above it to inherit from.
        assert_eq!(
            sources.inherited("fetch.prune", &none, &Scope::Global),
            None
        );
        assert_eq!(sources.get("user.name").unwrap().scope, "local");
    }

    #[test]
    fn a_repository_scope_reads_and_writes_only_its_own_file() {
        let repo = crate::testing::Repo::new();
        let scope = Scope::Repository(repo.path().to_owned());
        set(&scope, "user.name", "Local Ada").unwrap();
        assert_eq!(read(&scope).unwrap().get("user.name"), Some("Local Ada"));
        let sources = read_sources(&scope).unwrap();
        assert_eq!(sources.get("user.name").unwrap().scope, "local");
        unset(&scope, "user.name").unwrap();
        assert!(!read(&scope).unwrap().is_set("user.name"));
    }

    #[test]
    fn messages_say_what_changed() {
        assert_eq!(
            saved_message("user.name", Some("Ada")),
            "Saved user.name as Ada."
        );
        assert!(saved_message("user.name", None).starts_with("Cleared user.name"));
    }
}
