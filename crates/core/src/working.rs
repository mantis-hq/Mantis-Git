//! Uncommitted changes, and turning a chosen part of them into a commit.

use crate::ops;
use crate::patch::{self, FilePatch};
use crate::text::count;
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

/// Which hunks are ticked for the next commit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// By file, then by hunk. A file without hunks has one entry.
    ticks: Vec<Vec<bool>>,
}

impl Selection {
    /// The starting ticks. If anything was staged, that choice carries over by file.
    pub fn starting(files: &[WorkingFile]) -> Self {
        let any_staged = files.iter().any(|file| file.staged);
        let ticks = files
            .iter()
            .map(|file| vec![!any_staged || file.staged; file.patch.hunks.len().max(1)])
            .collect();
        Self { ticks }
    }

    /// How many ticks a file has.
    pub fn hunks(&self, file: usize) -> usize {
        self.ticks[file].len()
    }

    pub fn is_on(&self, file: usize, hunk: usize) -> bool {
        self.ticks[file][hunk]
    }

    /// Whether a file is ticked all the way, and whether it's ticked only partly.
    pub fn file_state(&self, file: usize) -> (bool, bool) {
        let hunks = &self.ticks[file];
        let on = hunks.iter().filter(|on| **on).count();
        (on == hunks.len(), on > 0 && on < hunks.len())
    }

    /// Ticking a file turns all of its hunks on, unless all were on already.
    pub fn toggle_file(&mut self, file: usize) {
        let hunks = &mut self.ticks[file];
        let all_on = hunks.iter().all(|on| *on);
        hunks.fill(!all_on);
    }

    pub fn toggle_hunk(&mut self, file: usize, hunk: usize) {
        let tick = &mut self.ticks[file][hunk];
        *tick = !*tick;
    }

    /// Turns the ticks into what [`commit`] wants.
    pub fn included(&self, files: &[WorkingFile]) -> Vec<Included> {
        files
            .iter()
            .zip(&self.ticks)
            .filter(|(_, hunks)| hunks.iter().any(|on| *on))
            .map(|(file, hunks)| Included {
                file: file.clone(),
                hunks: (0..file.patch.hunks.len())
                    .filter(|&hunk| hunks[hunk])
                    .collect(),
            })
            .collect()
    }

    /// Says how the commit stands, in plain words.
    pub fn sentence(&self) -> String {
        let going_in = self
            .ticks
            .iter()
            .filter(|hunks| hunks.iter().any(|on| *on))
            .count();
        let left_out: usize = self
            .ticks
            .iter()
            .map(|hunks| hunks.iter().filter(|on| !**on).count())
            .sum();
        let files = count(self.ticks.len(), "file");
        if going_in == 0 {
            format!("{files} changed, nothing ticked yet")
        } else {
            let left_out = count(left_out, "change");
            format!("{going_in} of {files} going in, {left_out} left out")
        }
    }
}

/// Reads every changed file against HEAD, whether staged, unstaged or untracked.
pub fn read(repo: &Path) -> Result<Vec<WorkingFile>, String> {
    // Before the first commit there's no HEAD to compare with, only an empty tree.
    let base = if ops::resolves(repo, "HEAD") {
        "HEAD".to_owned()
    } else {
        ops::run_with_input(repo, &["mktree"], b"")?
            .trim()
            .to_owned()
    };
    let diff = ops::run(repo, &patch::diff_args(&["diff", &base, "--no-renames"]))?;
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
    let output = ops::run_unchecked(
        ops::git(repo)
            .args(patch::diff_args(&["diff", "--no-index"]))
            .args(["--", "/dev/null", path]),
        None,
    );
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
    if ops::resolves(repo, "HEAD") {
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

/// Whether a commit has what it needs to be written.
pub fn can_commit(included: &[Included], subject: &str) -> bool {
    !included.is_empty() && !subject.trim().is_empty()
}

/// Whether a kind of change can be marked as breaking.
pub fn can_break(kind: &str) -> bool {
    kind == "feat"
}

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
    let mark = if breaking && can_break(kind) { "!" } else { "" };
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
    use crate::testing::Repo;
    use std::fs;

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
        repo.commit("base");
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
        repo.commit("base");
        repo.write("secret", "x\n");
        assert_eq!(read(repo.path()).unwrap(), []);
    }

    #[test]
    fn one_hunk_of_two_commits_and_the_other_stays_in_the_working_tree() {
        let repo = Repo::new();
        repo.write("file", &long("top", "bottom"));
        repo.commit("base");
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
        repo.commit("base");

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
        repo.commit("base");
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
        repo.commit("base");
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

    const TWO_HUNKS: &str = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
@@ -9 +9 @@
-c
+d
";

    fn changed(path: &str, staged: bool) -> WorkingFile {
        let mut patch = patch::parse(TWO_HUNKS).remove(0);
        patch.path = path.to_owned();
        WorkingFile {
            path: path.to_owned(),
            patch,
            staged,
        }
    }

    #[test]
    fn everything_starts_ticked_when_nothing_is_staged() {
        let selection = Selection::starting(&[changed("a", false), changed("b", false)]);
        assert_eq!(selection.file_state(0), (true, false));
        assert_eq!(selection.file_state(1), (true, false));
    }

    #[test]
    fn staged_files_carry_over_as_the_starting_choice() {
        let selection = Selection::starting(&[changed("a", true), changed("b", false)]);
        assert_eq!(selection.file_state(0), (true, false));
        assert_eq!(selection.file_state(1), (false, false));
    }

    #[test]
    fn a_file_with_no_hunks_still_gets_one_tick() {
        let mut binary = changed("a", false);
        binary.patch.hunks.clear();
        let selection = Selection::starting(&[binary]);
        assert_eq!(selection.hunks(0), 1);
        assert!(selection.is_on(0, 0));
    }

    #[test]
    fn ticking_a_partly_ticked_file_turns_it_all_on() {
        let mut selection = Selection::starting(&[changed("a", false)]);
        selection.toggle_hunk(0, 1);
        assert_eq!(selection.file_state(0), (false, true));
        selection.toggle_file(0);
        assert_eq!(selection.file_state(0), (true, false));
        selection.toggle_file(0);
        assert_eq!(selection.file_state(0), (false, false));
    }

    #[test]
    fn only_ticked_hunks_are_chosen() {
        let files = [changed("a", false), changed("b", false)];
        let mut selection = Selection::starting(&files);
        selection.toggle_hunk(0, 0);
        selection.toggle_file(1);
        let parts = selection.included(&files);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].hunks, [1]);
    }

    #[test]
    fn the_sentence_counts_what_goes_in_and_what_stays_out() {
        let files = [changed("a", false), changed("b", false)];
        let mut selection = Selection::starting(&files);
        selection.toggle_file(1);
        selection.toggle_hunk(0, 0);
        assert_eq!(
            selection.sentence(),
            "1 of 2 files going in, 3 changes left out"
        );
        selection.toggle_hunk(0, 1);
        assert_eq!(selection.sentence(), "2 files changed, nothing ticked yet");
    }

    #[test]
    fn a_commit_needs_something_ticked_and_a_subject() {
        let files = [changed("a", false)];
        let included = Selection::starting(&files).included(&files);
        assert!(can_commit(&included, "add it"));
        assert!(!can_commit(&included, "  "));
        assert!(!can_commit(&[], "add it"));
    }
}
