//! Identities: a name, email and signing key you commit as, kept under a label such as
//! "Work", and the ways one gets picked.
//!
//! Git has no identities, only config. Here an identity is a small file of its own,
//! `identities/<id>.gitconfig` next to the global config, holding a `[user]` block and a
//! `[gitclient] label`. Three things point at it, and all of them are references, so
//! editing the identity changes everywhere it is used:
//!
//! - a **rule** in the global config, an `includeIf` for a host, a folder or a remote
//!   pattern, which picks it for every repository that matches;
//! - a **pin**, an `include.path` in one repository's own config, which picks it there
//!   whatever the rules say, since a repository's config is read last;
//! - nothing, in which case the **default**, the global `user.*`, applies.
//!
//! Every read and write goes through `git config`, so git decides where the files live
//! and how the conditions are read. Everything here blocks, so call it off the UI thread.

use crate::settings::config::{self, Scope, Sources, Values};
use std::path::{Path, PathBuf};

const REMOTE: &str = "hasconfig:remote.*.url:";
const FOLDER: &str = "gitdir:";
/// Where identity files go, next to the config that includes them.
const FOLDER_NAME: &str = "identities";
const EXTENSION: &str = "gitconfig";
const USER_KEYS: [&str; 3] = ["user.name", "user.email", "user.signingkey"];

/// What an identity applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match {
    /// Repositories with a remote on one SSH host, by the name `ssh` is given: the
    /// `github-work` of `git@github-work:team/app.git`.
    Host(String),
    /// Repositories with a remote whose URL fits a pattern. For people who know globs.
    Remote(String),
    /// Repositories inside a folder, such as `~/work/`.
    Folder(String),
}

impl Match {
    /// The conditions to write, the first being the one that names the identity.
    ///
    /// A host takes four. Git's `**` only means "everything below" after a slash, so
    /// `git@host:**` would not match `git@host:team/app.git`, and a URL with no folder
    /// in it needs a pattern of its own.
    pub fn conditions(&self) -> Vec<String> {
        match self {
            Match::Host(host) => [
                format!("*@{host}:*"),
                format!("*@{host}:*/**"),
                format!("ssh://*@{host}/**"),
                format!("ssh://{host}/**"),
            ]
            .map(|pattern| format!("{REMOTE}{pattern}"))
            .to_vec(),
            Match::Remote(pattern) => vec![format!("{REMOTE}{pattern}")],
            Match::Folder(path) => vec![format!("{FOLDER}{path}")],
        }
    }

    /// Reads one condition back. A host's other conditions come out as `Remote` and are
    /// folded into it by [`list`].
    fn from_condition(condition: &str) -> Option<Match> {
        if let Some(pattern) = condition.strip_prefix(REMOTE) {
            let host = pattern
                .strip_prefix("*@")
                .and_then(|rest| rest.strip_suffix(":*"))
                .filter(|host| !host.is_empty() && !host.contains(['*', '/', ':']));
            Some(match host {
                Some(host) => Match::Host(host.to_owned()),
                None => Match::Remote(pattern.to_owned()),
            })
        } else {
            condition
                .strip_prefix(FOLDER)
                .map(|path| Match::Folder(path.to_owned()))
        }
    }

    pub fn pattern(&self) -> &str {
        match self {
            Match::Host(text) | Match::Remote(text) | Match::Folder(text) => text,
        }
    }

    /// For a heading.
    pub fn describe(&self) -> String {
        match self {
            Match::Host(host) => format!("the host {host}"),
            Match::Remote(pattern) => format!("remotes like {pattern}"),
            Match::Folder(path) => format!("repositories in {path}"),
        }
    }

    /// Without stray spaces, and a folder ending in a slash, which git reads as "and
    /// everything below it".
    pub fn tidy(&self) -> Match {
        match self {
            Match::Host(host) => Match::Host(host.trim().to_owned()),
            Match::Remote(pattern) => Match::Remote(pattern.trim().to_owned()),
            Match::Folder(path) => {
                let path = path.trim().replace('\\', "/");
                if path.is_empty() || path.ends_with('/') {
                    Match::Folder(path)
                } else {
                    Match::Folder(path + "/")
                }
            }
        }
    }

    /// Why this can't be used, if it can't.
    pub fn problem(&self) -> Option<&'static str> {
        let text = self.pattern();
        match self {
            _ if text.is_empty() => Some(match self {
                Match::Host(_) => "Say which SSH host this identity is for.",
                Match::Remote(_) => "Say which remotes this identity is for.",
                Match::Folder(_) => "Say which folder this identity is for.",
            }),
            Match::Host(_) if text.contains(['*', '?', '/', ':', '@']) || text.contains(' ') => {
                Some("A host is a name like github.com or work, with no @, : or /.")
            }
            _ => None,
        }
    }
}

fn include_key(condition: &str) -> String {
    format!("includeIf.{condition}.path")
}

/// The part of an identity git cares about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub email: String,
    /// The key that signs commits.
    pub signing_key: String,
    /// The SSH key file to push and pull with, as a path. Empty to leave it to ssh.
    pub ssh_key: String,
}

const SSH_SUFFIX: &str = " -o IdentitiesOnly=yes";

/// What git runs in place of `ssh` to use `key` and only `key`. Without
/// `IdentitiesOnly`, ssh would also offer every key in the agent, and a host that
/// accepts the first it likes could sign in as the wrong account.
pub fn ssh_command(key: &str) -> String {
    format!("ssh -i '{}'{SSH_SUFFIX}", key.replace('\'', r"'\''"))
}

/// The key in a command that [`ssh_command`] made. `None` for any other command, which
/// someone wrote by hand and which is left alone.
pub fn ssh_key_of(command: &str) -> Option<String> {
    let inner = command
        .trim()
        .strip_prefix("ssh -i '")?
        .strip_suffix(&format!("'{SSH_SUFFIX}"))?;
    Some(inner.replace(r"'\''", "'"))
}

impl Profile {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.email.is_empty()
            && self.signing_key.is_empty()
            && self.ssh_key.is_empty()
    }

    /// The `user.*` and SSH key of a config, trimmed.
    fn from_values(values: &Values) -> Profile {
        let text = |key: &str| values.get(key).unwrap_or("").trim().to_owned();
        Profile {
            name: text("user.name"),
            email: text("user.email"),
            signing_key: text("user.signingkey"),
            ssh_key: values
                .get("core.sshCommand")
                .and_then(ssh_key_of)
                .unwrap_or_default(),
        }
    }

    fn trimmed(&self) -> Profile {
        Profile {
            name: self.name.trim().to_owned(),
            email: self.email.trim().to_owned(),
            signing_key: self.signing_key.trim().to_owned(),
            ssh_key: self.ssh_key.trim().to_owned(),
        }
    }

    /// Writes this into `file`. An empty value removes its key. A `core.sshCommand`
    /// someone wrote by hand stays unless a key is chosen to replace it.
    fn write_to(&self, file: &Scope) -> Result<(), String> {
        let profile = self.trimmed();
        config::set(file, "user.name", &profile.name)?;
        config::set(file, "user.email", &profile.email)?;
        config::set(file, "user.signingkey", &profile.signing_key)?;
        let existing = config::read(file)?;
        let command = existing.get("core.sshCommand");
        if !profile.ssh_key.is_empty() {
            config::set(file, "core.sshCommand", &ssh_command(&profile.ssh_key))?;
        } else if command.is_none_or(|command| ssh_key_of(command).is_some()) {
            config::unset(file, "core.sshCommand")?;
        }
        Ok(())
    }

    /// "Ada <ada@example.com>", for a line of text.
    pub fn display(&self) -> String {
        match (self.name.is_empty(), self.email.is_empty()) {
            (false, false) => format!("{} <{}>", self.name, self.email),
            (false, true) => self.name.clone(),
            (true, false) => format!("<{}>", self.email),
            (true, true) => "no name or email".to_owned(),
        }
    }
}

/// A named profile, which can be used in many places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The file's name, without its extension. Empty for one not saved yet.
    pub id: String,
    pub label: String,
    pub profile: Profile,
}

/// "Use this identity for everything matching this."
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub matcher: Match,
    pub identity: String,
}

/// Everything about identities in a config.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Book {
    /// The global `user.*`, which applies when nothing picks another.
    pub default: Profile,
    /// In label order.
    pub identities: Vec<Identity>,
    pub rules: Vec<Rule>,
}

impl Book {
    pub fn identity(&self, id: &str) -> Option<&Identity> {
        self.identities.iter().find(|identity| identity.id == id)
    }

    pub fn rules_for<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Rule> {
        self.rules.iter().filter(move |rule| rule.identity == id)
    }

    /// Whether some identity, or the default, already is this profile.
    pub fn has_profile(&self, profile: &Profile) -> bool {
        self.default == *profile || self.identities.iter().any(|i| i.profile == *profile)
    }
}

/// The id of the identity file a config's `path` points at, whether written as a path
/// relative to the config, with a `~`, or in full.
fn id_of(path: &str) -> Option<String> {
    let path = Path::new(path.trim());
    let in_folder = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|folder| folder == FOLDER_NAME);
    let is_identity = path.extension().is_some_and(|ext| ext == EXTENSION);
    if !(in_folder && is_identity) {
        return None;
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
}

/// What to write after `path =`: with a `~` where it can, so the config reads the same
/// from any folder.
fn include_value(file: &Path) -> String {
    let text = file.to_string_lossy().replace('\\', "/");
    if let Some(home) = config::home_directory() {
        let home = home.to_string_lossy().replace('\\', "/");
        let prefix = format!("{}/", home.trim_end_matches('/'));
        if let Some(rest) = text.strip_prefix(&prefix) {
            return format!("~/{rest}");
        }
    }
    text
}

/// Where identity files are kept for `scope`. Git says where its config is; with no
/// config yet there is nothing to ask, so the usual place is used.
fn identities_folder(scope: &Scope) -> Result<PathBuf, String> {
    let directory = match config::config_directory(scope) {
        Ok(directory) => directory,
        Err(_) if matches!(scope, Scope::Global) => config::home_directory()
            .map(|home| home.join(".config").join("git"))
            .ok_or("There is no home folder to keep identities in.")?,
        Err(message) => return Err(message),
    };
    Ok(directory.join(FOLDER_NAME))
}

fn identity_file(scope: &Scope, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err("That isn't an identity.".to_owned());
    }
    Ok(identities_folder(scope)?.join(format!("{id}.{EXTENSION}")))
}

/// "Work Laptop" becomes "work-laptop".
fn slug(label: &str) -> String {
    let mut slug = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(60).collect();
    if slug.is_empty() {
        "identity".to_owned()
    } else {
        slug
    }
}

/// "work-laptop" read as words: "Work laptop". For files not made here, which have no label.
fn humanise(id: &str) -> String {
    let words = id.replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Reads the default, the identities and the rules of `scope`.
pub fn book(scope: &Scope) -> Result<Book, String> {
    let values = config::read(scope)?;

    let mut found: Vec<(Match, String)> = Vec::new();
    for (key, path) in values.iter() {
        let Some(condition) = key
            .strip_prefix("includeif.")
            .and_then(|rest| rest.strip_suffix(".path"))
        else {
            continue;
        };
        if let Some(matcher) = Match::from_condition(condition) {
            found.push((matcher, path.to_owned()));
        }
    }
    // The extra conditions of a host share its file, and belong to it.
    let extras: Vec<(String, String)> = found
        .iter()
        .filter(|(matcher, _)| matches!(matcher, Match::Host(_)))
        .flat_map(|(matcher, path)| {
            let extras = matcher.conditions().into_iter().skip(1);
            extras
                .map(|condition| (condition, path.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    found.retain(|(matcher, path)| {
        let own = (matcher.conditions().swap_remove(0), path.clone());
        !(matches!(matcher, Match::Remote(_)) && extras.contains(&own))
    });
    let rules: Vec<Rule> = found
        .into_iter()
        .filter_map(|(matcher, path)| id_of(&path).map(|identity| Rule { matcher, identity }))
        .collect();

    let mut identities = Vec::new();
    if let Ok(entries) = std::fs::read_dir(identities_folder(scope)?) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path
                .extension()
                .is_some_and(|ext| ext == EXTENSION)
                .then(|| path.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
                .flatten()
            else {
                continue;
            };
            let own = config::read(&Scope::File(path))?;
            // A file from before identities had labels is named after what it matched.
            let label = own
                .get("gitclient.label")
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    rules
                        .iter()
                        .find(|rule| rule.identity == id)
                        .map(|rule| rule.matcher.pattern().to_owned())
                })
                .unwrap_or_else(|| humanise(&id));
            identities.push(Identity {
                id,
                label,
                profile: Profile::from_values(&own),
            });
        }
    }
    identities.sort_by_key(|identity| (identity.label.to_lowercase(), identity.id.clone()));
    let rules = rules
        .into_iter()
        .filter(|rule| identities.iter().any(|i| i.id == rule.identity))
        .collect();

    Ok(Book {
        default: Profile::from_values(&values),
        identities,
        rules,
    })
}

/// Saves `identity`, making a new one when its id is empty. Returns the id.
pub fn save_identity(scope: &Scope, identity: &Identity) -> Result<String, String> {
    let label = identity.label.trim();
    if label.is_empty() {
        return Err("Give the identity a name, like Work or Personal.".to_owned());
    }
    let profile = identity.profile.trimmed();
    if profile.is_empty() {
        return Err("Give the identity a name or an email.".to_owned());
    }
    let folder = identities_folder(scope)?;
    let id = if identity.id.is_empty() {
        let base = slug(label);
        let mut id = base.clone();
        let mut number = 2;
        while folder.join(format!("{id}.{EXTENSION}")).exists() {
            id = format!("{base}-{number}");
            number += 1;
        }
        id
    } else {
        identity.id.clone()
    };
    let file = identity_file(scope, &id)?;
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("Couldn't make {}: {error}", folder.display()))?;
    let file = Scope::File(file);
    profile.write_to(&file)?;
    config::set(&file, "gitclient.label", label)?;
    Ok(id)
}

/// Deletes an identity and the rules that picked it. A repository pinned to it falls
/// back to its rules, since git skips an include whose file isn't there.
pub fn delete_identity(scope: &Scope, id: &str) -> Result<(), String> {
    let file = identity_file(scope, id)?;
    for rule in book(scope)?.rules_for(id) {
        remove_rule(scope, &rule.matcher)?;
    }
    match std::fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Couldn't delete {}: {error}", file.display())),
    }
}

/// Saves the default: the global `user.*`.
pub fn save_default(scope: &Scope, profile: &Profile) -> Result<(), String> {
    profile.write_to(scope)
}

/// Sets the default's SSH key and leaves its name, email and signing key as they are. An
/// empty key takes out a key that gitclient set, and leaves a command written by hand.
pub fn save_default_ssh_key(scope: &Scope, key: &str) -> Result<(), String> {
    let profile = Profile {
        ssh_key: key.to_owned(),
        ..Profile::from_values(&config::read(scope)?)
    };
    profile.write_to(scope)
}

/// Picks identity `id` for everything `matcher` matches, replacing any other that did.
pub fn add_rule(scope: &Scope, id: &str, matcher: &Match) -> Result<(), String> {
    let matcher = matcher.tidy();
    if let Some(problem) = matcher.problem() {
        return Err(problem.to_owned());
    }
    let file = identity_file(scope, id)?;
    if !file.exists() {
        return Err("That identity no longer exists.".to_owned());
    }
    let value = include_value(&file);
    for condition in matcher.conditions() {
        config::set(scope, &include_key(&condition), &value)?;
    }
    Ok(())
}

/// Stops `matcher` picking an identity.
pub fn remove_rule(scope: &Scope, matcher: &Match) -> Result<(), String> {
    let matcher = matcher.tidy();
    let values = config::read(scope)?;
    for condition in matcher.conditions() {
        if values.is_set(&include_key(&condition)) {
            scope.run(&["--remove-section", &format!("includeIf.{condition}")])?;
        }
    }
    Ok(())
}

/// What a repository should use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Whatever the rules say, else the default.
    Automatic,
    Identity(String),
}

/// What a repository uses, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoState {
    /// The identity this repository's own config points at.
    pub pinned: Option<String>,
    /// A name and email set in the repository's own config, outside any identity.
    pub own: Option<Profile>,
    pub committing: Committing,
}

pub fn repo_state(repo: &Path, book: &Book) -> Result<RepoState, String> {
    let local = Scope::Repository(repo.to_owned());
    let values = config::read(&local)?;
    let pinned = values
        .get_all("include.path")
        .into_iter()
        .rev()
        .filter_map(id_of)
        .find(|id| book.identity(id).is_some());
    let own = Some(Profile::from_values(&values)).filter(|own| !own.is_empty());
    let sources = config::read_sources(&local)?;
    Ok(RepoState {
        committing: Committing::from_sources(&sources, book, pinned.as_deref()),
        pinned,
        own,
    })
}

/// A value to match literally in `git config --unset name <pattern>`.
fn literal(text: &str) -> String {
    let mut pattern = String::from("^");
    for c in text.chars() {
        if "\\.[]{}()*+?^$|".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('$');
    pattern
}

/// Makes `repo` use `choice`. A name and email the repository had of its own are kept
/// as an identity first, unless one already is that, since picking would replace them.
pub fn pick(global: &Scope, repo: &Path, choice: &Choice) -> Result<(), String> {
    let local = Scope::Repository(repo.to_owned());
    let values = config::read(&local)?;
    let book = book(global)?;
    if let Choice::Identity(id) = choice
        && book.identity(id).is_none()
    {
        return Err("That identity no longer exists.".to_owned());
    }

    let own = Profile::from_values(&values);
    if !own.is_empty() && !book.has_profile(&own) {
        let label = repo
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("This repository");
        save_identity(
            global,
            &Identity {
                id: String::new(),
                label: label.to_owned(),
                profile: own,
            },
        )?;
    }
    for key in USER_KEYS {
        config::unset(&local, key)?;
    }
    // The key this app set goes with the name it replaces; one written by hand stays.
    if values.get("core.sshCommand").and_then(ssh_key_of).is_some() {
        config::unset(&local, "core.sshCommand")?;
    }
    for path in values.get_all("include.path") {
        if id_of(path).is_some() {
            local.run(&["--unset", "include.path", &literal(path)])?;
        }
    }
    if let Choice::Identity(id) = choice {
        let value = include_value(&identity_file(global, id)?);
        local.run(&["--add", "include.path", &value])?;
    }
    Ok(())
}

/// Who commits made in a repository are attributed to, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committing {
    pub name: Option<String>,
    pub email: Option<String>,
    /// Where the name and email come from, in words.
    pub from: String,
}

impl Committing {
    pub fn from_sources(sources: &Sources, book: &Book, pinned: Option<&str>) -> Committing {
        let name = sources.get("user.name");
        let email = sources.get("user.email");
        let place = |source: Option<&config::Source>| {
            source.map(|source| origin_words(source, book, pinned))
        };
        let from = match (place(name), place(email)) {
            (Some(name), Some(email)) if name != email => {
                format!("{name} for the name and {email} for the email")
            }
            (Some(place), _) | (None, Some(place)) => place,
            (None, None) => String::new(),
        };
        let value = |source: Option<&config::Source>| {
            source
                .map(|s| s.value.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Committing {
            name: value(name),
            email: value(email),
            from,
        }
    }

    pub fn sentence(&self) -> String {
        if self.name.is_none() && self.email.is_none() {
            return "No name or email is set here, so git can't make commits.".to_owned();
        }
        format!(
            "Commits here are made as {} <{}>, from {}.",
            self.name.as_deref().unwrap_or("(no name)"),
            self.email.as_deref().unwrap_or("no email"),
            self.from
        )
    }
}

fn origin_words(source: &config::Source, book: &Book, pinned: Option<&str>) -> String {
    let identity = source
        .origin
        .strip_prefix("file:")
        .and_then(id_of)
        .and_then(|id| book.identity(&id));
    if let Some(identity) = identity {
        let label = &identity.label;
        if pinned == Some(identity.id.as_str()) {
            return format!("the identity {label}, chosen for this repository");
        }
        let mut rules = book.rules_for(&identity.id);
        return match (rules.next(), rules.next()) {
            (Some(rule), None) => {
                format!(
                    "the identity {label}, because of {}",
                    rule.matcher.describe()
                )
            }
            (Some(_), Some(_)) => format!("the identity {label}, because of one of its rules"),
            _ => format!("the identity {label}"),
        };
    }
    match source.scope.as_str() {
        "system" => "the system config",
        "global" => "your global config",
        "local" => "this repository's own config",
        "worktree" => "this worktree's config",
        _ => "the command line",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn scope() -> (TempDir, Scope) {
        let dir = TempDir::new().unwrap();
        let scope = Scope::File(dir.path().join("config"));
        (dir, scope)
    }

    fn profile(name: &str, email: &str) -> Profile {
        Profile {
            name: name.into(),
            email: email.into(),
            signing_key: String::new(),
            ssh_key: String::new(),
        }
    }

    fn with_key(key: &str) -> Profile {
        Profile {
            ssh_key: key.into(),
            ..profile("Ada Lovelace", "ada@work.example")
        }
    }

    #[test]
    fn an_ssh_key_becomes_a_command_that_uses_only_that_key_and_reads_back() {
        let command = ssh_command("~/.ssh/id_work");
        assert_eq!(command, "ssh -i '~/.ssh/id_work' -o IdentitiesOnly=yes");
        assert_eq!(ssh_key_of(&command).as_deref(), Some("~/.ssh/id_work"));

        // Spaces and quotes in a path survive the round trip.
        for key in [
            "/home/ada/my keys/id work",
            "/tmp/it's/id",
            "C:/Users/Ada/.ssh/id",
        ] {
            assert_eq!(ssh_key_of(&ssh_command(key)).as_deref(), Some(key), "{key}");
        }
        // A command written by hand is not ours to read as a key.
        assert_eq!(ssh_key_of("ssh -i /k -F /dev/null"), None);
        assert_eq!(ssh_key_of("plink -i key.ppk"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_command_is_valid_shell_whatever_the_path() {
        // Git runs it through a shell, so check one does: `-V` makes ssh print its version
        // and quit, with `-i` and the path ignored.
        for key in [
            "/tmp/plain",
            "/tmp/with space/id",
            "/tmp/it's/id",
            "/tmp/$HOME/`x`",
        ] {
            let run = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{} -V", ssh_command(key)))
                .output();
            let Ok(run) = run else { return };
            if run.stderr.is_empty() && run.stdout.is_empty() {
                return; // no ssh here to ask
            }
            assert!(
                run.status.success(),
                "{key}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
    }

    #[test]
    fn an_identity_carries_its_ssh_key_and_git_uses_it_where_the_identity_applies() {
        let (_dir, scope) = scope();
        let id = save_identity(
            &scope,
            &Identity {
                profile: with_key("~/.ssh/id_work"),
                ..work()
            },
        )
        .unwrap();
        assert_eq!(
            book(&scope).unwrap().identity(&id).unwrap().profile.ssh_key,
            "~/.ssh/id_work"
        );
        add_rule(&scope, &id, &Match::Host("work".into())).unwrap();

        let by_rule = repo(&scope, Some("git@work:team/app.git"));
        let command = |repo: &TempDir| ask(repo.path(), &scope, &["config", "core.sshCommand"]);
        assert_eq!(command(&by_rule), ssh_command("~/.ssh/id_work"));
        let elsewhere = repo(&scope, Some("git@home:me/app.git"));
        assert_eq!(command(&elsewhere), "", "only where the identity applies");

        // A pin brings the key too, and picking another identity takes it away again.
        pick(&scope, elsewhere.path(), &Choice::Identity(id)).unwrap();
        assert_eq!(command(&elsewhere), ssh_command("~/.ssh/id_work"));
        pick(&scope, elsewhere.path(), &Choice::Automatic).unwrap();
        assert_eq!(command(&elsewhere), "");
    }

    #[test]
    fn changing_or_clearing_the_key_edits_the_identity_everywhere() {
        let (_dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        let pinned = repo(&scope, None);
        pick(&scope, pinned.path(), &Choice::Identity(id.clone())).unwrap();
        let command = || ask(pinned.path(), &scope, &["config", "core.sshCommand"]);
        assert_eq!(command(), "", "no key yet");

        let with = |key: &str| Identity {
            id: id.clone(),
            profile: with_key(key),
            ..work()
        };
        save_identity(&scope, &with("~/.ssh/one")).unwrap();
        assert_eq!(command(), ssh_command("~/.ssh/one"));
        save_identity(&scope, &with("~/.ssh/two")).unwrap();
        assert_eq!(command(), ssh_command("~/.ssh/two"));
        save_identity(&scope, &with("")).unwrap();
        assert_eq!(command(), "", "clearing it removes the command");
    }

    #[test]
    fn a_command_someone_wrote_by_hand_is_left_alone() {
        let (dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        let file = Scope::File(
            dir.path()
                .join("identities")
                .join(format!("{id}.gitconfig")),
        );
        config::set(&file, "core.sshCommand", "ssh -F /my/config").unwrap();

        // Saving the identity with no key chosen doesn't clobber it, and it isn't a key.
        save_identity(
            &scope,
            &Identity {
                id: id.clone(),
                ..work()
            },
        )
        .unwrap();
        assert_eq!(
            config::read(&file).unwrap().get("core.sshCommand"),
            Some("ssh -F /my/config")
        );
        assert_eq!(
            book(&scope).unwrap().identity(&id).unwrap().profile.ssh_key,
            ""
        );

        // Choosing a key is a decision to replace it.
        save_identity(
            &scope,
            &Identity {
                id,
                profile: with_key("~/.ssh/id_work"),
                ..work()
            },
        )
        .unwrap();
        assert_eq!(
            config::read(&file).unwrap().get("core.sshCommand"),
            Some(ssh_command("~/.ssh/id_work").as_str())
        );
    }

    #[test]
    fn a_repository_own_key_is_kept_with_its_name_and_a_hand_written_command_stays() {
        let (_dir, scope) = scope();
        let other = save_identity(&scope, &work()).unwrap();
        let repo = repo(&scope, None);
        ask(
            repo.path(),
            &scope,
            &["config", "user.email", "local@example.com"],
        );
        ask(
            repo.path(),
            &scope,
            &["config", "core.sshCommand", &ssh_command("~/.ssh/local")],
        );
        let state = repo_state(repo.path(), &book(&scope).unwrap()).unwrap();
        assert_eq!(state.own.unwrap().ssh_key, "~/.ssh/local");

        pick(&scope, repo.path(), &Choice::Identity(other)).unwrap();
        let kept = book(&scope).unwrap();
        let kept = kept
            .identities
            .iter()
            .find(|i| i.profile.email == "local@example.com")
            .expect("kept as an identity");
        assert_eq!(kept.profile.ssh_key, "~/.ssh/local");
        assert_eq!(ask(repo.path(), &scope, &["config", "core.sshCommand"]), "");

        // A command the user wrote by hand in the repository is theirs, and stays.
        ask(
            repo.path(),
            &scope,
            &["config", "core.sshCommand", "ssh -F /mine"],
        );
        pick(&scope, repo.path(), &Choice::Automatic).unwrap();
        assert_eq!(
            ask(repo.path(), &scope, &["config", "core.sshCommand"]),
            "ssh -F /mine"
        );
    }

    fn work() -> Identity {
        Identity {
            id: String::new(),
            label: "Work".into(),
            profile: profile("Ada Lovelace", "ada@work.example"),
        }
    }

    /// Runs git in `dir`, with `scope`'s file as the global config, so that git itself
    /// decides what the includes and conditions mean.
    fn ask(dir: &Path, scope: &Scope, args: &[&str]) -> String {
        let Scope::File(file) = scope else { panic!() };
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .env("GIT_CONFIG_GLOBAL", file)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A new repository, not the shared fixture, which keeps its own email in the
    /// repository and so would rightly win over any include.
    fn repo(scope: &Scope, remote: Option<&str>) -> TempDir {
        let repo = TempDir::new().unwrap();
        ask(repo.path(), scope, &["init", "-q"]);
        if let Some(url) = remote {
            ask(repo.path(), scope, &["remote", "add", "origin", url]);
        }
        repo
    }

    fn email_in(repo: &TempDir, scope: &Scope) -> String {
        ask(repo.path(), scope, &["config", "user.email"])
    }

    #[test]
    fn identities_are_saved_by_label_and_listed_in_label_order() {
        let (dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        assert_eq!(id, "work");
        let personal = Identity {
            label: "Personal".into(),
            profile: profile("Ada", "ada@home.example"),
            ..work()
        };
        save_identity(&scope, &personal).unwrap();
        // The same label again gets a file of its own.
        assert_eq!(save_identity(&scope, &work()).unwrap(), "work-2");

        let book = book(&scope).unwrap();
        let labels: Vec<_> = book.identities.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Personal", "Work", "Work"]);
        assert_eq!(book.identity("work").unwrap().profile, work().profile);
        assert!(dir.path().join("identities/work.gitconfig").exists());
    }

    #[test]
    fn saving_an_existing_identity_changes_it_in_place() {
        let (_dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        let renamed = Identity {
            id: id.clone(),
            label: "Day job".into(),
            profile: Profile {
                signing_key: "ABC".into(),
                ..profile("Ada", "new@work.example")
            },
        };
        assert_eq!(save_identity(&scope, &renamed).unwrap(), id);
        let book = book(&scope).unwrap();
        assert_eq!(book.identities.len(), 1);
        assert_eq!(book.identities[0].label, "Day job");
        assert_eq!(book.identities[0].profile.email, "new@work.example");
        assert_eq!(book.identities[0].profile.signing_key, "ABC");
    }

    #[test]
    fn an_identity_needs_a_label_and_something_to_say() {
        let (_dir, scope) = scope();
        let blank = Profile::default();
        let no_label = Identity {
            label: "  ".into(),
            ..work()
        };
        let empty = Identity {
            profile: blank,
            ..work()
        };
        assert!(save_identity(&scope, &no_label).is_err());
        assert!(save_identity(&scope, &empty).is_err());
        assert!(book(&scope).unwrap().identities.is_empty());
    }

    #[test]
    fn the_defaults_ssh_key_changes_alone_and_clears_again() {
        let (_dir, scope) = scope();
        save_default(&scope, &profile("Pretext", "p@example.com")).unwrap();
        save_default_ssh_key(&scope, "~/.ssh/id_home").unwrap();
        let default = book(&scope).unwrap().default;
        assert_eq!(default.ssh_key, "~/.ssh/id_home");
        assert_eq!(
            (default.name.as_str(), default.email.as_str()),
            ("Pretext", "p@example.com")
        );
        save_default_ssh_key(&scope, "").unwrap();
        let default = book(&scope).unwrap().default;
        assert_eq!(default.ssh_key, "");
        assert_eq!(default.name, "Pretext");
    }

    #[test]
    fn the_default_is_the_global_user_and_is_edited_like_an_identity() {
        let (_dir, scope) = scope();
        save_default(&scope, &profile("Pretext", "p@example.com")).unwrap();
        assert_eq!(
            book(&scope).unwrap().default,
            profile("Pretext", "p@example.com")
        );
        save_default(&scope, &profile("Pretext", "")).unwrap();
        assert_eq!(book(&scope).unwrap().default, profile("Pretext", ""));
    }

    #[test]
    fn one_identity_can_have_many_rules_and_git_applies_each() {
        let (dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        add_rule(&scope, &id, &Match::Host("work".into())).unwrap();
        add_rule(&scope, &id, &Match::Host("github-work".into())).unwrap();
        let folder = dir.path().join("Work Stuff");
        std::fs::create_dir_all(&folder).unwrap();
        let folder_text = folder.display().to_string().replace('\\', "/");
        add_rule(&scope, &id, &Match::Folder(folder_text)).unwrap();

        let book = book(&scope).unwrap();
        assert_eq!(book.rules.len(), 3, "{:?}", book.rules);
        assert!(book.rules.iter().all(|rule| rule.identity == "work"));

        for url in [
            "git@work:team/app.git",
            "ssh://git@github-work/team/app.git",
        ] {
            let repo = repo(&scope, Some(url));
            assert_eq!(email_in(&repo, &scope), "ada@work.example", "{url}");
        }
        let repo_elsewhere = repo(&scope, Some("git@home:me/app.git"));
        assert_eq!(email_in(&repo_elsewhere, &scope), "");
        let inside = folder.join("app");
        std::fs::create_dir_all(&inside).unwrap();
        ask(&inside, &scope, &["init", "-q"]);
        assert_eq!(
            ask(&inside, &scope, &["config", "user.email"]),
            "ada@work.example"
        );
    }

    #[test]
    fn a_rule_is_handed_to_another_identity_by_adding_it_there() {
        let (_dir, scope) = scope();
        let one = save_identity(&scope, &work()).unwrap();
        let two = save_identity(
            &scope,
            &Identity {
                label: "Other".into(),
                profile: profile("Ada", "other@example.com"),
                ..work()
            },
        )
        .unwrap();
        add_rule(&scope, &one, &Match::Host("work".into())).unwrap();
        add_rule(&scope, &two, &Match::Host("work".into())).unwrap();
        let book = book(&scope).unwrap();
        assert_eq!(book.rules.len(), 1);
        assert_eq!(book.rules[0].identity, "other");
        let repo = repo(&scope, Some("git@work:team/app.git"));
        assert_eq!(email_in(&repo, &scope), "other@example.com");
    }

    #[test]
    fn rules_are_validated_and_removed_whole() {
        let (dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        for bad in [
            Match::Remote("  ".into()),
            Match::Host(String::new()),
            Match::Host("git@work".into()),
            Match::Host("a/b".into()),
            Match::Folder(" ".into()),
        ] {
            assert!(add_rule(&scope, &id, &bad).is_err(), "{bad:?}");
        }
        assert!(add_rule(&scope, "nobody", &Match::Host("x".into())).is_err());
        assert!(add_rule(&scope, "../x", &Match::Host("x".into())).is_err());

        add_rule(&scope, &id, &Match::Host("work".into())).unwrap();
        add_rule(&scope, &id, &Match::Remote("https://example.com/**".into())).unwrap();
        assert_eq!(book(&scope).unwrap().rules.len(), 2);

        remove_rule(&scope, &Match::Host("work".into())).unwrap();
        let config = std::fs::read_to_string(dir.path().join("config")).unwrap();
        assert!(!config.contains("@work"), "{config}");
        assert_eq!(book(&scope).unwrap().rules.len(), 1);
        let repo = repo(&scope, Some("git@work:team/app.git"));
        assert_eq!(email_in(&repo, &scope), "");
    }

    #[test]
    fn deleting_an_identity_removes_its_file_and_its_rules() {
        let (dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        add_rule(&scope, &id, &Match::Host("work".into())).unwrap();
        delete_identity(&scope, &id).unwrap();
        let book = book(&scope).unwrap();
        assert!(book.identities.is_empty() && book.rules.is_empty());
        assert!(!dir.path().join("identities/work.gitconfig").exists());
        let config = std::fs::read_to_string(dir.path().join("config")).unwrap();
        assert!(!config.contains("includeIf"), "{config}");
        assert!(delete_identity(&scope, "../config").is_err());
    }

    #[test]
    fn identities_from_before_labels_still_show_with_their_rule() {
        let (dir, scope) = scope();
        std::fs::create_dir_all(dir.path().join("identities")).unwrap();
        std::fs::write(
            dir.path()
                .join("identities/hasconfig-remote-url-test.gitconfig"),
            "[user]\n\temail = old@example.com\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("config"),
            "[includeIf \"hasconfig:remote.*.url:*@test:*\"]\n\
             \tpath = identities/hasconfig-remote-url-test.gitconfig\n\
             [includeIf \"hasconfig:remote.*.url:*@test:*/**\"]\n\
             \tpath = identities/hasconfig-remote-url-test.gitconfig\n",
        )
        .unwrap();
        let book = book(&scope).unwrap();
        assert_eq!(book.identities.len(), 1);
        assert_eq!(book.identities[0].label, "test", "named after its rule");
        assert_eq!(book.rules.len(), 1, "the host's extra conditions fold in");
        assert_eq!(book.rules[0].matcher, Match::Host("test".into()));
    }

    #[test]
    fn pinning_picks_an_identity_in_one_repository_over_the_rules() {
        let (_dir, scope) = scope();
        let work_id = save_identity(&scope, &work()).unwrap();
        let home_id = save_identity(
            &scope,
            &Identity {
                label: "Home".into(),
                profile: profile("Ada", "ada@home.example"),
                ..work()
            },
        )
        .unwrap();
        add_rule(&scope, &work_id, &Match::Host("work".into())).unwrap();
        let repo = repo(&scope, Some("git@work:team/app.git"));
        assert_eq!(email_in(&repo, &scope), "ada@work.example", "by its rule");

        pick(&scope, repo.path(), &Choice::Identity(home_id.clone())).unwrap();
        assert_eq!(email_in(&repo, &scope), "ada@home.example", "pinned wins");
        let state = repo_state(repo.path(), &book(&scope).unwrap()).unwrap();
        assert_eq!(state.pinned.as_deref(), Some("home"));
        assert_eq!(state.own, None);

        // Picking another replaces the pin instead of adding to it.
        pick(&scope, repo.path(), &Choice::Identity(work_id)).unwrap();
        assert_eq!(email_in(&repo, &scope), "ada@work.example");
        let local = std::fs::read_to_string(repo.path().join(".git/config")).unwrap();
        assert_eq!(local.matches("[include]").count(), 1, "{local}");
        assert!(local.contains("identities/work.gitconfig"), "{local}");
        assert!(!local.contains("home.gitconfig"), "{local}");

        pick(&scope, repo.path(), &Choice::Automatic).unwrap();
        let local = std::fs::read_to_string(repo.path().join(".git/config")).unwrap();
        assert!(!local.contains("identities"), "{local}");
        assert_eq!(
            email_in(&repo, &scope),
            "ada@work.example",
            "the rule again"
        );
    }

    #[test]
    fn editing_an_identity_changes_every_repository_pinned_to_it() {
        let (_dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        let (one, two) = (repo(&scope, None), repo(&scope, None));
        for repo in [&one, &two] {
            pick(&scope, repo.path(), &Choice::Identity(id.clone())).unwrap();
        }
        save_identity(
            &scope,
            &Identity {
                id,
                profile: profile("Ada", "changed@work.example"),
                ..work()
            },
        )
        .unwrap();
        for repo in [&one, &two] {
            assert_eq!(email_in(repo, &scope), "changed@work.example");
        }
    }

    #[test]
    fn a_repository_own_name_is_kept_as_an_identity_when_it_is_replaced() {
        let (_dir, scope) = scope();
        let other = save_identity(&scope, &work()).unwrap();
        let repo = repo(&scope, None);
        ask(repo.path(), &scope, &["config", "user.name", "Local Ada"]);
        ask(
            repo.path(),
            &scope,
            &["config", "user.email", "local@example.com"],
        );
        let state = repo_state(repo.path(), &book(&scope).unwrap()).unwrap();
        assert_eq!(state.own, Some(profile("Local Ada", "local@example.com")));

        pick(&scope, repo.path(), &Choice::Identity(other)).unwrap();
        assert_eq!(email_in(&repo, &scope), "ada@work.example");
        let book = book(&scope).unwrap();
        let kept = book
            .identities
            .iter()
            .find(|i| i.profile.email == "local@example.com")
            .expect("the repository's own identity was kept");
        assert!(kept.label.starts_with('.') || !kept.label.is_empty());

        // Going back to it is one click, and doesn't keep it twice.
        let kept_id = kept.id.clone();
        pick(&scope, repo.path(), &Choice::Identity(kept_id)).unwrap();
        assert_eq!(email_in(&repo, &scope), "local@example.com");
        assert_eq!(book_len(&scope), 2);
        pick(&scope, repo.path(), &Choice::Automatic).unwrap();
        assert_eq!(book_len(&scope), 2);
    }

    fn book_len(scope: &Scope) -> usize {
        book(scope).unwrap().identities.len()
    }

    #[test]
    fn a_profile_the_default_already_is_is_not_kept_again() {
        let (_dir, scope) = scope();
        save_default(&scope, &profile("Pretext", "p@example.com")).unwrap();
        let repo = repo(&scope, None);
        ask(repo.path(), &scope, &["config", "user.name", "Pretext"]);
        ask(
            repo.path(),
            &scope,
            &["config", "user.email", "p@example.com"],
        );
        pick(&scope, repo.path(), &Choice::Automatic).unwrap();
        assert_eq!(book_len(&scope), 0);
        assert_eq!(email_in(&repo, &scope), "p@example.com");
    }

    #[test]
    fn picking_a_missing_identity_fails_and_changes_nothing() {
        let (_dir, scope) = scope();
        let repo = repo(&scope, None);
        ask(
            repo.path(),
            &scope,
            &["config", "user.email", "keep@example.com"],
        );
        assert!(pick(&scope, repo.path(), &Choice::Identity("gone".into())).is_err());
        assert_eq!(email_in(&repo, &scope), "keep@example.com");
    }

    #[test]
    fn committing_says_which_identity_and_why() {
        let (dir, scope) = scope();
        let id = save_identity(&scope, &work()).unwrap();
        add_rule(&scope, &id, &Match::Host("work".into())).unwrap();
        let book = book(&scope).unwrap();
        let file = dir.path().join("identities/work.gitconfig");
        let line = |scope: &str, origin: &Path, entry: &str| {
            format!("{scope}\0file:{}\0{entry}\0", origin.display())
        };
        let config = dir.path().join("config");
        let output = [
            line("global", &config, "user.name\nPretext"),
            line("global", &file, "user.email\nada@work.example"),
        ]
        .concat();
        let sources = config::parse_sources(&output);

        let by_rule = Committing::from_sources(&sources, &book, None);
        assert_eq!(by_rule.email.as_deref(), Some("ada@work.example"));
        assert_eq!(
            by_rule.from,
            "your global config for the name and the identity Work, because of the host work \
             for the email"
        );
        let pinned = Committing::from_sources(&sources, &book, Some("work"));
        assert!(
            pinned.from.contains("chosen for this repository"),
            "{}",
            pinned.from
        );
    }

    #[test]
    fn with_nothing_set_the_sentence_says_commits_cannot_be_made() {
        let none = Committing::from_sources(&Sources::default(), &Book::default(), None);
        assert!(none.sentence().starts_with("No name or email"));
    }

    #[test]
    fn ids_and_labels_read_well() {
        assert_eq!(slug("Work Laptop!"), "work-laptop");
        assert_eq!(slug("  "), "identity");
        assert_eq!(humanise("work-laptop"), "Work laptop");
        assert_eq!(
            id_of("~/.config/git/identities/work.gitconfig").as_deref(),
            Some("work")
        );
        assert_eq!(id_of("identities/work.gitconfig").as_deref(), Some("work"));
        assert_eq!(id_of("/etc/gitconfig.d/other.gitconfig"), None);
        assert_eq!(literal("~/a.b"), "^~/a\\.b$");
        assert_eq!(profile("Ada", "a@x").display(), "Ada <a@x>");
    }
}
