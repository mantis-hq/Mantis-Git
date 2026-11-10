//! Fetching by itself. A repository's remotes move on without it, so the app asks them
//! now and then, which keeps "behind" counts and remote branches true without anyone
//! pressing Fetch.
//!
//! It is a fetch and nothing more: no branch is moved and no branch is deleted, and it never
//! asks a question. A key with a passphrase that no agent holds, or a login, makes it fail
//! quietly (the Log says why) until the person fetches by hand and answers.

use crate::ops::{self, run_command};
use std::path::Path;
use std::time::Duration;

/// How long to wait between fetches of one repository.
pub const EVERY: Duration = Duration::from_secs(5 * 60);

/// Fetches every remote without ever asking the person anything.
pub fn fetch(repo: &Path) -> Result<(), String> {
    let mut command = ops::git(repo);
    command.args(["fetch", "--all", "--quiet"]);
    run_command(ops::quietly(&mut command), None).map(|_| ())
}

/// The sentence under "Fetch in the background".
pub fn sentence(on: bool) -> &'static str {
    if on {
        "gitclient fetches every remote every 5 minutes, so what is ahead and behind stays true. \
         It never asks for a passphrase or a login by itself."
    } else {
        "gitclient only fetches when you press Fetch."
    }
}

/// What the header says after that switch is flipped.
pub fn saved_message(on: bool) -> &'static str {
    if on {
        "Saved. gitclient will fetch every 5 minutes."
    } else {
        "Saved. gitclient will only fetch when you press Fetch."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn it_brings_in_what_a_remote_gained_and_moves_no_branch() {
        let origin = Repo::new();
        origin.write("a.txt", "one");
        origin.commit("one");
        let clone = Repo::new();
        clone.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
        clone.git(&["fetch", "-q", "origin"]);
        let branch = origin.git(&["rev-parse", "--abbrev-ref", "HEAD"]);
        clone.git(&["checkout", "-q", "-b", "local", &format!("origin/{branch}")]);
        let before = clone.git(&["rev-parse", "HEAD"]);

        origin.write("a.txt", "two");
        let second = origin.commit("two");
        assert_eq!(fetch(clone.path()), Ok(()));

        assert_eq!(
            clone.git(&["rev-parse", &format!("origin/{branch}")]),
            second
        );
        assert_eq!(clone.git(&["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn a_remote_that_cannot_be_reached_is_an_error_not_a_question() {
        let repo = Repo::new();
        repo.git(&["remote", "add", "origin", "/nowhere/at/all"]);
        assert!(fetch(repo.path()).is_err());
    }

    #[test]
    fn the_sentences_name_the_choice() {
        assert!(sentence(true).contains("every 5 minutes"));
        assert!(sentence(false).contains("only fetches when you press Fetch"));
        assert!(saved_message(true).contains("5 minutes"));
        assert!(saved_message(false).contains("only fetch when"));
    }
}
