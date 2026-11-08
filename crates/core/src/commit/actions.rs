//! The commands that act on commits: check one out, copy it onto this branch, revert it,
//! or move the branch to it. Each says how to undo what it did.

use crate::ops;
use crate::text::{count, short_oid};
use std::path::Path;

/// Switches to a branch, or to the bare commit when no branch names it.
pub fn check_out(repo: &Path, branch: Option<&str>, oid: &str) -> Result<Option<String>, String> {
    match branch {
        Some(branch) => ops::run(repo, &["switch", branch]).map(|_| None),
        None => ops::run(repo, &["switch", "--detach", oid]).map(|_| {
            Some(format!(
                "Checked out {} without a branch. Switch to a branch to get back.",
                short_oid(oid)
            ))
        }),
    }
}

pub fn cherry_pick(repo: &Path, oid: &str) -> Result<Option<String>, String> {
    ops::run(repo, &["cherry-pick", oid])?;
    Ok(Some(format!(
        "Copied {} onto this branch as a new commit. Revert it to undo.",
        short_oid(oid)
    )))
}

pub fn revert(repo: &Path, oid: &str, is_merge: bool) -> Result<Option<String>, String> {
    let mut args = vec!["revert", "--no-edit"];
    if is_merge {
        args.extend(["-m", "1"]);
    }
    args.push(oid);
    ops::run(repo, &args)?;
    Ok(Some(format!(
        "Added a commit that reverts {}. Revert that one to undo.",
        short_oid(oid)
    )))
}

/// Copies several commits onto this branch, oldest first.
pub fn cherry_pick_many(repo: &Path, oids: &[String]) -> Result<Option<String>, String> {
    let mut args = vec!["cherry-pick"];
    args.extend(oids.iter().map(String::as_str));
    ops::run(repo, &args)?;
    Ok(Some(format!(
        "Copied {} onto this branch. Revert them to undo.",
        count(oids.len(), "commit")
    )))
}

/// Reverts several commits, newest first, so that each undoes what is still there.
pub fn revert_many(repo: &Path, oids: &[String]) -> Result<Option<String>, String> {
    let mut args = vec!["revert", "--no-edit"];
    args.extend(oids.iter().map(String::as_str));
    ops::run(repo, &args)?;
    Ok(Some(format!(
        "Added {} that revert them. Revert those to undo.",
        count(oids.len(), "commit")
    )))
}

/// Moves the current branch to `oid`. A hard reset also discards working changes.
pub fn reset(repo: &Path, oid: &str, hard: bool) -> Result<Option<String>, String> {
    let old = ops::run(repo, &["rev-parse", "HEAD"])?;
    let mode = if hard { "--hard" } else { "--mixed" };
    ops::run(repo, &["reset", mode, oid])?;
    Ok(Some(format!(
        "Moved this branch to {}. To undo it, run `git reset --hard {}`.",
        short_oid(oid),
        short_oid(old.trim())
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn checking_out_a_commit_says_how_to_get_back() {
        let repo = Repo::new();
        let first = repo.commit("first");
        repo.commit("second");

        let message = check_out(repo.path(), None, &first).unwrap().unwrap();
        assert!(message.contains("without a branch"));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), first);

        assert_eq!(check_out(repo.path(), Some("main"), &first), Ok(None));
        assert_eq!(repo.git(&["branch", "--show-current"]), "main");
    }

    #[test]
    fn cherry_pick_and_revert_add_commits_and_say_how_to_undo() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b", "2\n");
        let side = repo.commit("add b");
        repo.git(&["checkout", "-q", "main"]);

        let message = cherry_pick(repo.path(), &side).unwrap().unwrap();
        assert!(message.contains("Revert it to undo"));
        assert!(repo.path().join("b").exists());

        let picked = repo.git(&["rev-parse", "HEAD"]);
        revert(repo.path(), &picked, false).unwrap();
        assert!(!repo.path().join("b").exists());
    }

    #[test]
    fn reverting_a_merge_undoes_the_side_branch() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b", "2\n");
        repo.commit("add b");
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);

        let merge = repo.git(&["rev-parse", "HEAD"]);
        revert(repo.path(), &merge, true).unwrap();
        assert!(!repo.path().join("b").exists());
    }

    #[test]
    fn a_conflicting_cherry_pick_reports_it_and_leaves_the_repo_to_resolve() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("a", "side\n");
        let side = repo.commit("side");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("a", "main\n");
        repo.commit("main");

        assert!(cherry_pick(repo.path(), &side).is_err());
        assert!(
            !repo
                .git(&["diff", "--name-only", "--diff-filter=U"])
                .is_empty()
        );
    }

    #[test]
    fn several_commits_are_copied_in_order_and_reverted_together() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b", "2\n");
        let first = repo.commit("add b");
        repo.write("c", "3\n");
        let second = repo.commit("add c");
        repo.git(&["checkout", "-q", "main"]);

        let message = cherry_pick_many(repo.path(), &[first, second])
            .unwrap()
            .unwrap();
        assert!(message.contains("2 commits"));
        assert!(repo.path().join("b").exists() && repo.path().join("c").exists());

        let head = repo.git(&["rev-parse", "HEAD"]);
        let before = repo.git(&["rev-parse", "HEAD~1"]);
        revert_many(repo.path(), &[head, before]).unwrap();
        assert!(!repo.path().join("b").exists() && !repo.path().join("c").exists());
    }

    #[test]
    fn a_reset_keeps_or_discards_changes_and_says_how_to_go_back() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        let first = repo.commit("one");
        repo.write("a", "2\n");
        repo.commit("two");

        let message = reset(repo.path(), &first, false).unwrap().unwrap();
        assert!(message.contains("git reset --hard"));
        assert_eq!(repo.read("a"), "2\n");

        reset(repo.path(), &first, true).unwrap();
        assert_eq!(repo.read("a"), "1\n");
    }
}
