//! The submodules a repository has: other repositories it pins at one commit each.
//! Read from `git submodule status`, `.gitmodules` and the index.

use crate::ops;
use crate::text::short_oid;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How a submodule's folder compares with what the repository records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing has been cloned into its folder yet.
    NotDownloaded,
    /// Checked out at the recorded commit.
    Current,
    /// Checked out at some other commit than the recorded one.
    Moved,
    /// The repository's own merge left its record in conflict.
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submodule {
    /// The name in `.gitmodules`, which is usually its path.
    pub name: String,
    /// Where it lives, relative to the top of the repository.
    pub path: String,
    /// The folder on disk.
    pub folder: PathBuf,
    /// Where it is cloned from. Missing when `.gitmodules` has no entry for it.
    pub url: Option<String>,
    /// The commit the repository records for it.
    pub recorded: String,
    /// The commit checked out in it, when it has been downloaded.
    pub head: Option<String>,
    pub state: State,
}

impl Submodule {
    /// What to call it in a list: its path.
    pub fn label(&self) -> &str {
        &self.path
    }

    /// Where it stands, in a few words.
    pub fn detail(&self) -> String {
        match (self.state, &self.head) {
            (State::NotDownloaded, _) => "not downloaded".to_owned(),
            (State::Conflict, _) => "in conflict".to_owned(),
            (State::Moved, Some(head)) => format!("moved to {}", short_oid(head)),
            (_, Some(head)) => short_oid(head).to_owned(),
            (_, None) => String::new(),
        }
    }

    /// Whether there is a repository to open.
    pub fn is_downloaded(&self) -> bool {
        self.state != State::NotDownloaded
    }

    /// The line that updates it, which names its result.
    pub fn update_label(&self) -> String {
        let name = &self.path;
        match self.state {
            State::NotDownloaded => format!("Download {name} at {}", short_oid(&self.recorded)),
            State::Moved => format!("Check out {} in {name}", short_oid(&self.recorded)),
            State::Current | State::Conflict => format!("Update {name}"),
        }
    }

    /// What updating does to a submodule that is on another commit, and how to undo it.
    /// Nothing is lost by the other states, so they need no question.
    pub fn update_warning(&self) -> Option<String> {
        if self.state != State::Moved {
            return None;
        }
        let head = self.head.as_deref()?;
        Some(format!(
            "This checks out {} in {}, which is the commit this repository records. It is on {} \
             now, and any commits made there that no branch holds stay in its reflog. To go back, \
             run `git -C {} checkout {head}`.",
            short_oid(&self.recorded),
            self.path,
            short_oid(head),
            self.path,
        ))
    }

    /// Why it can't be updated, when it can't.
    pub fn update_unavailable(&self) -> Option<&'static str> {
        match self.state {
            State::Conflict => Some("Resolve its conflict first"),
            _ if self.url.is_none() => Some("It has no entry in .gitmodules"),
            _ => None,
        }
    }
}

/// Reads `git submodule status`. Each line is a flag, the commit, the path and, once it
/// is downloaded, a description in brackets.
pub fn parse_status(listing: &str) -> Vec<(State, String, String)> {
    let mut found = Vec::new();
    for line in listing.lines() {
        let mut chars = line.chars();
        let state = match chars.next() {
            Some('-') => State::NotDownloaded,
            Some('+') => State::Moved,
            Some('U') => State::Conflict,
            Some(' ') => State::Current,
            _ => continue,
        };
        let rest = chars.as_str();
        let Some((oid, path)) = rest.split_once(' ') else {
            continue;
        };
        let path = match path.rfind(" (") {
            Some(at) if path.ends_with(')') => &path[..at],
            _ => path,
        };
        found.push((state, oid.to_owned(), path.to_owned()));
    }
    found
}

/// Reads `.gitmodules` as `git config -z` prints it: name to path and url.
fn parse_modules(listing: &str) -> HashMap<String, (Option<String>, Option<String>)> {
    let mut found: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for entry in listing.split('\0') {
        let Some((key, value)) = entry.split_once('\n') else {
            continue;
        };
        let Some(key) = key.strip_prefix("submodule.") else {
            continue;
        };
        // Names can hold dots, so the setting is what follows the last one.
        let Some((name, setting)) = key.rsplit_once('.') else {
            continue;
        };
        let slot = found.entry(name.to_owned()).or_default();
        match setting {
            "path" => slot.0 = Some(value.to_owned()),
            "url" => slot.1 = Some(value.to_owned()),
            _ => {}
        }
    }
    found
}

/// Reads the recorded commit of each submodule from `git ls-files -s -z`.
fn parse_recorded(listing: &str) -> HashMap<String, String> {
    let mut found = HashMap::new();
    for entry in listing.split('\0') {
        let Some((info, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut fields = info.split(' ');
        if let (Some("160000"), Some(oid)) = (fields.next(), fields.next()) {
            found
                .entry(path.to_owned())
                .or_insert_with(|| oid.to_owned());
        }
    }
    found
}

/// The submodules of the repository at `workdir`, by path. Empty when it has none, and
/// when git can't say.
pub fn list(workdir: &Path) -> Vec<Submodule> {
    let Ok(status) = ops::run(workdir, &["submodule", "status"]) else {
        return Vec::new();
    };
    let states = parse_status(&status);
    if states.is_empty() {
        return Vec::new();
    }
    let modules = ops::run(
        workdir,
        &[
            "config",
            "-z",
            "-f",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.(path|url)$",
        ],
    )
    .map(|listing| parse_modules(&listing))
    .unwrap_or_default();
    // `submodule status` already says the recorded commit, except for a submodule that has
    // moved (it says where it is) or is in conflict, so only those are looked up, and by
    // their own paths. Listing the whole index takes over ten seconds in a large
    // repository, and this runs each time the refs are read.
    let differing: Vec<&str> = states
        .iter()
        .filter(|(state, _, _)| matches!(state, State::Moved | State::Conflict))
        .map(|(_, _, path)| path.as_str())
        .collect();
    let recorded = if differing.is_empty() {
        HashMap::new()
    } else {
        let mut args = vec!["--literal-pathspecs", "ls-files", "-s", "-z", "--"];
        args.extend(differing);
        ops::run(workdir, &args)
            .map(|listing| parse_recorded(&listing))
            .unwrap_or_default()
    };

    let mut found: Vec<Submodule> = states
        .into_iter()
        .map(|(state, oid, path)| {
            let named = modules
                .iter()
                .find(|(_, (module_path, _))| module_path.as_deref() == Some(path.as_str()));
            let (name, url) = match named {
                Some((name, (_, url))) => (name.clone(), url.clone()),
                None => (path.clone(), None),
            };
            Submodule {
                name,
                folder: workdir.join(&path),
                url,
                recorded: recorded.get(&path).cloned().unwrap_or_else(|| oid.clone()),
                head: (state != State::NotDownloaded).then_some(oid),
                state,
                path,
            }
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// Downloads `path` if it isn't yet, and checks out the commit the repository records.
/// It reads from the submodule's own url, so it needs the network the first time.
pub fn update(workdir: &Path, path: &str) -> Result<Option<String>, String> {
    ops::run(workdir, &["submodule", "update", "--init", "--", path])?;
    Ok(Some(format!("Updated {path}.")))
}

/// Why `path` can't be where a submodule lives, in words for the person typing it.
/// An empty path is fine when adding, since git names the folder after the address.
pub fn why_bad_path(path: &str) -> Option<&'static str> {
    if path.starts_with('/') || path.split('/').any(|part| part == "..") {
        Some("A submodule's folder has to be inside this repository.")
    } else if path.starts_with('-') {
        Some("A submodule's folder can't start with a dash.")
    } else {
        None
    }
}

/// Adds a submodule cloned from `url` when `original` is `None`, at `path` or, when that
/// is empty, in a folder named after the address. Otherwise changes the submodule at
/// `original`: pointing it at another address and moving it to `path`, when those
/// changed. Either way git stages the change, ready to commit.
pub fn save(
    workdir: &Path,
    original: Option<&str>,
    path: &str,
    url: &str,
) -> Result<Option<String>, String> {
    save_with(workdir, &[], original, path, url)
}

/// [`save`], with switches for git ahead of the command that clones, which tests use to
/// allow cloning from a local path.
fn save_with(
    workdir: &Path,
    switches: &[&str],
    original: Option<&str>,
    path: &str,
    url: &str,
) -> Result<Option<String>, String> {
    let (path, url) = (path.trim().trim_end_matches('/'), url.trim());
    if url.is_empty() {
        return Err("Give the submodule an address to clone from.".to_owned());
    }
    if let Some(reason) = why_bad_path(path) {
        return Err(reason.to_owned());
    }
    let Some(original) = original else {
        let mut args = switches.to_vec();
        args.extend(["submodule", "add", "--", url]);
        if !path.is_empty() {
            args.push(path);
        }
        ops::run(workdir, &args)?;
        let added = match path {
            "" => "a submodule".to_owned(),
            path => path.to_owned(),
        };
        return Ok(Some(format!(
            "Added {added}. It is staged, so commit it to keep it."
        )));
    };
    if path.is_empty() {
        return Err("Give the submodule a folder.".to_owned());
    }
    let before = list(workdir)
        .into_iter()
        .find(|module| module.path == original)
        .ok_or_else(|| format!("There is no submodule at {original} any more."))?;
    let moved = before.path != path;
    let repointed = before.url.as_deref() != Some(url);
    if !moved && !repointed {
        return Ok(Some(format!("{path} is already set up that way.")));
    }
    if repointed {
        ops::run(workdir, &["submodule", "set-url", "--", original, url])?;
        ops::run(workdir, &["add", "--", ".gitmodules"])?;
    }
    if moved {
        // git moves it into a folder that is there, but doesn't make one.
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(workdir.join(parent))
                .map_err(|error| format!("Couldn't make the folder for {path}: {error}"))?;
        }
        ops::run(workdir, &["mv", "--", original, path])?;
    }
    Ok(Some(format!(
        "Saved {path}. The change is staged, so commit it to keep it."
    )))
}

/// Does [`update`] for every submodule.
pub fn update_all(workdir: &Path) -> Result<Option<String>, String> {
    ops::run(workdir, &["submodule", "update", "--init"])?;
    Ok(Some("Updated every submodule.".to_owned()))
}

/// The line that updates all of `modules`, or why it can't, when there is more than one
/// to do and something to do.
pub fn update_all_label(modules: &[Submodule]) -> Option<String> {
    let todo = modules
        .iter()
        .filter(|module| module.state != State::Current)
        .count();
    (todo > 0 && modules.len() > 1)
        .then(|| format!("Update {}", crate::text::count(todo, "submodule")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    const SUPER: &str = "-1111111111111111111111111111111111111111 libs/a\n\
         +2222222222222222222222222222222222222222 libs/b (v1.0-2-gabc)\n\
         U3333333333333333333333333333333333333333 c d (heads/main)\n \
         4444444444444444444444444444444444444444 e (heads/main)\n";

    #[test]
    fn status_lines_give_state_commit_and_path() {
        let got = parse_status(SUPER);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].0, State::NotDownloaded);
        assert_eq!(got[1], (State::Moved, "2".repeat(40), "libs/b".to_owned()));
        assert_eq!(got[2].2, "c d", "paths may hold spaces");
        assert_eq!(got[3].0, State::Current);
    }

    #[test]
    fn gitmodules_names_may_hold_dots() {
        let got = parse_modules(
            "submodule.libs/a.b.path\nlibs/a.b\0submodule.libs/a.b.url\nhttps://x/y.git\0",
        );
        assert_eq!(
            got["libs/a.b"],
            (
                Some("libs/a.b".to_owned()),
                Some("https://x/y.git".to_owned())
            )
        );
    }

    /// A repository that has `lib` as a submodule, and the repository it came from.
    fn with_submodule() -> (Repo, Repo) {
        let lib = Repo::new();
        lib.write("lib.txt", "one");
        lib.commit("lib: first");
        let repo = Repo::new();
        repo.write("app.txt", "x");
        repo.commit("first");
        repo.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            lib.path().to_str().unwrap(),
            "lib",
        ]);
        repo.commit("add lib");
        (repo, lib)
    }

    #[test]
    fn a_repository_without_submodules_lists_none() {
        let repo = Repo::new();
        repo.commit("first");
        assert!(list(repo.path()).is_empty());
    }

    #[test]
    fn a_downloaded_submodule_is_current_until_it_moves() {
        let (repo, lib) = with_submodule();
        let got = list(repo.path());
        assert_eq!(got.len(), 1);
        let module = &got[0];
        assert_eq!(module.path, "lib");
        assert_eq!(module.state, State::Current);
        assert_eq!(module.url.as_deref(), lib.path().to_str());
        assert_eq!(module.head.as_deref(), Some(module.recorded.as_str()));
        assert!(module.update_warning().is_none());

        // A new commit inside it makes it differ from the record.
        let inside = repo.path().join("lib");
        crate::testing::empty_commit(&inside, "lib: second");
        let moved = &list(repo.path())[0];
        assert_eq!(moved.state, State::Moved);
        assert_ne!(moved.head.as_deref(), Some(moved.recorded.as_str()));
        assert!(moved.detail().starts_with("moved to "));
        let warning = moved.update_warning().unwrap();
        assert!(warning.contains("git -C lib checkout "), "{warning}");
        assert_eq!(moved.update_label().split(' ').next(), Some("Check"));
    }

    #[test]
    fn a_fresh_clone_has_it_not_downloaded_and_updating_downloads_it() {
        let (repo, _lib) = with_submodule();
        let clone = tempfile::tempdir().unwrap();
        let target = clone.path().join("clone");
        crate::testing::git(
            clone.path(),
            &["clone", "-q", repo.path().to_str().unwrap(), "clone"],
        );

        let module = list(&target).remove(0);
        assert_eq!(module.state, State::NotDownloaded);
        assert!(!module.is_downloaded());
        assert_eq!(module.detail(), "not downloaded");
        assert!(module.update_label().starts_with("Download lib at "));
        assert!(module.update_warning().is_none(), "nothing there to lose");

        // Cloning from a local path needs a switch the app's own git calls leave off.
        crate::testing::git(
            &target,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
            ],
        );
        assert_eq!(list(&target)[0].state, State::Current);
    }

    #[test]
    fn updating_puts_a_moved_submodule_back_on_the_recorded_commit() {
        let (repo, _lib) = with_submodule();
        crate::testing::empty_commit(&repo.path().join("lib"), "lib: second");
        assert_eq!(list(repo.path())[0].state, State::Moved);

        assert_eq!(
            update(repo.path(), "lib"),
            Ok(Some("Updated lib.".to_owned()))
        );
        assert_eq!(list(repo.path())[0].state, State::Current);
        assert_eq!(
            update_all(repo.path()),
            Ok(Some("Updated every submodule.".to_owned()))
        );
    }

    #[test]
    fn adding_repointing_and_moving_a_submodule_stages_each_change() {
        let lib = Repo::new();
        lib.write("lib.txt", "one");
        lib.commit("lib: first");
        let other = Repo::new();
        other.commit("other: first");
        let repo = Repo::new();
        repo.write("app.txt", "x");
        repo.commit("first");
        let lib_url = lib.path().to_str().unwrap();

        // Cloning from a local path needs a switch the app's own git calls leave off.
        let allow = ["-c", "protocol.file.allow=always"];
        let said = save_with(repo.path(), &allow, None, "vendor/lib/", lib_url).unwrap();
        assert!(said.unwrap().starts_with("Added vendor/lib."));
        let added = &list(repo.path())[0];
        assert_eq!(
            (added.path.as_str(), added.url.as_deref()),
            ("vendor/lib", Some(lib_url))
        );

        let other_url = other.path().to_str().unwrap();
        save(repo.path(), Some("vendor/lib"), "libs/lib", other_url).unwrap();
        let saved = list(repo.path());
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].path, "libs/lib");
        assert_eq!(saved[0].url.as_deref(), Some(other_url));
        let unstaged = repo.git(&["diff", "--name-only"]);
        assert_eq!(unstaged.trim(), "", "everything is staged: {unstaged}");

        let same = save(repo.path(), Some("libs/lib"), "libs/lib", other_url);
        assert!(same.unwrap().unwrap().contains("already"));
    }

    #[test]
    fn bad_submodule_input_is_refused_before_git_runs() {
        let repo = Repo::new();
        let refused = |original, path, url| save(repo.path(), original, path, url).unwrap_err();
        assert!(refused(None, "lib", " ").contains("address"));
        assert!(refused(None, "../lib", "u").contains("inside"));
        assert!(refused(None, "/lib", "u").contains("inside"));
        assert!(refused(None, "-lib", "u").contains("dash"));
        assert!(refused(Some("lib"), "", "u").contains("folder"));
        assert!(refused(Some("lib"), "lib", "u").contains("no submodule"));
        assert_eq!(why_bad_path(""), None, "git names it after the address");
    }

    #[test]
    fn only_a_submodule_that_has_work_counts_for_update_all() {
        let make = |state| Submodule {
            name: "x".into(),
            path: "x".into(),
            folder: PathBuf::from("/x"),
            url: Some("u".into()),
            recorded: "a".repeat(40),
            head: Some("a".repeat(40)),
            state,
        };
        let two = [make(State::Current), make(State::Moved)];
        assert_eq!(
            update_all_label(&two).as_deref(),
            Some("Update 1 submodule")
        );
        assert_eq!(update_all_label(&[make(State::Moved)]), None);
        assert_eq!(
            update_all_label(&[make(State::Current), make(State::Current)]),
            None
        );
    }
}
