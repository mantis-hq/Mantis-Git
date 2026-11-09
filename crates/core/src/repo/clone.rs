//! Cloning a repository from an address into a folder, and the checks that can be made
//! before git runs: what to call the folder, and whether the address and the place can
//! work.

use crate::ops;
use crate::settings::config::home_directory;
use std::path::{Path, PathBuf};

/// The name git would give the folder for `address`: its last part without `.git`, as in
/// `cat` for `https://example.com/octo/cat.git/` and `git@host:octo/cat.git`.
pub fn name_of(address: &str) -> Option<String> {
    let trimmed = address.trim().trim_end_matches(['/', '\\']);
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let name = trimmed.rsplit(['/', '\\', ':']).next()?.trim();
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_owned())
}

/// Whether `text`, such as what is on the clipboard, is an address a repository can be
/// cloned from. It is stricter than what git takes, because it decides whether to fill
/// a form in: a bare word or a sentence is not one.
pub fn looks_like_address(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    if let Some((scheme, rest)) = text.split_once("://") {
        return matches!(scheme, "http" | "https" | "ssh" | "git") && rest.contains('/');
    }
    // `git@host:owner/repo.git`, which has no scheme.
    match text.split_once('@') {
        Some((user, rest)) => {
            !user.is_empty()
                && rest
                    .split_once(':')
                    .is_some_and(|(host, path)| !host.is_empty() && path.contains('/'))
        }
        None => false,
    }
}

/// Where `parent` is on disk: a leading `~` is the home folder.
fn expand(parent: &str) -> PathBuf {
    let parent = parent.trim();
    match parent
        .strip_prefix('~')
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', '\\']))
    {
        Some(rest) => match home_directory() {
            Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
            None => PathBuf::from(parent),
        },
        None => PathBuf::from(parent),
    }
}

/// The folder the clone ends up in: named after the address, inside `parent`.
pub fn destination(address: &str, parent: &str) -> Option<PathBuf> {
    if parent.trim().is_empty() {
        return None;
    }
    Some(expand(parent).join(name_of(address)?))
}

/// The folder the form starts with: the one last cloned into, else the home folder.
pub fn default_parent(remembered: Option<&Path>) -> String {
    remembered
        .map(Path::to_path_buf)
        .or_else(home_directory)
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

/// Why this address and folder can't work, found without running git.
pub fn problem(address: &str, parent: &str) -> Option<String> {
    let address = address.trim();
    if address.is_empty() {
        return Some("Paste the address of the repository to clone.".to_owned());
    }
    if address.starts_with('-') {
        return Some("An address can't start with a dash.".to_owned());
    }
    if name_of(address).is_none() {
        return Some("Can't tell what to call the folder from this address.".to_owned());
    }
    if parent.trim().is_empty() {
        return Some("Say which folder to clone into.".to_owned());
    }
    let folder = expand(parent);
    if folder.is_file() {
        return Some(format!("{} is a file, not a folder.", folder.display()));
    }
    let target = destination(address, parent)?;
    let taken = target
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(target.exists());
    taken.then(|| format!("{} already exists and isn't empty.", target.display()))
}

/// Clones `address` into a folder inside `parent` and returns that folder. It blocks on
/// git, which can take as long as the repository is big, so call it off the UI thread.
pub fn run(address: &str, parent: &str) -> Result<PathBuf, String> {
    if let Some(problem) = problem(address, parent) {
        return Err(problem);
    }
    let target = destination(address, parent).ok_or("Say which folder to clone into.")?;
    let folder = expand(parent);
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("Couldn't make {}: {error}", folder.display()))?;
    let name = name_of(address).unwrap_or_default();
    ops::run(&folder, &["clone", "--", address.trim(), &name])?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn the_folder_is_named_after_the_last_part_of_the_address() {
        for (address, name) in [
            ("https://example.com/octo/cat.git", "cat"),
            ("https://example.com/octo/cat/", "cat"),
            ("git@github.com:octo/cat.git", "cat"),
            ("ssh://git@host:2222/octo/cat.git", "cat"),
            ("  /srv/git/cat.git  ", "cat"),
            ("C:\\code\\cat", "cat"),
        ] {
            assert_eq!(name_of(address).as_deref(), Some(name), "{address}");
        }
        assert_eq!(name_of(""), None);
        assert_eq!(name_of("https://example.com/.."), None);
    }

    #[test]
    fn only_things_that_look_like_addresses_fill_the_form_in() {
        assert!(looks_like_address("https://github.com/octo/cat"));
        assert!(looks_like_address(" git@github.com:octo/cat.git\n"));
        assert!(looks_like_address("ssh://git@host/octo/cat.git"));
        assert!(!looks_like_address("cat"));
        assert!(!looks_like_address("https://example.com"));
        assert!(!looks_like_address("see https://example.com/a/b"));
        assert!(!looks_like_address("file:///etc/passwd"));
        assert!(!looks_like_address("/home/me/cat"));
    }

    #[test]
    fn what_cant_work_is_said_before_git_runs() {
        let dir = Repo::new();
        let parent = dir.path().to_str().unwrap();
        assert!(problem("", parent).unwrap().contains("Paste"));
        assert!(problem("--upload-pack=x", parent).unwrap().contains("dash"));
        assert!(
            problem("https://e.com/a/b.git", " ")
                .unwrap()
                .contains("folder")
        );
        assert_eq!(problem("https://e.com/a/b.git", parent), None);
        dir.write("b/x", "1");
        assert!(
            problem("https://e.com/a/b.git", parent)
                .unwrap()
                .contains("already exists")
        );
    }

    #[test]
    fn cloning_makes_the_folder_and_returns_it() {
        let source = Repo::new();
        source.write("a", "1\n");
        source.commit("base");
        let into = Repo::new();
        let parent = into.path().join("new/place");
        let address = source.path().to_str().unwrap();
        let target = run(address, parent.to_str().unwrap()).unwrap();
        assert_eq!(
            target,
            parent.join(name_of(address).unwrap()),
            "named after the source"
        );
        assert!(target.join("a").is_file());
        assert!(run(address, parent.to_str().unwrap()).is_err(), "now taken");
    }
}
