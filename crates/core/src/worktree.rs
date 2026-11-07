//! The working trees a repository has, from `git worktree list`.

use crate::ops;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The short name of the branch checked out here, if it is on one.
    pub branch: Option<String>,
    pub head: String,
    /// This is the one the repository was cloned into, which holds the `.git` folder.
    pub is_main: bool,
    /// This is the one the app has open.
    pub is_current: bool,
    pub locked: bool,
    /// Its folder is gone, and `git worktree prune` would forget it.
    pub missing: bool,
}

impl Worktree {
    /// What to call it in a list: its folder's name.
    pub fn label(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Where it stands, in a few words: its branch, or the commit when it has none.
    pub fn detail(&self) -> String {
        match &self.branch {
            Some(branch) => branch.clone(),
            None => format!("detached at {}", crate::text::short_oid(&self.head)),
        }
    }

    /// Why it can't be switched to, when it can't.
    pub fn unavailable(&self) -> Option<&'static str> {
        self.missing.then_some("Its folder is gone")
    }
}

/// Reads git's porcelain listing. Bare entries are left out: they have nothing to show.
pub fn parse(listing: &str) -> Vec<Worktree> {
    let mut found: Vec<Worktree> = Vec::new();
    for (index, block) in listing.split("\n\n").enumerate() {
        let mut worktree = Worktree {
            path: PathBuf::new(),
            branch: None,
            head: String::new(),
            // Git lists the main one first.
            is_main: index == 0,
            is_current: false,
            locked: false,
            missing: false,
        };
        let mut bare = false;
        let mut any = false;
        for line in block.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => {
                    worktree.path = PathBuf::from(value);
                    any = true;
                }
                "HEAD" => worktree.head = value.to_owned(),
                "branch" => {
                    worktree.branch = value.strip_prefix("refs/heads/").map(str::to_owned);
                }
                "bare" => bare = true,
                "locked" => worktree.locked = true,
                "prunable" => worktree.missing = true,
                _ => {}
            }
        }
        if any && !bare {
            found.push(worktree);
        }
    }
    found
}

/// The repository's working trees, main one first, with `workdir` marked as current.
pub fn list(repo: &Path, workdir: &Path) -> Vec<Worktree> {
    let Ok(listing) = ops::run(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let mut found = parse(&listing);
    let here = canonical(workdir);
    for worktree in &mut found {
        worktree.is_current = canonical(&worktree.path) == here;
    }
    found
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn porcelain_output_reads_branches_detached_heads_and_flags() {
        let got = parse(
            "worktree /code/app\nHEAD aaaaaaaaaa\nbranch refs/heads/main\n\n\
             worktree /code/app-fix\nHEAD bbbbbbbbbb\nbranch refs/heads/fix/x\nlocked why\n\n\
             worktree /code/app-old\nHEAD cccccccccc\ndetached\nprunable gitdir file points to non-existent location\n",
        );
        assert_eq!(got.len(), 3);
        assert!(got[0].is_main && !got[1].is_main);
        assert_eq!(got[1].branch.as_deref(), Some("fix/x"));
        assert!(got[1].locked);
        assert!(got[2].missing);
        assert_eq!(got[2].label(), "app-old");
        assert_eq!(got[2].detail(), "detached at ccccccc");
        assert_eq!(got[0].detail(), "main");
    }

    #[test]
    fn a_bare_repository_is_not_listed() {
        assert!(parse("worktree /code/app.git\nbare\n").is_empty());
    }

    #[test]
    fn a_real_repository_lists_its_linked_trees_and_marks_the_current_one() {
        let repo = Repo::new();
        repo.commit("first");
        let linked = repo.path().join("linked");
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/x",
            linked.to_str().unwrap(),
        ]);

        let from_main = list(repo.path(), repo.path());
        assert_eq!(from_main.len(), 2);
        assert!(from_main[0].is_main && from_main[0].is_current);
        assert_eq!(from_main[1].branch.as_deref(), Some("feat/x"));
        assert!(!from_main[1].is_current);

        let from_linked = list(&linked, &linked);
        assert_eq!(from_linked.len(), 2);
        assert!(from_linked[0].is_main && !from_linked[0].is_current);
        assert!(from_linked[1].is_current);
    }
}
