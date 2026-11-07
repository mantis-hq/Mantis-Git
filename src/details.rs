//! What a single commit changed, and the commands that act on one.

use crate::ops;
use std::path::Path;
use std::sync::Arc;

/// One file a commit touched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    /// `A`, `M`, `D`, `R`, `C` or `T`, as git reports it.
    pub status: char,
    pub path: Arc<str>,
    /// Where a renamed or copied file came from.
    pub old_path: Option<Arc<str>>,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

impl FileChange {
    pub fn status_name(&self) -> &'static str {
        match self.status {
            'A' => "Added",
            'D' => "Deleted",
            'R' => "Renamed",
            'C' => "Copied",
            'T' => "Type changed",
            _ => "Modified",
        }
    }

    /// The folder and file name, to draw the folder quieter.
    pub fn split_path(&self) -> (&str, &str) {
        let split = self.path.rfind('/').map_or(0, |slash| slash + 1);
        self.path.split_at(split)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitDetails {
    pub oid: Arc<str>,
    /// The message after its subject line.
    pub body: String,
    pub files: Vec<FileChange>,
}

impl CommitDetails {
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(add, del), file| {
            (add + file.additions, del + file.deletions)
        })
    }
}

pub fn read(repo: &Path, oid: &str) -> Result<CommitDetails, String> {
    let body = ops::run(
        repo,
        &["log", "-1", "--no-show-signature", "--format=%b", oid],
    )?;
    let output = ops::run(
        repo,
        &[
            "diff-tree",
            "--root",
            "-r",
            "-z",
            "-M",
            "--raw",
            "--numstat",
            "--no-commit-id",
            oid,
        ],
    )?;
    Ok(CommitDetails {
        oid: oid.into(),
        body: body.trim().to_owned(),
        files: parse_changes(&output),
    })
}

/// Parses `--raw --numstat -z`: every file's raw record, then every file's numstat record.
fn parse_changes(output: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut numstat_index = 0;
    let mut fields = output.split('\0').filter(|field| !field.is_empty());

    while let Some(field) = fields.next() {
        if let Some(raw) = field.strip_prefix(':') {
            let code = raw.split(' ').nth(4).unwrap_or("M");
            let status = code.chars().next().unwrap_or('M');
            let first = fields.next().unwrap_or_default();
            let (path, old_path) = if matches!(status, 'R' | 'C') {
                (fields.next().unwrap_or_default(), Some(first.into()))
            } else {
                (first, None)
            };
            files.push(FileChange {
                status,
                path: path.into(),
                old_path,
                ..FileChange::default()
            });
            continue;
        }

        let mut stats = field.splitn(3, '\t');
        let (additions, deletions) = (stats.next(), stats.next());
        // A rename's numstat leaves its path empty and gives both paths as fields.
        if stats.next().is_none_or(str::is_empty) {
            fields.next();
            fields.next();
        }
        if let Some(file) = files.get_mut(numstat_index) {
            file.binary = additions == Some("-");
            file.additions = additions.and_then(|n| n.parse().ok()).unwrap_or(0);
            file.deletions = deletions.and_then(|n| n.parse().ok()).unwrap_or(0);
        }
        numstat_index += 1;
    }
    files
}

/// Switches to a branch, or to the bare commit when no branch names it.
pub fn check_out(repo: &Path, branch: Option<&str>, oid: &str) -> Result<Option<String>, String> {
    let short = &oid[..oid.len().min(7)];
    match branch {
        Some(branch) => ops::run(repo, &["switch", branch]).map(|_| None),
        None => ops::run(repo, &["switch", "--detach", oid]).map(|_| {
            Some(format!(
                "Checked out {short} without a branch. Switch to a branch to get back."
            ))
        }),
    }
}

pub fn cherry_pick(repo: &Path, oid: &str) -> Result<Option<String>, String> {
    ops::run(repo, &["cherry-pick", oid])?;
    let short = &oid[..oid.len().min(7)];
    Ok(Some(format!(
        "Copied {short} onto this branch as a new commit. Revert it to undo."
    )))
}

pub fn revert(repo: &Path, oid: &str, is_merge: bool) -> Result<Option<String>, String> {
    let mut args = vec!["revert", "--no-edit"];
    if is_merge {
        args.extend(["-m", "1"]);
    }
    args.push(oid);
    ops::run(repo, &args)?;
    let short = &oid[..oid.len().min(7)];
    Ok(Some(format!(
        "Added a commit that reverts {short}. Revert that one to undo."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    struct Repo(TempDir);

    impl Repo {
        fn new() -> Self {
            let repo = Repo(TempDir::new().unwrap());
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(["-c", "user.name=T", "-c", "user.email=t@e.com"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", self.path().join(".no-global-config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", message]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    fn lines(count: usize, word: &str) -> String {
        (0..count)
            .map(|index| format!("{word} {index}\n"))
            .collect()
    }

    #[test]
    fn a_root_commit_lists_its_files_as_added() {
        let repo = Repo::new();
        repo.write("src/app.rs", "one\ntwo\n");
        let oid = repo.commit("first\n\nThe details.\nAcross lines.");

        let details = read(repo.path(), &oid).unwrap();
        assert_eq!(details.body, "The details.\nAcross lines.");
        assert_eq!(details.files.len(), 1);
        let file = &details.files[0];
        assert_eq!((file.status, file.additions, file.deletions), ('A', 2, 0));
        assert_eq!(file.split_path(), ("src/", "app.rs"));
        assert_eq!(file.status_name(), "Added");
    }

    #[test]
    fn modified_deleted_and_binary_files_are_told_apart() {
        let repo = Repo::new();
        repo.write("keep", &lines(3, "keep"));
        repo.write("gone", "bye\n");
        fs::write(repo.path().join("blob"), [0u8, 1, 2]).unwrap();
        repo.commit("base");

        repo.write("keep", "keep 0\nchanged\nkeep 2\n");
        fs::remove_file(repo.path().join("gone")).unwrap();
        fs::write(repo.path().join("blob"), [3u8, 0, 4]).unwrap();
        let oid = repo.commit("change");

        let details = read(repo.path(), &oid).unwrap();
        let file = |path: &str| details.files.iter().find(|f| &*f.path == path).unwrap();
        assert_eq!((file("keep").status, file("keep").additions), ('M', 1));
        assert_eq!(file("keep").deletions, 1);
        assert_eq!(file("gone").status, 'D');
        assert!(file("blob").binary);
        assert_eq!(details.totals(), (1, 2));
    }

    #[test]
    fn renames_keep_both_paths_and_line_up_with_their_stats() {
        let repo = Repo::new();
        repo.write("before.rs", &lines(20, "same"));
        repo.write("other", "a\n");
        repo.commit("base");

        repo.git(&["mv", "before.rs", "after.rs"]);
        repo.write("after.rs", &format!("{}extra\n", lines(20, "same")));
        repo.write("other", "a\nb\n");
        let oid = repo.commit("rename");

        let details = read(repo.path(), &oid).unwrap();
        let renamed = details.files.iter().find(|f| f.status == 'R').unwrap();
        assert_eq!(&*renamed.path, "after.rs");
        assert_eq!(renamed.old_path.as_deref(), Some("before.rs"));
        assert_eq!(renamed.additions, 1);
        let other = details.files.iter().find(|f| &*f.path == "other").unwrap();
        assert_eq!(other.additions, 1, "stats stay aligned after a rename");
    }

    // Windows doesn't allow tabs in file names.
    #[cfg(unix)]
    #[test]
    fn paths_with_spaces_and_tabs_stay_whole() {
        let repo = Repo::new();
        repo.write("with space/a\tb.txt", "x\n");
        let oid = repo.commit("odd path");
        let details = read(repo.path(), &oid).unwrap();
        assert_eq!(&*details.files[0].path, "with space/a\tb.txt");
        assert_eq!(details.files[0].additions, 1);
    }

    #[test]
    fn a_clean_merge_has_no_changes_of_its_own() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b", "2\n");
        repo.commit("side");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("c", "3\n");
        repo.commit("main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);

        let merge = repo.git(&["rev-parse", "HEAD"]);
        assert!(read(repo.path(), &merge).unwrap().files.is_empty());
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
}
