//! What to suggest when someone sets up an identity: the SSH hosts their remotes and
//! `~/.ssh/config` mention, and the key files in `~/.ssh`. Suggestions only. Nothing here
//! writes SSH config, and the key itself is chosen in the identity and handed to git.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::ops;
use crate::settings::config::home_directory;
use std::path::{Path, PathBuf};

/// A remote of a repository, and the SSH host it goes through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHost {
    pub remote: String,
    pub url: String,
    /// `None` when the remote isn't SSH.
    pub alias: Option<String>,
}

/// The folder OpenSSH keeps keys in.
fn ssh_folder() -> Option<PathBuf> {
    home_directory().map(|home| home.join(".ssh"))
}

/// The file OpenSSH reads first.
pub fn config_file() -> Option<PathBuf> {
    ssh_folder().map(|folder| folder.join("config"))
}

/// The host names that have a block of their own in `config`, in order. Patterns such as
/// `*` or `!x` aren't names of anything and are left out.
pub fn aliases(config: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let Some((word, rest)) = line.split_once(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        if !word.eq_ignore_ascii_case("host") {
            continue;
        }
        for name in rest.trim_start_matches(['=', ' ', '\t']).split_whitespace() {
            let name = name.trim_matches('"');
            if !name.is_empty()
                && !name.contains(['*', '?', '!'])
                && !found.iter().any(|known| known == name)
            {
                found.push(name.to_owned());
            }
        }
    }
    found
}

/// The private keys in `folder`: files that have a `.pub` beside them. That is how
/// `ssh-keygen` leaves a pair, and it keeps `config` and `known_hosts` out.
pub fn keys_in(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut keys: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|ext| ext != "pub"))
        .filter(|path| {
            let mut public = path.as_os_str().to_owned();
            public.push(".pub");
            Path::new(&public).is_file()
        })
        .collect();
    keys.sort();
    keys
}

/// The private keys in `~/.ssh`.
pub fn keys() -> Vec<PathBuf> {
    ssh_folder()
        .map(|folder| keys_in(&folder))
        .unwrap_or_default()
}

/// A key as it is written in a config: with a `~` for the home folder and forward
/// slashes, so it reads the same on every system.
pub fn display_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if let Some(home) = home_directory() {
        let home = home.to_string_lossy().replace('\\', "/");
        let prefix = format!("{}/", home.trim_end_matches('/'));
        if let Some(rest) = text.strip_prefix(&prefix) {
            return format!("~/{rest}");
        }
    }
    text
}

/// The host a remote URL goes to over SSH, as `ssh` would be asked for it. `None` for
/// HTTPS, local paths and anything else that isn't SSH.
pub fn remote_alias(url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host = authority.rsplit('@').next()?;
        // A port isn't part of the name ssh is asked for.
        let host = host.split(':').next()?;
        return (!host.is_empty()).then(|| host.to_owned());
    }
    if url.contains("://") {
        return None;
    }
    // `user@host:path`. Anything with a slash before the colon is a path.
    let (login, _) = url.split_once(':')?;
    if login.contains('/') || login.contains('\\') {
        return None;
    }
    // A Windows drive, `C:\repo`, is also a letter and a colon.
    if login.len() == 1 && url[1..].starts_with(":\\") {
        return None;
    }
    let host = login.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

/// The remotes of `repo` with the SSH host each one goes through.
pub fn remotes(repo: &Path) -> Result<Vec<RemoteHost>, String> {
    // Exits with nothing to say when there are no remotes.
    let output =
        ops::run(repo, &["config", "--get-regexp", r"^remote\..*\.url$"]).unwrap_or_default();
    Ok(output
        .lines()
        .filter_map(|line| {
            let (key, url) = line.split_once(' ')?;
            let remote = key.strip_prefix("remote.")?.strip_suffix(".url")?;
            Some(RemoteHost {
                remote: remote.to_owned(),
                url: url.to_owned(),
                alias: remote_alias(url),
            })
        })
        .collect())
}

/// The names in `~/.ssh/config`, then the ones only a remote of `repo` mentions.
pub fn host_names(repo: Option<&Path>) -> Vec<String> {
    let mut names = config_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .map(|text| aliases(&text))
        .unwrap_or_default();
    if let Some(repo) = repo {
        for remote in remotes(repo).unwrap_or_default() {
            if let Some(alias) = remote.alias
                && !names.contains(&alias)
            {
                names.push(alias);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_blocks_are_listed_without_patterns() {
        let config = "\
# my hosts
Host github.com
  IdentityFile ~/.ssh/id_kaj
Host Yoda yoda
Host *
  ServerAliveInterval 30
host=storagebox
Host *.internal !secret nas
  User me
Host github.com
";
        assert_eq!(
            aliases(config),
            ["github.com", "Yoda", "yoda", "storagebox", "nas"]
        );
    }

    #[test]
    fn remote_urls_name_the_host_ssh_is_asked_for() {
        assert_eq!(
            remote_alias("git@github-pretext:me/repo.git").as_deref(),
            Some("github-pretext")
        );
        assert_eq!(
            remote_alias("github.com:me/repo").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            remote_alias("ssh://git@host.example:2222/me/repo").as_deref(),
            Some("host.example")
        );
        assert_eq!(
            remote_alias("ssh://alias/me/repo").as_deref(),
            Some("alias")
        );
        assert_eq!(remote_alias("https://github.com/me/repo.git"), None);
        assert_eq!(remote_alias("/srv/git/repo"), None);
        assert_eq!(remote_alias("../repo"), None);
        assert_eq!(remote_alias(r"C:\repos\app"), None);
        assert_eq!(remote_alias("file:///srv/repo"), None);
    }

    #[test]
    fn only_key_pairs_are_offered_as_keys() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "id_work",
            "id_work.pub",
            "id_home",
            "id_home.pub",
            "config",
            "known_hosts",
            "lonely.pub",
            "no_public_half",
        ] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        std::fs::create_dir(dir.path().join("a_folder")).unwrap();
        let names: Vec<String> = keys_in(dir.path())
            .iter()
            .map(|key| key.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["id_home", "id_work"]);
        assert!(keys_in(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn remotes_of_a_repository_are_matched_to_hosts() {
        let repo = crate::testing::Repo::new();
        crate::testing::git(
            repo.path(),
            &["remote", "add", "origin", "git@work:team/app.git"],
        );
        crate::testing::git(
            repo.path(),
            &["remote", "add", "web", "https://example.com/app"],
        );
        let remotes = remotes(repo.path()).unwrap();
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].remote, "origin");
        assert_eq!(remotes[0].alias.as_deref(), Some("work"));
        assert_eq!(remotes[1].alias, None);
        assert!(host_names(Some(repo.path())).contains(&"work".to_owned()));
    }

    #[test]
    fn a_repository_without_remotes_has_none() {
        let repo = crate::testing::Repo::new();
        assert!(remotes(repo.path()).unwrap().is_empty());
    }
}
