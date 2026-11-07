//! Uncommitted changes, and turning a chosen part of them into a commit.

use crate::ops;
use crate::patch::{self, FilePatch};
use std::collections::HashSet;
use std::path::Path;

/// New files larger than this are committed whole without showing their text.
const MAX_SHOWN_FILE: u64 = 1 << 20;

/// One file with uncommitted changes, staged or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingFile {
    pub path: String,
    pub patch: FilePatch,
    /// Whether any of the file was already staged.
    pub staged: bool,
}

/// Which hunks of a file go into a commit, by index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Included {
    pub file: WorkingFile,
    pub hunks: Vec<usize>,
}

impl Included {
    /// Whether all of the file goes in, which also covers what has no hunks to choose:
    /// binary files and mode changes.
    fn is_whole(&self) -> bool {
        self.hunks.len() == self.file.patch.hunks.len()
    }
}

/// Reads every changed file against HEAD, whether staged, unstaged or untracked.
pub fn read(repo: &Path) -> Result<Vec<WorkingFile>, String> {
    let head = ops::run(repo, &["rev-parse", "-q", "--verify", "HEAD"]).map(|_| "HEAD".to_owned());
    // Before the first commit there's no HEAD to compare with, only an empty tree.
    let base = match head {
        Ok(head) => head,
        Err(_) => ops::run_with_input(repo, &["mktree"], b"")?
            .trim()
            .to_owned(),
    };
    let diff = ops::run(
        repo,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            &base,
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ],
    )?;
    let names = ops::run(repo, &["diff", "--cached", "--name-only", "-z", &base])?;
    let staged: HashSet<&str> = names.split('\0').collect();

    let mut files: Vec<WorkingFile> = patch::parse(&diff)
        .into_iter()
        .map(|patch| WorkingFile {
            staged: staged.contains(patch.path.as_str()),
            path: patch.path.clone(),
            patch,
        })
        .collect();

    let untracked = ops::run(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        files.push(WorkingFile {
            path: path.to_owned(),
            patch: read_new_file(repo, path),
            staged: false,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Shows an untracked file as all additions. `git diff --no-index` exits non-zero when it
/// finds a difference, which here it always does, so its output is taken either way.
fn read_new_file(repo: &Path, path: &str) -> FilePatch {
    let hidden = FilePatch::without_text(path);
    let size = std::fs::metadata(repo.join(path)).map_or(u64::MAX, |file| file.len());
    if size > MAX_SHOWN_FILE {
        return hidden;
    }
    let output = crate::git::git(repo)
        .args([
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-index",
            "--no-color",
        ])
        .args(["--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/"])
        .args(["--", "/dev/null", path])
        .output();
    let Ok(output) = output else {
        return hidden;
    };
    let diff = String::from_utf8_lossy(&output.stdout);
    let mut patch = patch::parse(&diff).into_iter().next().unwrap_or(hidden);
    patch.path = path.to_owned();
    patch
}

/// Commits exactly the chosen hunks, through git's own commit so hooks and signing run.
/// If that fails, what was staged before is put back.
pub fn commit(repo: &Path, included: &[Included], message: &str) -> Result<String, String> {
    let saved = ops::run(repo, &["write-tree"])
        .map_err(|error| format!("Couldn't save what's staged: {error}"))?;
    let saved = saved.trim();

    let result = stage(repo, included)
        .and_then(|()| ops::run_with_input(repo, &["commit", "-q", "-F", "-"], message.as_bytes()));
    if result.is_err() {
        let _ = ops::run(repo, &["read-tree", saved]);
    }
    result?;
    ops::run(repo, &["rev-parse", "HEAD"]).map(|oid| oid.trim().to_owned())
}

/// Makes the index exactly HEAD plus the chosen changes.
fn stage(repo: &Path, included: &[Included]) -> Result<(), String> {
    let has_head = ops::run(repo, &["rev-parse", "-q", "--verify", "HEAD"]).is_ok();
    if has_head {
        ops::run(repo, &["reset", "-q"])?;
    } else {
        ops::run(repo, &["read-tree", "--empty"])?;
    }
    for part in included {
        if part.is_whole() {
            let args = ["--literal-pathspecs", "add", "-A", "--", &part.file.path];
            ops::run(repo, &args)?;
        } else {
            let patch = part.file.patch.with_hunks(part.hunks.iter().copied());
            let args = ["apply", "--cached", "--whitespace=nowarn"];
            ops::run_with_input(repo, &args, patch.as_bytes())?;
        }
    }
    Ok(())
}

/// The kinds of change the commit form offers, in its order.
pub const COMMIT_TYPES: [&str; 7] = ["feat", "fix", "docs", "refactor", "test", "chore", "ci"];

/// Builds a Conventional Commit message.
pub fn message(kind: &str, breaking: bool, subject: &str, details: &str) -> String {
    let mut message = format!("{}\n", subject_line(kind, breaking, subject));
    let details = details.trim();
    if !details.is_empty() {
        message.push('\n');
        message.push_str(details);
        message.push('\n');
    }
    message
}

pub fn subject_line(kind: &str, breaking: bool, subject: &str) -> String {
    let mark = if breaking && kind == "feat" { "!" } else { "" };
    format!("{kind}{mark}: {}", subject.trim())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The highest `v1.2.3` style tag.
pub fn latest_version<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<Version> {
    tags.into_iter().filter_map(parse_version).max()
}

fn parse_version(tag: &str) -> Option<Version> {
    let mut parts = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = Version(next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// What landing this commit would do to the version, following the README's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bump {
    None,
    Patch(Version),
    Minor(Version),
    Major(Version),
    /// Breaking, but versions before 1.0 only take a minor bump for it.
    BreakingMinor(Version),
}

pub fn bump(kind: &str, breaking: bool, current: Version) -> Bump {
    let Version(major, minor, patch) = current;
    match (kind, breaking) {
        ("feat", true) if major == 0 => Bump::BreakingMinor(Version(0, minor + 1, 0)),
        ("feat", true) => Bump::Major(Version(major + 1, 0, 0)),
        ("feat", false) => Bump::Minor(Version(major, minor + 1, 0)),
        ("fix", _) => Bump::Patch(Version(major, minor, patch + 1)),
        _ => Bump::None,
    }
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
            repo.git(&["config", "user.name", "T"]);
            repo.git(&["config", "user.email", "t@e.com"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo.git(&["config", "core.autocrlf", "false"]);
            repo
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(args)
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

        fn read(&self, path: &str) -> String {
            fs::read_to_string(self.path().join(path)).unwrap()
        }

        fn commit_all(&self, subject: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", subject]);
        }
    }

    /// Twenty lines, so a change at each end is two separate hunks.
    fn long(top: &str, bottom: &str) -> String {
        let middle: String = (0..18).map(|n| format!("line {n}\n")).collect();
        format!("{top}\n{middle}{bottom}\n")
    }

    fn all(file: &WorkingFile) -> Included {
        Included {
            file: file.clone(),
            hunks: (0..file.patch.hunks.len()).collect(),
        }
    }

    fn file<'a>(files: &'a [WorkingFile], path: &str) -> &'a WorkingFile {
        files.iter().find(|file| file.path == path).unwrap()
    }

    #[test]
    fn staged_unstaged_and_untracked_changes_are_all_read() {
        let repo = Repo::new();
        repo.write("staged", "a\n");
        repo.write("unstaged", "a\n");
        repo.commit_all("base");
        repo.write("staged", "b\n");
        repo.git(&["add", "staged"]);
        repo.write("unstaged", "b\n");
        repo.write("dir/new file", "hello\n");

        let files = read(repo.path()).unwrap();
        assert_eq!(files.len(), 3);
        assert!(file(&files, "staged").staged);
        assert!(!file(&files, "unstaged").staged);
        let new = file(&files, "dir/new file");
        assert_eq!(new.patch.hunks[0].lines.len(), 1, "shown as additions");
    }

    #[test]
    fn ignored_files_stay_out() {
        let repo = Repo::new();
        repo.write(".gitignore", "secret\n");
        repo.commit_all("base");
        repo.write("secret", "x\n");
        assert_eq!(read(repo.path()).unwrap(), []);
    }

    #[test]
    fn one_hunk_of_two_commits_and_the_other_stays_in_the_working_tree() {
        let repo = Repo::new();
        repo.write("file", &long("top", "bottom"));
        repo.commit_all("base");
        repo.write("file", &long("new top", "new bottom"));

        let files = read(repo.path()).unwrap();
        assert_eq!(files[0].patch.hunks.len(), 2);
        let part = Included {
            file: files[0].clone(),
            hunks: vec![1],
        };
        commit(repo.path(), &[part], "bottom only\n").unwrap();

        assert_eq!(
            repo.git(&["show", "HEAD:file"]),
            long("top", "new bottom").trim()
        );
        assert_eq!(repo.read("file"), long("new top", "new bottom"));
        let left = read(repo.path()).unwrap();
        assert_eq!(
            left[0].patch.hunks.len(),
            1,
            "the other hunk is still changed"
        );
    }

    #[test]
    fn a_new_file_a_deletion_and_a_binary_file_commit_whole() {
        let repo = Repo::new();
        repo.write("gone", "bye\n");
        fs::write(repo.path().join("blob"), [0u8, 1]).unwrap();
        repo.commit_all("base");

        fs::remove_file(repo.path().join("gone")).unwrap();
        fs::write(repo.path().join("blob"), [0u8, 2]).unwrap();
        repo.write("new", "hello\n");

        let files = read(repo.path()).unwrap();
        assert!(file(&files, "blob").patch.binary);
        let parts: Vec<_> = files.iter().map(all).collect();
        commit(repo.path(), &parts, "all three\n").unwrap();

        assert_eq!(read(repo.path()).unwrap(), []);
        assert_eq!(repo.git(&["show", "HEAD:new"]), "hello");
        assert!(repo.git(&["ls-files"]).lines().all(|name| name != "gone"));
    }

    #[test]
    fn files_left_out_stay_changed_and_unstaged() {
        let repo = Repo::new();
        repo.write("in", "a\n");
        repo.write("out", "a\n");
        repo.commit_all("base");
        repo.write("in", "b\n");
        repo.write("out", "b\n");
        repo.git(&["add", "out"]);

        let files = read(repo.path()).unwrap();
        commit(repo.path(), &[all(file(&files, "in"))], "just one\n").unwrap();

        assert_eq!(repo.git(&["show", "HEAD:out"]), "a");
        assert_eq!(repo.read("out"), "b\n");
    }

    // Windows doesn't allow `*` in file names.
    #[cfg(unix)]
    #[test]
    fn paths_with_glob_characters_are_taken_literally() {
        let repo = Repo::new();
        repo.write("a*", "new\n");
        repo.write("abc", "other\n");
        let files = read(repo.path()).unwrap();
        commit(repo.path(), &[all(file(&files, "a*"))], "one\n").unwrap();
        assert_eq!(repo.git(&["ls-files"]), "a*");
    }

    #[test]
    fn a_first_commit_works_with_no_head() {
        let repo = Repo::new();
        repo.write("first", "hello\n");
        let files = read(repo.path()).unwrap();
        assert_eq!(files.len(), 1);
        commit(repo.path(), &[all(&files[0])], "first\n").unwrap();
        assert_eq!(repo.git(&["log", "--format=%s"]), "first");
    }

    #[test]
    fn a_failing_hook_keeps_what_was_staged() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.write("b", "1\n");
        repo.commit_all("base");
        repo.write("a", "2\n");
        repo.write("b", "2\n");
        repo.git(&["add", "b"]);

        let hook = repo.path().join(".git/hooks/pre-commit");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let files = read(repo.path()).unwrap();
        let error = commit(repo.path(), &[all(file(&files, "a"))], "nope\n").unwrap_err();
        assert!(error.contains("lint failed"), "{error}");
        assert_eq!(repo.git(&["log", "--format=%s"]), "base");
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "b");
    }

    #[test]
    fn messages_follow_conventional_commits() {
        assert_eq!(message("fix", false, " a bug ", ""), "fix: a bug\n");
        assert_eq!(message("feat", true, "x", ""), "feat!: x\n");
        assert_eq!(message("docs", true, "x", ""), "docs: x\n");
        assert_eq!(message("feat", false, "x", " why\n"), "feat: x\n\nwhy\n");
    }

    #[test]
    fn versions_are_read_from_tags_and_junk_is_skipped() {
        let tags = [
            "v0.1.0",
            "nightly",
            "v0.10.0",
            "v0.9.0",
            "1.2",
            "v1.0.0-rc1",
        ];
        assert_eq!(latest_version(tags), Some(Version(0, 10, 0)));
        assert_eq!(latest_version(["nope"]), None);
    }

    #[test]
    fn bumps_follow_the_readme_table() {
        let early = Version(0, 1, 0);
        assert_eq!(bump("fix", false, early), Bump::Patch(Version(0, 1, 1)));
        assert_eq!(bump("feat", false, early), Bump::Minor(Version(0, 2, 0)));
        assert_eq!(
            bump("feat", true, early),
            Bump::BreakingMinor(Version(0, 2, 0))
        );
        assert_eq!(bump("docs", false, early), Bump::None);

        let stable = Version(1, 2, 3);
        assert_eq!(bump("feat", true, stable), Bump::Major(Version(2, 0, 0)));
        assert_eq!(bump("feat", false, stable), Bump::Minor(Version(1, 3, 0)));
    }
}
