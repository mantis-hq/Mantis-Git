//! The user's git config, as a short list of settings worth changing rather than every
//! key git knows. This file says which settings there are, what they are called, and how
//! to read and write them. A view only draws what it finds here.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::ops::{git_anywhere, run_command};
use std::path::PathBuf;

/// Which config file is being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The user's own config, applying to every repository.
    Global,
    /// One file, for tests.
    File(PathBuf),
}

impl Scope {
    /// What a person calls this config.
    pub fn name(&self) -> &'static str {
        match self {
            Scope::Global => "global git config",
            Scope::File(_) => "git config file",
        }
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut command = git_anywhere();
        command.arg("config");
        match self {
            Scope::Global => command.arg("--global"),
            Scope::File(path) => command.arg("--file").arg(path),
        };
        run_command(command.args(args), None)
    }
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
    }
}

const fn toggle(key: &'static str, label: &'static str, hint: &'static str) -> Setting {
    Setting {
        key,
        label,
        hint,
        kind: Kind::Toggle { default: false },
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
            ),
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
        let key = key.to_ascii_lowercase();
        self.entries
            .iter()
            .rev()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some()
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
        // Git says so when the file isn't there yet, and nothing is set.
        Err(message) if message.contains("No such file") || message.contains("cannot read") => {
            String::new()
        }
        // `git config --global --list` with no file exits with no message at all.
        Err(message) if message.starts_with("git failed without") => String::new(),
        Err(message) => return Err(message),
    };
    Ok(parse(&output))
}

/// Parses `git config --list -z`: `key`, a newline, the value, then a NUL.
fn parse(output: &str) -> Values {
    let entries = output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| match entry.split_once('\n') {
            Some((key, value)) => (key.to_ascii_lowercase(), value.to_owned()),
            // A key written with no `=` has no value at all, which git reads as on.
            None => (entry.to_ascii_lowercase(), "true".to_owned()),
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
    fn messages_say_what_changed() {
        assert_eq!(
            saved_message("user.name", Some("Ada")),
            "Saved user.name as Ada."
        );
        assert!(saved_message("user.name", None).starts_with("Cleared user.name"));
    }
}
