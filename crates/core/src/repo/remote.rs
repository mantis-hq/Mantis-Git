//! A repository's remotes: the names and addresses it fetches from and pushes to, and
//! adding, changing and removing them.

use crate::ops;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
    /// Set when pushes go somewhere other than `url`.
    pub push_url: Option<String>,
}

/// Reads `git remote -v`, which lists each remote once for fetching and once for pushing.
pub fn parse(listing: &str) -> Vec<Remote> {
    let mut found: Vec<Remote> = Vec::new();
    for line in listing.lines() {
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some((address, kind)) = rest.rsplit_once(' ') else {
            continue;
        };
        let at = match found.iter().position(|remote| remote.name == name) {
            Some(at) => at,
            None => {
                found.push(Remote {
                    name: name.to_owned(),
                    ..Remote::default()
                });
                found.len() - 1
            }
        };
        match kind {
            "(fetch)" => found[at].url = address.to_owned(),
            "(push)" => found[at].push_url = Some(address.to_owned()),
            _ => {}
        }
    }
    for remote in &mut found {
        if remote.push_url.as_deref() == Some(remote.url.as_str()) {
            remote.push_url = None;
        }
    }
    found
}

/// The repository's remotes in the order git lists them. Empty when git can't say.
pub fn list(repo: &Path) -> Vec<Remote> {
    ops::run(repo, &["remote", "-v"]).map_or_else(|_| Vec::new(), |listing| parse(&listing))
}

/// The remote that a remote branch such as `origin/feat/x` belongs to, and the branch's name
/// on it. A remote's name can hold a slash, so the longest match wins.
pub fn split_branch<'a>(remotes: &'a [Remote], name: &'a str) -> Option<(&'a Remote, &'a str)> {
    remotes
        .iter()
        .filter_map(|remote| {
            let branch = name.strip_prefix(remote.name.as_str())?.strip_prefix('/')?;
            Some((remote, branch))
        })
        .max_by_key(|(remote, _)| remote.name.len())
}

/// Why `name` can't be a remote's name, in words for the person typing it.
pub fn why_bad_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("Give the remote a name, such as origin.")
    } else if name.starts_with('-') || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some("A remote's name can't start with a dash or hold spaces.")
    } else {
        None
    }
}

/// Adds a remote when `original` is `None`, otherwise changes the remote called
/// `original`: renaming it, and pointing it somewhere else, when those changed. Renaming
/// moves its remote branches too, which is git's own behaviour.
pub fn save(
    repo: &Path,
    original: Option<&str>,
    name: &str,
    url: &str,
) -> Result<Option<String>, String> {
    let (name, url) = (name.trim(), url.trim());
    if let Some(reason) = why_bad_name(name) {
        return Err(reason.to_owned());
    }
    if url.is_empty() {
        return Err("Give the remote an address to fetch from.".to_owned());
    }
    let Some(original) = original else {
        ops::run(repo, &["remote", "add", name, url])?;
        return Ok(Some(format!("Added the remote {name}.")));
    };
    let before = list(repo)
        .into_iter()
        .find(|remote| remote.name == original)
        .ok_or_else(|| format!("There is no remote called {original} any more."))?;
    if before.name == name && before.url == url {
        return Ok(Some(format!("{name} is already set up that way.")));
    }
    if before.name != name {
        ops::run(repo, &["remote", "rename", original, name])?;
    }
    if before.url != url {
        ops::run(repo, &["remote", "set-url", name, url])?;
    }
    Ok(Some(format!("Saved the remote {name}.")))
}

/// Takes a remote away, along with its remote branches. Local branches stay.
pub fn remove(repo: &Path, name: &str) -> Result<Option<String>, String> {
    ops::run(repo, &["remote", "remove", name])?;
    Ok(Some(format!("Removed the remote {name}.")))
}

/// What removing `remote` does, and how to bring it back.
pub fn remove_warning(remote: &Remote) -> String {
    let (name, url) = (&remote.name, &remote.url);
    format!(
        "This forgets the remote {name} and its remote branches here. Your own branches stay, \
         and nothing changes on the server. To bring it back, run `git remote add {name} {url}` \
         and fetch."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn the_listing_pairs_fetch_and_push_addresses_and_keeps_only_a_different_push() {
        let got = parse(
            "origin\tgit@host:a.git (fetch)\norigin\tgit@host:a.git (push)\n\
             up\thttps://x/up.git (fetch)\nup\thttps://x/other.git (push)\n",
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].push_url, None);
        assert_eq!(got[1].url, "https://x/up.git");
        assert_eq!(got[1].push_url.as_deref(), Some("https://x/other.git"));
    }

    #[test]
    fn adding_renaming_repointing_and_removing_a_remote() {
        let repo = Repo::new();
        let path = repo.path();
        save(path, None, "origin", " https://example.com/a.git ").unwrap();
        assert_eq!(list(path)[0].url, "https://example.com/a.git");

        save(
            path,
            Some("origin"),
            "upstream",
            "https://example.com/b.git",
        )
        .unwrap();
        let remotes = list(path);
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "upstream");
        assert_eq!(remotes[0].url, "https://example.com/b.git");

        let message = save(
            path,
            Some("upstream"),
            "upstream",
            "https://example.com/b.git",
        );
        assert!(message.unwrap().unwrap().contains("already"));

        remove(path, "upstream").unwrap();
        assert!(list(path).is_empty());
    }

    #[test]
    fn bad_input_is_refused_with_a_reason_before_git_runs() {
        let repo = Repo::new();
        assert!(
            save(repo.path(), None, "", "u")
                .unwrap_err()
                .contains("name")
        );
        assert!(save(repo.path(), None, "a b", "u").is_err());
        assert!(save(repo.path(), None, "-x", "u").is_err());
        assert!(
            save(repo.path(), None, "o", " ")
                .unwrap_err()
                .contains("address")
        );
        assert!(save(repo.path(), Some("nope"), "nope", "u").is_err());
        // A name that is already taken is git's to refuse.
        save(repo.path(), None, "o", "u").unwrap();
        assert!(save(repo.path(), None, "o", "v").is_err());
    }

    #[test]
    fn the_removal_warning_says_how_to_add_it_back() {
        let remote = Remote {
            name: "origin".into(),
            url: "git@host:a.git".into(),
            push_url: None,
        };
        assert!(remove_warning(&remote).contains("git remote add origin git@host:a.git"));
    }
}
