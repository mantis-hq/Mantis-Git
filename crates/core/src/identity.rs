//! Identities: a name, email and signing key that apply to the repositories matching a
//! remote or a folder, so that work and personal commits don't need a switch.
//!
//! Git has no list of identities, only conditional includes. An identity here is one
//! `includeIf` in the global config pointing at a small file of its own holding the
//! `[user]` block. Every read and write goes through `git config`, so git decides where
//! the files live and how the conditions are read.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::config::{self, Scope, Sources};
use std::path::{Path, PathBuf};

const REMOTE: &str = "hasconfig:remote.*.url:";
const FOLDER: &str = "gitdir:";
/// Where identity files go, next to the config that includes them.
const FOLDER_NAME: &str = "identities";

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

    fn condition(&self) -> String {
        self.conditions().swap_remove(0)
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

    /// The name of the file holding this identity.
    fn file_name(&self) -> String {
        let mut slug = String::new();
        for c in self.condition().chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug: String = slug.trim_matches('-').chars().take(80).collect();
        format!("{slug}.gitconfig")
    }
}

fn include_key(condition: &str) -> String {
    format!("includeIf.{condition}.path")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub matcher: Match,
    pub name: String,
    pub email: String,
    pub signing_key: String,
}

/// Where an include's `path` points: `~` is the home folder, and a relative path is
/// relative to the config that includes it.
fn resolve(directory: &Path, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return config::home_directory().unwrap_or_default().join(rest);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    }
}

/// The identities set up in `scope`, in the order they were added.
pub fn list(scope: &Scope) -> Result<Vec<Identity>, String> {
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
    let hosts: Vec<String> = found
        .iter()
        .filter(|(matcher, _)| matches!(matcher, Match::Host(_)))
        .map(|(_, path)| path.clone())
        .collect();
    found.retain(|(matcher, path)| !(matches!(matcher, Match::Remote(_)) && hosts.contains(path)));

    let mut identities = Vec::new();
    for (matcher, path) in found {
        let file = resolve(&config::config_directory(scope)?, &path);
        let own = config::read(&Scope::File(file))?;
        let text = |key: &str| own.get(key).unwrap_or("").to_owned();
        identities.push(Identity {
            matcher,
            name: text("user.name"),
            email: text("user.email"),
            signing_key: text("user.signingkey"),
        });
    }
    Ok(identities)
}

/// Adds the identity, or changes the one for the same match.
pub fn save(scope: &Scope, identity: &Identity) -> Result<(), String> {
    let matcher = identity.matcher.tidy();
    if let Some(problem) = matcher.problem() {
        return Err(problem.to_owned());
    }
    let (name, email, key) = (
        identity.name.trim(),
        identity.email.trim(),
        identity.signing_key.trim(),
    );
    if name.is_empty() && email.is_empty() && key.is_empty() {
        return Err("Give the identity a name or an email.".to_owned());
    }

    let values = config::read(scope)?;
    let path = values.get(&include_key(&matcher.condition())).map_or_else(
        || format!("{FOLDER_NAME}/{}", matcher.file_name()),
        str::to_owned,
    );
    // Writing the include first makes git create the config where it wants it, so that
    // the folder to put the identity in can then be asked for.
    for condition in matcher.conditions() {
        config::set(scope, &include_key(&condition), &path)?;
    }
    let file = resolve(&config::config_directory(scope)?, &path);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("Couldn't make {}: {error}", folder.display()))?;
    }
    let file = Scope::File(file);
    config::set(&file, "user.name", name)?;
    config::set(&file, "user.email", email)?;
    config::set(&file, "user.signingkey", key)
}

/// Removes the identity for `matcher`, and its file if this app made it.
pub fn remove(scope: &Scope, matcher: &Match) -> Result<(), String> {
    let matcher = matcher.tidy();
    let values = config::read(scope)?;
    let path = values
        .get(&include_key(&matcher.condition()))
        .map(str::to_owned);
    for condition in matcher.conditions() {
        if values.is_set(&include_key(&condition)) {
            scope.run(&["--remove-section", &format!("includeIf.{condition}")])?;
        }
    }
    let ours = format!("{FOLDER_NAME}/{}", matcher.file_name());
    if path.as_deref() == Some(ours.as_str()) {
        let file = resolve(&config::config_directory(scope)?, &ours);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Couldn't delete {}: {error}", file.display())),
        }
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
    pub fn from_sources(sources: &Sources, identities: &[Identity]) -> Committing {
        let name = sources.get("user.name");
        let email = sources.get("user.email");
        let place = |source: Option<&config::Source>| source.map(|s| origin_words(s, identities));
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

/// Who commits made in `repo` are attributed to. `global` is where identities are kept.
pub fn committing_as(repo: &Path, global: &Scope) -> Result<Committing, String> {
    let sources = config::read_sources(&Scope::Repository(repo.to_owned()))?;
    // Without a readable list of identities, the answer is still right, just less specific.
    let identities = list(global).unwrap_or_default();
    Ok(Committing::from_sources(&sources, &identities))
}

fn origin_words(source: &config::Source, identities: &[Identity]) -> String {
    let file = source
        .origin
        .strip_prefix("file:")
        .map(Path::new)
        .filter(|_| source.scope != "local");
    if let Some(file) = file {
        let in_identities = file
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|folder| folder == FOLDER_NAME);
        let name = file.file_name().and_then(|name| name.to_str());
        if in_identities
            && let Some(identity) = identities
                .iter()
                .find(|identity| Some(identity.matcher.file_name().as_str()) == name)
        {
            return format!("the identity for {}", identity.matcher.describe());
        }
    }
    match source.scope.as_str() {
        "system" => "the system config",
        "global" => "your global config",
        "local" => "this repository's config",
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

    fn work() -> Identity {
        Identity {
            matcher: Match::Host("work".into()),
            name: "Ada Lovelace".into(),
            email: "ada@work.example".into(),
            signing_key: String::new(),
        }
    }

    #[test]
    fn identities_round_trip_and_keep_their_case() {
        let (_dir, scope) = scope();
        let folder = Identity {
            matcher: Match::Folder("~/Work".into()),
            ..work()
        };
        save(&scope, &work()).unwrap();
        save(&scope, &folder).unwrap();

        let listed = list(&scope).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0], work());
        assert_eq!(listed[1].matcher, Match::Folder("~/Work/".into()));
        assert_eq!(listed[1].email, "ada@work.example");
    }

    #[test]
    fn saving_again_changes_the_identity_instead_of_adding_one() {
        let (_dir, scope) = scope();
        save(&scope, &work()).unwrap();
        save(
            &scope,
            &Identity {
                email: "new@work.example".into(),
                signing_key: "ABC".into(),
                ..work()
            },
        )
        .unwrap();
        let listed = list(&scope).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].email, "new@work.example");
        assert_eq!(listed[0].signing_key, "ABC");
    }

    #[test]
    fn removing_deletes_the_include_and_its_file() {
        let (dir, scope) = scope();
        save(&scope, &work()).unwrap();
        let folder = dir.path().join(FOLDER_NAME);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);

        remove(&scope, &work().matcher).unwrap();
        assert!(list(&scope).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        let config = std::fs::read_to_string(dir.path().join("config")).unwrap();
        assert!(!config.contains("includeIf"), "{config}");
    }

    #[test]
    fn an_identity_needs_a_match_and_something_to_say() {
        let (_dir, scope) = scope();
        let blank = |matcher| Identity {
            matcher,
            name: String::new(),
            email: String::new(),
            signing_key: String::new(),
        };
        assert!(save(&scope, &blank(Match::Host("x".into()))).is_err());
        for matcher in [
            Match::Remote("  ".into()),
            Match::Host(String::new()),
            Match::Host("git@work".into()),
            Match::Host("a/b".into()),
            Match::Folder(" ".into()),
        ] {
            let identity = Identity {
                matcher: matcher.clone(),
                ..work()
            };
            assert!(save(&scope, &identity).is_err(), "{matcher:?}");
        }
        assert!(list(&scope).unwrap().is_empty());
    }

    /// Asks git, in a new repository with `remote`, what email the identities give.
    fn email_for(scope: &Scope, remote: Option<&str>) -> String {
        // Not the shared fixture, which keeps its own email in the repository and so
        // would rightly win over any include.
        let repo = TempDir::new().unwrap();
        let Scope::File(file) = scope else { panic!() };
        let ask = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                // Point git's global config at the file, so that git itself decides
                // whether the condition applies.
                .env("GIT_CONFIG_GLOBAL", file)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        ask(&["init", "-q"]);
        if let Some(url) = remote {
            ask(&["remote", "add", "origin", url]);
        }
        ask(&["config", "user.email"])
    }

    #[test]
    fn a_host_matches_every_way_of_writing_its_remote() {
        let (_dir, scope) = scope();
        save(&scope, &work()).unwrap();
        for url in [
            "git@work:team/app.git",
            "git@work:app.git",
            "work:team/app.git",
            "ssh://git@work/team/app.git",
            "ssh://work/team/app.git",
        ] {
            let expected = if url.starts_with("work:") {
                // No user at all: not something `*@` can match, and rare.
                ""
            } else {
                "ada@work.example"
            };
            assert_eq!(email_for(&scope, Some(url)), expected, "{url}");
        }
        for url in [
            "git@work2:team/app.git",
            "git@home:me/app.git",
            "https://work/team/app.git",
        ] {
            assert_eq!(email_for(&scope, Some(url)), "", "{url}");
        }
        assert_eq!(email_for(&scope, None), "", "no remote, no match");
    }

    #[test]
    fn a_raw_remote_pattern_and_a_folder_work_too() {
        let (dir, scope) = scope();
        let pattern = Identity {
            matcher: Match::Remote("https://example.com/**".into()),
            ..work()
        };
        save(&scope, &pattern).unwrap();
        assert_eq!(
            email_for(&scope, Some("https://example.com/me/app")),
            "ada@work.example"
        );
        assert_eq!(email_for(&scope, Some("https://other.org/me/app")), "");

        // Only one entry for each, whatever the number of conditions behind it.
        save(&scope, &work()).unwrap();
        assert_eq!(list(&scope).unwrap().len(), 2);

        let folder = dir.path().join("Work Stuff");
        let folder_identity = Identity {
            matcher: Match::Folder(folder.display().to_string().replace('\\', "/")),
            ..work()
        };
        save(&scope, &folder_identity).unwrap();
        assert_eq!(list(&scope).unwrap().len(), 3);
    }

    #[test]
    fn removing_a_host_removes_all_of_its_conditions() {
        let (dir, scope) = scope();
        save(&scope, &work()).unwrap();
        remove(&scope, &work().matcher).unwrap();
        let config = std::fs::read_to_string(dir.path().join("config")).unwrap();
        assert!(!config.contains("includeIf"), "{config}");
        assert_eq!(email_for(&scope, Some("git@work:team/app.git")), "");
    }

    #[test]
    fn committing_says_where_the_name_comes_from() {
        let (dir, scope) = scope();
        save(&scope, &work()).unwrap();
        let identities = list(&scope).unwrap();
        let file = dir
            .path()
            .join(FOLDER_NAME)
            .join(work().matcher.file_name());
        let config = dir.path().join("config");
        let line = |scope: &str, origin: &Path, entry: &str| {
            format!("{scope}\0file:{}\0{entry}\0", origin.display())
        };
        let output = [
            line("global", &config, "user.name\nPretext"),
            line("global", &file, "user.email\nada@work.example"),
        ]
        .concat();
        let committing = Committing::from_sources(&config::parse_sources(&output), &identities);
        assert_eq!(committing.name.as_deref(), Some("Pretext"));
        assert_eq!(committing.email.as_deref(), Some("ada@work.example"));
        assert_eq!(
            committing.from,
            "your global config for the name and the identity for the host work for the email"
        );
    }

    #[test]
    fn with_nothing_set_the_sentence_says_commits_cannot_be_made() {
        let none = Committing::from_sources(&Sources::default(), &[]);
        assert!(none.sentence().starts_with("No name or email"));
    }

    #[test]
    fn identity_files_get_readable_unique_names() {
        let a = Match::Host("work".into()).file_name();
        let b = Match::Folder("~/work/".into()).file_name();
        assert_eq!(a, "hasconfig-remote-url-work.gitconfig");
        assert_ne!(a, b);
        assert_ne!(a, Match::Host("work2".into()).file_name());
    }
}
