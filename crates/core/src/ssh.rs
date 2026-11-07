//! Which SSH host and key each remote of a repository will use. Read only: the installed
//! OpenSSH resolves a host name with `ssh -G`, so `Match` blocks, includes and defaults
//! are all its business, and nothing here parses or writes SSH config beyond listing the
//! names of the hosts someone gave a block to.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::config::home_directory;
use crate::ops;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What `ssh` will do for one host name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    /// The name as typed: in a remote it is the part after `@`.
    pub alias: String,
    pub hostname: String,
    pub user: String,
    /// The key files it will offer, as written in the config.
    pub keys: Vec<String>,
    /// Whether those are OpenSSH's own guesses because the config names no key.
    pub default_keys: bool,
}

impl Host {
    /// "git@github.com", for a line of text.
    pub fn login(&self) -> String {
        format!("{}@{}", self.user, self.hostname)
    }

    /// The key files without their folder: "id_ed25519".
    pub fn key_names(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(|key| {
                key.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(key.as_str())
                    .to_owned()
            })
            .collect()
    }
}

/// A remote of a repository, and the SSH host it goes through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHost {
    pub remote: String,
    pub url: String,
    /// `None` when the remote isn't SSH.
    pub alias: Option<String>,
}

/// The file OpenSSH reads first.
pub fn config_file() -> Option<PathBuf> {
    home_directory().map(|home| home.join(".ssh").join("config"))
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

/// Reads `ssh -G` output, one `name value` per line.
pub fn parse_resolved(alias: &str, output: &str) -> Host {
    let mut host = Host {
        alias: alias.to_owned(),
        hostname: alias.to_owned(),
        user: String::new(),
        keys: Vec::new(),
        default_keys: false,
    };
    for line in output.lines() {
        let Some((name, value)) = line.split_once(' ') else {
            continue;
        };
        match name {
            "hostname" => host.hostname = value.to_owned(),
            "user" => host.user = value.to_owned(),
            "identityfile" => host.keys.push(value.to_owned()),
            _ => {}
        }
    }
    host
}

/// Asks the installed `ssh` what it would do for `alias`. No connection is made.
pub fn resolve(alias: &str) -> Result<Host, String> {
    if alias.starts_with('-') {
        return Err("That isn't a host name.".to_owned());
    }
    let output = Command::new("ssh")
        .args(["-G", alias])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| {
            "OpenSSH wasn't found, so the keys can't be looked up. \
             Git can still use SSH if it finds ssh itself."
                .to_owned()
        })?;
    if !output.status.success() {
        return Err(ops::stderr_message(&output.stderr));
    }
    Ok(parse_resolved(
        alias,
        &String::from_utf8_lossy(&output.stdout),
    ))
}

/// A name no config will have a block for, to ask what ssh does for a stranger.
const PROBE: &str = "gitclient-no-such-host.invalid";

/// Resolves every host in `aliases`, marking the ones that use the keys ssh would offer
/// any host. Asking beats keeping a list of key types, which OpenSSH keeps adding to.
/// Hosts ssh refuses to describe are left out; only a missing `ssh` is an error.
pub fn resolve_all(aliases: &[String]) -> Result<Vec<Host>, String> {
    let probe = resolve(PROBE)?;
    let mut hosts = Vec::new();
    for alias in aliases {
        if let Ok(mut host) = resolve(alias) {
            host.default_keys = !host.keys.is_empty() && host.keys == probe.keys;
            hosts.push(host);
        }
    }
    Ok(hosts)
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
    fn resolved_output_gives_the_login_and_the_keys() {
        let output = "\
host github-pretext
user git
hostname github.com
port 22
identitiesonly yes
identityfile ~/.ssh/id_pretext
";
        let host = parse_resolved("github-pretext", output);
        assert_eq!(host.login(), "git@github.com");
        assert_eq!(host.key_names(), ["id_pretext"]);
        assert!(!host.default_keys);
    }

    #[test]
    fn a_host_with_no_key_of_its_own_is_called_default() {
        // Needs ssh, which any machine that can use SSH remotes has.
        let Ok(hosts) = resolve_all(&[PROBE.to_owned(), "also-nothing.invalid".to_owned()]) else {
            return;
        };
        assert_eq!(hosts.len(), 2);
        assert!(hosts.iter().all(|host| host.default_keys), "{hosts:?}");
        assert!(!parse_resolved("h", "identityfile ~/.ssh/work").default_keys);
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
