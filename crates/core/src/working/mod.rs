//! Uncommitted changes: what is staged, what is not, and turning the staged part into a
//! commit.

pub mod message;
pub mod version;

use crate::commit::details::{self, FileChange};
use crate::ops::{self, Cancel};
use crate::patch::{self, DiffView, FilePatch};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

/// New files larger than this are staged whole without showing their text.
const MAX_SHOWN_FILE: u64 = 1 << 20;

/// One file with uncommitted changes on one side of the index. Only what the list needs
/// is read up front; the text of its changes is read with [`file_diff`] when it is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingFile {
    pub path: String,
    /// How the file is listed: its status and the number of lines added and removed.
    pub change: FileChange,
    /// Git doesn't know the file yet, so its patch is all additions whatever the view.
    pub untracked: bool,
}

impl WorkingFile {
    /// The file as the inspector lists it, the same as for a commit's files.
    pub fn change(&self) -> FileChange {
        self.change.clone()
    }

    /// Whether single hunks of this file can be moved in and out of the index. A file that
    /// is added or removed whole, or has no text, only moves as a whole.
    pub fn has_movable_hunks(&self) -> bool {
        !self.untracked && !self.change.binary && self.change.status == 'M'
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

/// Reads which files are staged and which are not, with their line counts but not their
/// text. The three git commands run side by side, and an untracked file is counted here
/// rather than by running git on each of what may be thousands.
///
/// A set `cancel` stops it with [`ops::CANCELLED`].
pub fn read(repo: &Path, cancel: &Cancel) -> Result<Changes, String> {
    let list = |cached: bool| -> Result<Vec<WorkingFile>, String> {
        let mut command = vec!["diff", "--no-renames", "--raw", "--numstat", "-z"];
        if cached {
            // Before the first commit `--cached` compares with an empty tree.
            command.push("--cached");
        }
        let output = ops::run_cancellable(repo, &command, cancel)?;
        Ok(details::parse_changes(&output)
            .into_iter()
            .map(|change| WorkingFile {
                path: change.path.to_string(),
                change,
                untracked: false,
            })
            .collect())
    };
    let (staged, unstaged, untracked) = std::thread::scope(|scope| {
        let staged = scope.spawn(|| list(true));
        let unstaged = scope.spawn(|| list(false));
        let untracked = ops::run_cancellable(
            repo,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            cancel,
        );
        let gone = || Err("git stopped".to_owned());
        (
            staged.join().unwrap_or_else(|_| gone()),
            unstaged.join().unwrap_or_else(|_| gone()),
            untracked,
        )
    });
    let (mut staged, mut unstaged) = (staged?, unstaged?);
    let untracked = untracked?;

    let paths: Vec<&str> = untracked
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    unstaged.extend(untracked_files(repo, &paths, cancel)?);
    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Changes { unstaged, staged })
}

/// How long after a file was last written what was read of it can be kept: one written
/// again in the same clock tick, at the same size, would look unchanged.
const SETTLED: Duration = Duration::from_secs(2);

/// An untracked file's listing with the time and size of the file it was read from.
type Remembered = (SystemTime, u64, FileChange);

/// Untracked files as listed at the last read, per repository, so that a refresh only
/// reads the ones that changed.
static REMEMBERED: Mutex<Option<HashMap<PathBuf, HashMap<String, Remembered>>>> = Mutex::new(None);

/// Lists untracked files as added, with their lines counted on several threads.
fn untracked_files(
    repo: &Path,
    paths: &[&str],
    cancel: &Cancel,
) -> Result<Vec<WorkingFile>, String> {
    let mut before = REMEMBERED
        .lock()
        .ok()
        .and_then(|mut all| all.as_mut()?.remove(repo))
        .unwrap_or_default();
    let known: Vec<Option<Remembered>> = paths.iter().map(|path| before.remove(*path)).collect();
    drop(before);

    let now = SystemTime::now();
    let next = AtomicUsize::new(0);
    let found: Mutex<Vec<(usize, FileChange, Option<(SystemTime, u64)>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(8);
    std::thread::scope(|scope| {
        for _ in 0..threads.min(paths.len()) {
            scope.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else { break };
                    if cancel.is_cancelled() {
                        break;
                    }
                    let meta = std::fs::symlink_metadata(repo.join(path)).ok();
                    let stamp = meta
                        .as_ref()
                        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
                    let change = match (&known[index], stamp) {
                        (Some((time, size, change)), Some(stamp)) if (*time, *size) == stamp => {
                            change.clone()
                        }
                        _ => count_new_file(repo, path, meta.as_ref()),
                    };
                    let settled = stamp.filter(|(time, _)| {
                        now.duration_since(*time).is_ok_and(|age| age > SETTLED)
                    });
                    mine.push((index, change, settled));
                }
                found.lock().unwrap_or_else(|e| e.into_inner()).extend(mine);
            });
        }
    });
    if cancel.is_cancelled() {
        return Err(ops::CANCELLED.to_owned());
    }

    let mut found = found.into_inner().unwrap_or_else(|e| e.into_inner());
    found.sort_by_key(|(index, ..)| *index);
    let mut kept = HashMap::new();
    let mut files = Vec::with_capacity(found.len());
    for (index, change, settled) in found {
        let path = paths[index];
        if let Some((time, size)) = settled {
            kept.insert(path.to_owned(), (time, size, change.clone()));
        }
        files.push(WorkingFile {
            path: path.to_owned(),
            change,
            untracked: true,
        });
    }
    if let Ok(mut all) = REMEMBERED.lock() {
        all.get_or_insert_with(HashMap::new)
            .insert(repo.to_path_buf(), kept);
    }
    Ok(files)
}

/// How an untracked file is listed: added, with every line counted as an addition. One
/// that is too large to show, or isn't text, is listed as binary.
fn count_new_file(repo: &Path, path: &str, meta: Option<&std::fs::Metadata>) -> FileChange {
    let mut change = FileChange {
        status: 'A',
        path: path.into(),
        binary: true,
        ..FileChange::default()
    };
    if !meta.is_some_and(|meta| meta.is_file() && meta.len() <= MAX_SHOWN_FILE) {
        return change;
    }
    let Ok(content) = std::fs::read(repo.join(path)) else {
        return change;
    };
    // Git's own test for a binary file: a NUL in the first 8000 bytes.
    if content[..content.len().min(8000)].contains(&0) {
        return change;
    }
    change.binary = false;
    let newlines = content.iter().filter(|byte| **byte == b'\n').count();
    let unfinished = content.last().is_some_and(|byte| *byte != b'\n');
    change.additions = (newlines + usize::from(unfinished)) as u32;
    change
}

/// One file's changes as they are drawn, in the chosen [`DiffView`], read when it is opened.
/// A file git doesn't know yet is all additions whatever the view.
pub fn file_diff(
    repo: &Path,
    file: &WorkingFile,
    side: Side,
    view: DiffView,
) -> Result<FilePatch, String> {
    if file.untracked {
        return Ok(read_new_file(repo, &file.path));
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
/// plain patch as it stands now, so read again after each move.
pub fn move_hunk(repo: &Path, file: &WorkingFile, hunk: usize, side: Side) -> Result<(), String> {
    let plain = file_diff(repo, file, side, DiffView::default())?;
    if hunk >= plain.hunks.len() {
        return Err("That change is gone; the list has been refreshed.".to_owned());
    }
    let patch = plain.with_hunks([hunk]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;
    use std::fs;

    fn read(repo: &Path) -> Result<Changes, String> {
        super::read(repo, &Cancel::new())
    }

    /// How many hunks a listed file's plain patch has.
    fn hunks(repo: &Repo, file: &WorkingFile, side: Side) -> usize {
        let patch = file_diff(repo.path(), file, side, DiffView::default()).unwrap();
        patch.hunks.len()
    }

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
        assert_eq!(read(repo.path()).unwrap().staged[0].change.status, 'D');
        unstage_files(repo.path(), &["gone".to_owned()]).unwrap();
        assert_eq!(read(repo.path()).unwrap().unstaged[0].change.status, 'D');
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
        assert_eq!(hunks(&repo, both, Side::Unstaged), 2);
        assert!(both.has_movable_hunks());
        move_hunk(repo.path(), both, 1, Side::Unstaged).unwrap();

        let changes = read(repo.path()).unwrap();
        assert_eq!(hunks(&repo, &changes.staged[0], Side::Staged), 1);
        assert_eq!(hunks(&repo, &changes.unstaged[0], Side::Unstaged), 1);
        assert_eq!(
            repo.git(&["show", ":file"]),
            long("top", "new bottom").trim()
        );
        assert_eq!(repo.read("file"), long("new top", "new bottom"));

        move_hunk(repo.path(), &changes.staged[0], 0, Side::Staged).unwrap();
        let changes = read(repo.path()).unwrap();
        assert!(changes.staged.is_empty());
        assert_eq!(hunks(&repo, &changes.unstaged[0], Side::Unstaged), 2);
    }

    #[test]
    fn untracked_files_are_counted_without_their_text_being_read_by_git() {
        let repo = Repo::new();
        repo.write("keep", "x\n");
        repo.commit("base");
        repo.write("two", "a\nb\n");
        repo.write("unfinished", "a\nb");
        repo.write("empty", "");
        fs::write(repo.path().join("binary"), b"a\0b\n").unwrap();
        let changes = read(repo.path()).unwrap();
        let counts = |path: &str| {
            let change = &file(&changes.unstaged, path).change;
            (
                change.status,
                change.additions,
                change.deletions,
                change.binary,
            )
        };
        assert_eq!(counts("two"), ('A', 2, 0, false));
        assert_eq!(counts("unfinished"), ('A', 2, 0, false));
        assert_eq!(counts("empty"), ('A', 0, 0, false));
        assert_eq!(counts("binary"), ('A', 0, 0, true));
        // The text itself is read when the file is opened, as all additions.
        let two = file(&changes.unstaged, "two");
        let patch = file_diff(repo.path(), two, Side::Unstaged, DiffView::default()).unwrap();
        assert_eq!(patch.hunks[0].lines.len(), 2);
    }

    #[test]
    fn a_cancelled_read_stops_instead_of_listing_files() {
        let repo = Repo::new();
        repo.write("new", "x\n");
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(
            super::read(repo.path(), &cancel),
            Err(ops::CANCELLED.to_owned())
        );
    }

    #[test]
    fn thousands_of_changed_files_are_listed_and_a_second_read_agrees() {
        let repo = Repo::new();
        for n in 0..1500 {
            repo.write(&format!("tracked/{n:04}"), "a\n");
        }
        repo.commit("base");
        for n in 0..1500 {
            repo.write(&format!("tracked/{n:04}"), "a\nb\n");
            repo.write(&format!("new/{n:04}"), "x\ny\nz\n");
        }
        let first = read(repo.path()).unwrap();
        assert_eq!(first.unstaged.len(), 3000);
        assert_eq!(file(&first.unstaged, "tracked/0007").change.additions, 1);
        assert_eq!(file(&first.unstaged, "new/0007").change.additions, 3);
        assert_eq!(read(repo.path()).unwrap(), first);
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
    fn words_name_the_side() {
        assert_eq!(Side::Unstaged.action("a.rs"), "Stage a.rs");
        assert_eq!(Side::Staged.action("a.rs"), "Unstage a.rs");
        assert_eq!(Side::Staged.hunk_note(), "in this commit");
    }
}
