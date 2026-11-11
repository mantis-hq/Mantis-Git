//! Which forge a repository is on when Mantis Git can't tell, and the choice that says. The
//! Repository page shows and sets it; it lives in the repository's own git config as
//! `gitclient.forge`, which is where a name and an email already live.

use super::{Forge, Hosting, hosting, origin_first};
use crate::ops;
use crate::repo::remote;
use crate::settings::config::{self, Scope};
use std::path::Path;

/// The git config key the choice is kept under.
pub const KEY: &str = "gitclient.forge";

/// The forge chosen for the repository, if one was.
pub fn chosen(repo: &Path) -> Option<Forge> {
    let value = ops::run(repo, &["config", "--local", "--get", KEY]).ok()?;
    Forge::from_config(&value)
}

/// Chooses `forge` for the repository, or with `None` takes the choice back so that
/// Mantis Git tells by itself.
pub fn set(repo: &Path, forge: Option<Forge>) -> Result<(), String> {
    let scope = Scope::Repository(repo.to_owned());
    config::set(&scope, KEY, forge.map_or("", Forge::config_value))
}

/// How the repository's forge stands, for the page that sets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub chosen: Option<Forge>,
    /// The remote on the web that the rest is about, `origin` if it is on the web.
    pub remote: Option<String>,
    /// The forge Mantis Git can tell it is without being told.
    pub detected: Option<Forge>,
}

/// Reads how the repository's forge stands.
pub fn read(repo: &Path) -> Choice {
    let chosen = chosen(repo);
    let remotes = remote::list(repo);
    let on_the_web = origin_first(&remotes).find(|remote| hosting(remote, None).is_some());
    let detected = on_the_web.and_then(|remote| match hosting(remote, None)? {
        Hosting::Known(site) => Some(site.forge),
        Hosting::Unknown(_) => None,
    });
    Choice {
        chosen,
        remote: on_the_web.map(|remote| remote.name.clone()),
        detected,
    }
}

/// What the popup says when a request was asked for on a remote whose forge isn't known.
pub fn unknown_question(remote: &str) -> String {
    format!(
        "Mantis Git can't tell whether {remote} is on GitHub, GitLab or Gitea, so it doesn't \
         know which page starts a request. If it is on one of them, say which in this \
         repository's settings."
    )
}

impl Choice {
    /// What the page says above the chips.
    pub fn sentence(&self) -> String {
        let Some(remote) = &self.remote else {
            return "This repository has no remote on the web, so there is no forge to choose."
                .to_owned();
        };
        match (self.chosen, self.detected) {
            (Some(forge), _) => format!(
                "Mantis Git treats {remote} as hosted on {}, as chosen here.",
                forge.name()
            ),
            (None, Some(forge)) => {
                format!("Mantis Git can tell that {remote} is on {}.", forge.name())
            }
            (None, None) => format!(
                "Mantis Git can't tell whether {remote} is on GitHub, GitLab or Gitea. \
                 Choose the one it is on, if any."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn a_choice_is_kept_in_the_repository_and_can_be_taken_back() {
        let repo = Repo::new();
        assert_eq!(chosen(repo.path()), None);
        set(repo.path(), Some(Forge::GitLab)).unwrap();
        assert_eq!(chosen(repo.path()), Some(Forge::GitLab));
        assert_eq!(repo.git(&["config", "--local", KEY]).trim(), "gitlab");
        set(repo.path(), Some(Forge::Gitea)).unwrap();
        assert_eq!(chosen(repo.path()), Some(Forge::Gitea));
        set(repo.path(), None).unwrap();
        assert_eq!(chosen(repo.path()), None);
        set(repo.path(), None).unwrap();
    }

    #[test]
    fn the_sentence_says_what_is_known_and_what_can_be_done() {
        let repo = Repo::new();
        assert_eq!(
            read(repo.path()).sentence(),
            "This repository has no remote on the web, so there is no forge to choose."
        );

        repo.git(&["remote", "add", "origin", "git@gitlab.com:team/app.git"]);
        let choice = read(repo.path());
        assert_eq!(choice.detected, Some(Forge::GitLab));
        assert_eq!(
            choice.sentence(),
            "Mantis Git can tell that origin is on GitLab."
        );

        // Nothing at this address answers, so only a choice can settle it.
        repo.git(&[
            "remote",
            "set-url",
            "origin",
            "http://127.0.0.1:1/me/app.git",
        ]);
        let choice = read(repo.path());
        assert_eq!(choice.detected, None);
        assert!(
            choice
                .sentence()
                .starts_with("Mantis Git can't tell whether origin")
        );

        set(repo.path(), Some(Forge::Gitea)).unwrap();
        assert_eq!(
            read(repo.path()).sentence(),
            "Mantis Git treats origin as hosted on Gitea, as chosen here."
        );
    }
}
