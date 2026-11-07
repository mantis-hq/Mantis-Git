//! Uncommitted changes: what is staged, what is not, and turning the staged part into a
//! commit.

use crate::details::FileChange;
use crate::ops;
use crate::patch::{self, DiffView, FilePatch, LineKind};
use crate::text::count;
use std::path::Path;

/// New files larger than this are staged whole without showing their text.
const MAX_SHOWN_FILE: u64 = 1 << 20;

/// One file with uncommitted changes on one side of the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingFile {
    pub path: String,
    pub patch: FilePatch,
    /// Git doesn't know the file yet, so its patch is all additions whatever the view.
    pub untracked: bool,
}

impl WorkingFile {
    /// The file as the inspector lists it, the same as for a commit's files.
    pub fn change(&self) -> FileChange {
        let count = |kind| {
            let lines = self.patch.hunks.iter().flat_map(|hunk| &hunk.lines);
            lines.filter(|line| line.kind == kind).count() as u32
        };
        FileChange {
            status: self.patch.status(),
            path: self.path.as_str().into(),
            old_path: None,
            additions: count(LineKind::Added),
            deletions: count(LineKind::Removed),
            binary: self.patch.binary,
        }
    }

    /// Whether single hunks of this file can be moved in and out of the index. A file that
    /// is added or removed whole, or has no text, only moves as a whole.
    pub fn has_movable_hunks(&self) -> bool {
        !self.untracked && !self.patch.binary && self.patch.status() == 'M'
    }
}

/// Which half of the changes a list holds. A file changed both in and out of the index is
/// in both, each with its own patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Changes in the working tree that the index doesn't have.
    Unstaged,
    /// Changes in the index that HEAD doesn't have: what the next commit holds.
    Staged,
}

impl Side {
    pub fn heading(self) -> &'static str {
        match self {
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
        }
    }

    /// What a list says when it has no files.
    pub fn empty(self) -> &'static str {
        match self {
            Self::Unstaged => "Nothing left to stage.",
            Self::Staged => "Nothing staged yet. Tick a file to put it in the next commit.",
        }
    }

    /// What ticking a file or folder here does, naming it.
    pub fn action(self, name: &str) -> String {
        match self {
            Self::Unstaged => format!("Stage {name}"),
            Self::Staged => format!("Unstage {name}"),
        }
    }

    /// The word on a hunk's header for whether it goes in the next commit.
    pub fn hunk_note(self) -> &'static str {
        match self {
            Self::Unstaged => "left out",
            Self::Staged => "in this commit",
        }
    }

    /// Whether ticking moves things out of the index.
    pub fn unstages(self) -> bool {
        self == Self::Staged
    }
}

/// Every changed file, by where its changes are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub unstaged: Vec<WorkingFile>,
    pub staged: Vec<WorkingFile>,
}

impl Changes {
    pub fn side(&self, side: Side) -> &[WorkingFile] {
        match side {
            Side::Unstaged => &self.unstaged,
            Side::Staged => &self.staged,
        }
    }

    /// A file by its place in the two lists end to end, unstaged first.
    pub fn file(&self, index: usize) -> Option<(Side, &WorkingFile)> {
        match self.unstaged.get(index) {
            Some(file) => Some((Side::Unstaged, file)),
            None => self
                .staged
                .get(index.checked_sub(self.unstaged.len())?)
                .map(|file| (Side::Staged, file)),
        }
    }

    /// The two lists end to end.
    pub fn all(&self) -> impl Iterator<Item = &WorkingFile> {
        self.unstaged.iter().chain(&self.staged)
    }
}

/// Reads what is staged and what is not, each against what it differs from.
pub fn read(repo: &Path) -> Result<Changes, String> {
    let read_side = |command: &[&str]| -> Result<Vec<WorkingFile>, String> {
        let diff = ops::run(repo, &patch::diff_args(command))?;
        Ok(patch::parse(&diff)
            .into_iter()
            .map(|patch| WorkingFile {
                path: patch.path.clone(),
                patch,
                untracked: false,
            })
            .collect())
    };
    // Before the first commit `--cached` compares with an empty tree.
    let mut staged = read_side(&["diff", "--cached", "--no-renames"])?;
    let mut unstaged = read_side(&["diff", "--no-renames"])?;

    let untracked = ops::run(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        unstaged.push(WorkingFile {
            path: path.to_owned(),
            patch: read_new_file(repo, path),
            untracked: true,
        });
    }
    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Changes { unstaged, staged })
}

/// One file as it is drawn, in the chosen [`DiffView`]. What [`read`] found is the plain
/// view, so that is used as it is, and so is a file git doesn't know yet.
pub fn file_diff(
    repo: &Path,
    file: &WorkingFile,
    side: Side,
    view: DiffView,
) -> Result<FilePatch, String> {
    if view.is_default() || file.untracked {
        return Ok(file.patch.clone());
    }
    let mut command = vec!["diff", "--no-renames"];
    if side == Side::Staged {
        command.push("--cached");
    }
    let mut args = patch::diff_args(&command);
    args.extend(view.args());
    args.extend(["--", &file.path]);
    let diff = ops::run(repo, &args)?;
    // Nothing but white space changed, so git has no diff to print.
    let mut patch = patch::parse(&diff).into_iter().next().unwrap_or_default();
    patch.path.clone_from(&file.path);
    Ok(patch)
}

/// Puts these files, all of their changes, into the index.
pub fn stage_files(repo: &Path, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["--literal-pathspecs", "add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    ops::run(repo, &args).map(drop)
}

/// Takes these files out of the index, leaving the files themselves as they are.
pub fn unstage_files(repo: &Path, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["--literal-pathspecs"];
    if ops::resolves(repo, "HEAD") {
        args.extend(["reset", "-q", "--"]);
    } else {
        args.extend(["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"]);
    }
    args.extend(paths.iter().map(String::as_str));
    ops::run(repo, &args).map(drop)
}

/// Moves one hunk of a file into the index, or out of it. The hunk is by position in the
/// plain patch [`read`] gave, so read again after each move.
pub fn move_hunk(repo: &Path, file: &WorkingFile, hunk: usize, side: Side) -> Result<(), String> {
    if hunk >= file.patch.hunks.len() {
        return Err("That change is gone; the list has been refreshed.".to_owned());
    }
    let patch = file.patch.with_hunks([hunk]);
    let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
    if side.unstages() {
        args.push("--reverse");
    }
    ops::run_with_input(repo, &args, patch.as_bytes()).map(drop)
}

/// Commits what is staged, through git's own commit so hooks and signing run.
pub fn commit(repo: &Path, message: &str) -> Result<String, String> {
    ops::run_with_input(repo, &["commit", "-q", "-F", "-"], message.as_bytes())?;
    ops::run(repo, &["rev-parse", "HEAD"]).map(|oid| oid.trim().to_owned())
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

/// The kinds of change the commit form offers, in its order.
pub const COMMIT_TYPES: [&str; 7] = ["feat", "fix", "docs", "refactor", "test", "chore", "ci"];

/// Whether a commit has what it needs to be written: something staged, and a subject.
pub fn can_commit(staged: usize, subject: &str) -> bool {
    staged > 0 && !subject.trim().is_empty()
}

/// What the commit button says it will do.
pub fn commit_label(staged: usize) -> String {
    match staged {
        0 => "Stage a file to commit".to_owned(),
        files => format!("Commit {}", count(files, "file")),
    }
}

/// What to tell the user once a commit landed, with how to take it back.
pub fn committed_notice(line: &str) -> String {
    format!(
        "Committed “{line}”. `git reset --soft HEAD~1` undoes it and keeps your changes staged."
    )
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

    fn file<'a>(files: &'a [WorkingFile], path: &str) -> &'a WorkingFile {
        files.iter().find(|file| file.path == path).unwrap()
    }

    fn names(files: &[WorkingFile]) -> Vec<&str> {
        files.iter().map(|file| file.path.as_str()).collect()
    }

    /// Twenty lines, so a change at each end is two separate hunks.
    fn long(top: &str, bottom: &str) -> String {
        let middle: String = (0..18).map(|n| format!("line {n}\n")).collect();
        format!("{top}\n{middle}{bottom}\n")
    }

    #[test]
    fn working_files_list_like_a_commits_files() {
        let repo = Repo::new();
        repo.write("kept.txt", "a\nb\n");
        repo.commit("first");
        repo.write("kept.txt", "a\nc\nd\n");
        repo.write("new.txt", "x\n");
        let changes = read(repo.path()).unwrap();
        let summary: Vec<_> = changes
            .unstaged
            .iter()
            .map(WorkingFile::change)
            .map(|c| (c.status, c.path.to_string(), c.additions, c.deletions))
            .collect();
        assert_eq!(
            summary,
            [
                ('M', "kept.txt".to_owned(), 2, 1),
                ('A', "new.txt".to_owned(), 1, 0)
            ]
        );
        assert!(changes.staged.is_empty());
    }

    #[test]
    fn staged_and_unstaged_changes_are_read_apart() {
        let repo = Repo::new();
        repo.write("staged", "a\n");
        repo.write("unstaged", "a\n");
        repo.write("both", "a\n");
        repo.commit("base");
        repo.write("staged", "b\n");
        repo.git(&["add", "staged"]);
        repo.write("unstaged", "b\n");
        repo.write("both", "b\n");
        repo.git(&["add", "both"]);
        repo.write("both", "c\n");
        repo.write("dir/new file", "hello\n");

        let changes = read(repo.path()).unwrap();
        assert_eq!(names(&changes.staged), ["both", "staged"]);
        assert_eq!(
            names(&changes.unstaged),
            ["both", "dir/new file", "unstaged"]
        );
        assert!(file(&changes.unstaged, "dir/new file").untracked);
        // The same file has a different patch on each side.
        assert_eq!(changes.staged[0].change().additions, 1);
        assert_eq!(changes.unstaged[0].change().deletions, 1);
        assert_eq!(changes.file(0).map(|(side, _)| side), Some(Side::Unstaged));
        assert_eq!(changes.file(3).map(|(side, _)| side), Some(Side::Staged));
        assert!(changes.file(5).is_none());
    }

    #[test]
    fn ignored_files_stay_out() {
        let repo = Repo::new();
        repo.write(".gitignore", "secret\n");
        repo.commit("base");
        repo.write("secret", "x\n");
        assert_eq!(read(repo.path()).unwrap(), Changes::default());
    }

    #[test]
    fn staging_and_unstaging_move_files_in_and_out_of_the_index() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.write("b", "1\n");
        repo.commit("base");
        repo.write("a", "2\n");
        repo.write("b", "2\n");
        repo.write("new file", "x\n");

        let both: Vec<String> = ["a", "new file"].map(String::from).into();
        stage_files(repo.path(), &both).unwrap();
        let changes = read(repo.path()).unwrap();
        assert_eq!(names(&changes.staged), ["a", "new file"]);
        assert_eq!(names(&changes.unstaged), ["b"]);

        unstage_files(repo.path(), &both).unwrap();
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
        assert_eq!(repo.read("a"), "2\n", "the file itself is untouched");
    }

    #[test]
    fn staging_a_deletion_and_unstaging_it() {
        let repo = Repo::new();
        repo.write("gone", "bye\n");
        repo.commit("base");
        fs::remove_file(repo.path().join("gone")).unwrap();
        stage_files(repo.path(), &["gone".to_owned()]).unwrap();
        assert_eq!(read(repo.path()).unwrap().staged[0].patch.status(), 'D');
        unstage_files(repo.path(), &["gone".to_owned()]).unwrap();
        assert_eq!(read(repo.path()).unwrap().unstaged[0].patch.status(), 'D');
    }

    #[test]
    fn unstaging_works_before_the_first_commit() {
        let repo = Repo::new();
        repo.write("first", "hello\n");
        stage_files(repo.path(), &["first".to_owned()]).unwrap();
        assert_eq!(repo.git(&["ls-files"]), "first");
        assert_eq!(names(&read(repo.path()).unwrap().staged), ["first"]);
        unstage_files(repo.path(), &["first".to_owned()]).unwrap();
        assert_eq!(repo.git(&["ls-files"]), "");
    }

    // Windows doesn't allow `*` in file names.
    #[cfg(unix)]
    #[test]
    fn paths_with_glob_characters_are_taken_literally() {
        let repo = Repo::new();
        repo.write("a*", "new\n");
        repo.write("abc", "other\n");
        stage_files(repo.path(), &["a*".to_owned()]).unwrap();
        assert_eq!(repo.git(&["ls-files"]), "a*");
    }

    #[test]
    fn one_hunk_moves_in_and_out_of_the_index_alone() {
        let repo = Repo::new();
        repo.write("file", &long("top", "bottom"));
        repo.commit("base");
        repo.write("file", &long("new top", "new bottom"));

        let changes = read(repo.path()).unwrap();
        let both = &changes.unstaged[0];
        assert_eq!(both.patch.hunks.len(), 2);
        assert!(both.has_movable_hunks());
        move_hunk(repo.path(), both, 1, Side::Unstaged).unwrap();

        let changes = read(repo.path()).unwrap();
        assert_eq!(changes.staged[0].patch.hunks.len(), 1);
        assert_eq!(changes.unstaged[0].patch.hunks.len(), 1);
        assert_eq!(
            repo.git(&["show", ":file"]),
            long("top", "new bottom").trim()
        );
        assert_eq!(repo.read("file"), long("new top", "new bottom"));

        move_hunk(repo.path(), &changes.staged[0], 0, Side::Staged).unwrap();
        let changes = read(repo.path()).unwrap();
        assert!(changes.staged.is_empty());
        assert_eq!(changes.unstaged[0].patch.hunks.len(), 2);
    }

    #[test]
    fn whole_files_have_no_hunks_to_move() {
        let repo = Repo::new();
        repo.write("gone", "bye\n");
        repo.commit("base");
        fs::remove_file(repo.path().join("gone")).unwrap();
        repo.write("new", "hello\n");
        let changes = read(repo.path()).unwrap();
        assert!(!file(&changes.unstaged, "gone").has_movable_hunks());
        assert!(!file(&changes.unstaged, "new").has_movable_hunks());
    }

    #[test]
    fn a_commit_holds_exactly_what_is_staged() {
        let repo = Repo::new();
        repo.write("in", "a\n");
        repo.write("out", "a\n");
        repo.commit("base");
        repo.write("in", "b\n");
        repo.write("out", "b\n");
        stage_files(repo.path(), &["in".to_owned()]).unwrap();

        commit(repo.path(), "just one\n").unwrap();
        assert_eq!(repo.git(&["show", "HEAD:in"]), "b");
        assert_eq!(repo.git(&["show", "HEAD:out"]), "a");
        assert_eq!(repo.read("out"), "b\n");
        assert_eq!(names(&read(repo.path()).unwrap().unstaged), ["out"]);
    }

    #[test]
    fn a_first_commit_works_with_no_head() {
        let repo = Repo::new();
        repo.write("first", "hello\n");
        stage_files(repo.path(), &["first".to_owned()]).unwrap();
        commit(repo.path(), "first\n").unwrap();
        assert_eq!(repo.git(&["log", "--format=%s"]), "first");
    }

    #[test]
    fn a_failing_hook_keeps_what_was_staged() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.write("a", "2\n");
        stage_files(repo.path(), &["a".to_owned()]).unwrap();

        let hook = repo.path().join(".git/hooks/pre-commit");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let error = commit(repo.path(), "nope\n").unwrap_err();
        assert!(error.contains("lint failed"), "{error}");
        assert_eq!(repo.git(&["log", "--format=%s"]), "base");
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "a");
    }

    #[test]
    fn a_working_diff_can_show_the_whole_file_and_skip_white_space() {
        let repo = Repo::new();
        repo.write("file", &long("top", "bottom"));
        repo.commit("base");
        repo.write("file", &long("top  ", "new bottom"));
        let changes = read(repo.path()).unwrap();
        let file = &changes.unstaged[0];
        let lines = |patch: &FilePatch| patch.hunks.iter().map(|h| h.lines.len()).sum::<usize>();

        let plain = file_diff(repo.path(), file, Side::Unstaged, DiffView::default()).unwrap();
        assert_eq!(plain.hunks.len(), 2);

        let whole = DiffView {
            whole_file: true,
            ..DiffView::default()
        };
        let whole = file_diff(repo.path(), file, Side::Unstaged, whole).unwrap();
        assert_eq!(whole.hunks.len(), 1);
        assert_eq!(
            lines(&whole),
            22,
            "every line of the file, plus the one removed"
        );

        let quiet = DiffView {
            ignore_whitespace: true,
            ..DiffView::default()
        };
        let quiet = file_diff(repo.path(), file, Side::Unstaged, quiet).unwrap();
        assert_eq!(quiet.hunks.len(), 1, "the trailing spaces no longer count");
    }

    #[test]
    fn the_staged_side_is_drawn_from_the_index() {
        let repo = Repo::new();
        repo.write("file", "a b\n");
        repo.commit("base");
        repo.write("file", "a  b\n");
        stage_files(repo.path(), &["file".to_owned()]).unwrap();
        let changes = read(repo.path()).unwrap();
        let quiet = DiffView {
            ignore_whitespace: true,
            ..DiffView::default()
        };
        let patch = file_diff(repo.path(), &changes.staged[0], Side::Staged, quiet).unwrap();
        assert!(patch.hunks.is_empty());
        assert_eq!(patch.path, "file");
    }

    #[test]
    fn words_name_the_side_and_the_result() {
        assert_eq!(Side::Unstaged.action("a.rs"), "Stage a.rs");
        assert_eq!(Side::Staged.action("a.rs"), "Unstage a.rs");
        assert_eq!(Side::Staged.hunk_note(), "in this commit");
        assert_eq!(commit_label(0), "Stage a file to commit");
        assert_eq!(commit_label(2), "Commit 2 files");
        assert!(can_commit(1, "add it"));
        assert!(!can_commit(1, "  "));
        assert!(!can_commit(0, "add it"));
        assert!(committed_notice("fix: x").contains("git reset --soft HEAD~1"));
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
