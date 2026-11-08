//! The commands that act on commits: check one out, copy it onto this branch, revert it,
//! move the branch to it, or apply one file of it to the working changes. Each says how to
//! undo what it did.

use super::details;
use crate::ops;
use crate::text::{count, short_oid};
use std::fs;
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

/// Applies what one commit changed in one file to the working tree, leaving the index and
/// the branch alone. `old_path` is where a renamed file came from, so the rename carries
/// over. A file that has moved on since the commit's parent no longer takes the patch, so
/// the two changes are merged instead; where they clash, the file is left as it was.
pub fn apply_file(
    repo: &Path,
    oid: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<Option<String>, String> {
    let base = details::first_parent(repo, oid);
    let mut args = vec!["diff-tree", "--root", "-r", "-p", "-M", "--binary"];
    args.push("--no-commit-id");
    args.extend(base.as_deref());
    args.extend([oid, "--", path]);
    args.extend(old_path);
    let patch = ops::run(repo, &args)?;
    let name = file_name(path);
    if patch.trim().is_empty() {
        return Err(format!(
            "{} changed nothing in {name} to apply.",
            short_oid(oid)
        ));
    }
    let applied = |undo: &str| {
        Ok(Some(format!(
            "Applied the changes in {name} from {} to your working changes. To undo it, run \
             `{undo}`.",
            short_oid(oid)
        )))
    };
    if let Err(error) = ops::run_with_input(repo, &["apply"], patch.as_bytes()) {
        let merged = match old_path {
            None => merge_file(repo, oid, base.as_deref(), path)?,
            Some(_) => Merge::Clash,
        };
        return match merged {
            Merge::Done { saved } => {
                applied(&format!("git cat-file blob {} > {path}", short_oid(&saved)))
            }
            Merge::AlreadyThere => Err(format!(
                "{name} already has the changes from {}, so nothing was changed.",
                short_oid(oid)
            )),
            Merge::Clash => Err(apply_error(name, error)),
        };
    }

    let mut undo = String::from("git show ");
    if base.is_some() {
        undo.push_str("--diff-merges=first-parent ");
    }
    undo.push_str(&format!("{} -- {path}", short_oid(oid)));
    if let Some(old_path) = old_path {
        undo.push_str(&format!(" {old_path}"));
    }
    undo.push_str(" | git apply -R");
    applied(&undo)
}

/// How merging a commit's change into a file went.
enum Merge {
    /// Written. `saved` is the id of the file's contents before, kept in git's object
    /// store so the merge can be undone.
    Done { saved: String },
    /// The file already holds the change.
    AlreadyThere,
    /// The two changes clash, or the file can't be merged line by line.
    Clash,
}

/// Merges what a commit changed in a file it modified into the file as it is now, for when
/// the patch no longer fits. Nothing is written unless the merge is clean.
fn merge_file(repo: &Path, oid: &str, base: Option<&str>, path: &str) -> Result<Merge, String> {
    let parent = base.map_or_else(|| format!("{oid}^"), str::to_owned);
    let target = repo.join(path);
    let (Ok(current), Some(old), Some(new)) = (
        fs::read(&target),
        working_blob(repo, &parent, path),
        working_blob(repo, oid, path),
    ) else {
        return Ok(Merge::Clash);
    };
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for (side, bytes) in [("current", &current), ("base", &old), ("theirs", &new)] {
        let file = dir.path().join(side);
        fs::write(&file, bytes).map_err(|error| error.to_string())?;
        files.push(file);
    }
    // The exit status is how many places clash, and any of them leaves the file alone.
    let merged = ops::run_unchecked(ops::git(repo).args(["merge-file", "-p"]).args(&files), None)?;
    if !merged.success {
        return Ok(Merge::Clash);
    }
    if merged.stdout == current {
        return Ok(Merge::AlreadyThere);
    }
    let saved = ops::run(repo, &["hash-object", "-w", "--no-filters", "--", path])?;
    fs::write(&target, merged.stdout).map_err(|error| error.to_string())?;
    Ok(Merge::Done {
        saved: saved.trim().to_owned(),
    })
}

/// A file's contents at a commit, as they would be checked out, with line endings and other
/// filters applied. `None` when the commit has no such file.
fn working_blob(repo: &Path, rev: &str, path: &str) -> Option<Vec<u8>> {
    let spec = format!("{rev}:{path}");
    let finished = ops::run_unchecked(ops::git(repo).args(["cat-file", "--filters", &spec]), None);
    finished.ok().filter(|f| f.success).map(|f| f.stdout)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Says in a sentence why a patch didn't go on, when git's reason is that the file isn't
/// what the commit expected.
fn apply_error(name: &str, error: String) -> String {
    let stale = [
        "does not apply",
        "already exists",
        "does not exist",
        "No such file",
    ];
    if stale.iter().any(|reason| error.contains(reason)) {
        format!("The changes in {name} don't fit the file as it is now, so nothing was changed.")
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    /// Runs the shell line a message gives as its undo.
    fn run_undo(repo: &Repo, message: &str) {
        let line = message.split('`').nth(1).unwrap();
        let status = std::process::Command::new("sh")
            .args(["-c", line])
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success(), "{line}");
    }

    #[test]
    fn applying_one_file_of_a_commit_leaves_the_rest_and_the_index_alone() {
        let repo = Repo::new();
        repo.write("a", "1\n2\n3\n");
        repo.write("b", "x\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("a", "1\ntwo\n3\n");
        repo.write("b", "y\n");
        let side = repo.commit("change both");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("a", "0\n1\n2\n3\n");

        let message = apply_file(repo.path(), &side, "a", None).unwrap().unwrap();
        assert_eq!(repo.read("a"), "0\n1\ntwo\n3\n");
        assert_eq!(repo.read("b"), "x\n");
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
        assert_eq!(
            repo.git(&["rev-parse", "HEAD"]),
            repo.git(&["rev-parse", "main"])
        );

        run_undo(&repo, &message);
        assert_eq!(repo.read("a"), "0\n1\n2\n3\n");
    }

    #[test]
    fn applying_a_file_from_the_first_commit_or_a_rename() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        let root = repo.commit("base");
        repo.git(&["mv", "a", "b"]);
        let moved = repo.commit("rename");
        repo.git(&["checkout", "-q", "--detach", &root]);
        std::fs::remove_file(repo.path().join("a")).unwrap();

        apply_file(repo.path(), &root, "a", None).unwrap();
        assert_eq!(repo.read("a"), "1\n");

        let message = apply_file(repo.path(), &moved, "b", Some("a"))
            .unwrap()
            .unwrap();
        assert!(!repo.path().join("a").exists());
        assert_eq!(repo.read("b"), "1\n");
        run_undo(&repo, &message);
        assert_eq!(repo.read("a"), "1\n");
    }

    #[test]
    fn a_patch_that_fits_is_undone_by_reversing_it() {
        let repo = Repo::new();
        repo.write("a", "1\n2\n3\n4\n5\n6\n7\n8\n");
        repo.commit("base");
        repo.write("a", "1\n2\n3\n4\nfive\n6\n7\n8\n");
        let change = repo.commit("change");
        repo.git(&["reset", "-q", "--hard", "HEAD^"]);

        let message = apply_file(repo.path(), &change, "a", None)
            .unwrap()
            .unwrap();
        assert!(message.contains("| git apply -R`"), "{message}");
        assert_eq!(repo.read("a"), "1\n2\n3\n4\nfive\n6\n7\n8\n");
        run_undo(&repo, &message);
        assert_eq!(repo.read("a"), "1\n2\n3\n4\n5\n6\n7\n8\n");
    }

    #[test]
    fn a_change_the_file_already_has_or_one_that_clashes_changes_nothing() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.write("a", "2\n");
        let head = repo.commit("change");

        let error = apply_file(repo.path(), &head, "a", None).unwrap_err();
        assert!(error.starts_with("a already has the changes"), "{error}");

        repo.write("a", "3\n");
        let error = apply_file(repo.path(), &head, "a", None).unwrap_err();
        assert_eq!(
            error,
            "The changes in a don't fit the file as it is now, so nothing was changed."
        );
        assert_eq!(repo.read("a"), "3\n");
    }

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
