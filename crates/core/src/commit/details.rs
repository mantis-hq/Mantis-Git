//! What a single commit changed: its files, and the diff of each.

use crate::cache::{Cacheable, Reader, Writer};
use crate::ops::{self, Cancel};
use crate::patch::{self, DiffView, FilePatch};
use crate::text;
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
    /// The bigger of the file's two versions, in bytes. 0 when it wasn't asked.
    pub size: u64,
}

/// A file bigger than this has its changes counted and drawn only when asked for. Git's
/// diff takes time that grows faster than the file does when the two versions differ a
/// lot, so a huge file can hold everything up for a minute. The limit is high on purpose:
/// Unity scenes of 35 MB diff in a fraction of a second, and an earlier limit of 8 MB hid
/// those for nothing. Unit tests use a small one so they needn't write a huge file.
pub const LARGE_FILE_BYTES: u64 = if cfg!(test) {
    8 * 1024 * 1024
} else {
    256 * 1024 * 1024
};

impl FileChange {
    /// Whether its diff is left until the reader asks for it.
    pub fn large(&self) -> bool {
        self.size > LARGE_FILE_BYTES
    }

    /// What ends the file's row: `+n −n`, `binary`, or its size when it is large. Empty while
    /// the counts are still being read (`counted` is false).
    pub fn count_label(&self, counted: bool) -> String {
        if self.large() {
            text::bytes(self.size)
        } else if !counted {
            String::new()
        } else if self.binary {
            "binary".to_owned()
        } else {
            format!("+{} −{}", self.additions, self.deletions)
        }
    }

    /// Whether [`file_diff`] will leave this file's diff unread with `view`.
    pub fn waits_to_be_asked(&self, view: DiffView) -> bool {
        self.large() && !view.allow_large
    }

    /// Why no changes are shown for a large file, and what asking for them costs.
    pub fn large_sentence(&self) -> String {
        format!(
            "This file is {}. Its changes aren't worked out until you ask, because that can \
             take a long time. Press the button to read them anyway.",
            text::bytes(self.size)
        )
    }

    /// The button that asks for a large file's diff.
    pub fn large_button(&self) -> String {
        let (_, name) = text::split_path(&self.path);
        format!("Show changes in {name} ({})", text::bytes(self.size))
    }

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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitDetails {
    pub oid: Arc<str>,
    /// The message after its subject line.
    pub body: String,
    /// Whether it has more than one parent, so its files are read against the first.
    pub merge: bool,
    pub files: Vec<FileChange>,
    /// Whether each file's `+n −n` has been read. [`read_names`] leaves them out, since
    /// counting means diffing every file; [`count`] fills them in.
    pub counted: bool,
}

impl CommitDetails {
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(add, del), file| {
            (add + file.additions, del + file.deletions)
        })
    }
}

impl Cacheable for CommitDetails {
    // 2: merges are read against their first parent. Version 1 stored them with no files.
    // 3: remembers whether it is a merge, so a file's diff needn't ask git again.
    // 4: remembers each file's size, so a large one can wait to be asked for.
    const FORMAT: u32 = 4;

    fn weight(&self) -> usize {
        let files: usize = self
            .files
            .iter()
            .map(|file| file.path.len() + file.old_path.as_ref().map_or(0, |old| old.len()) + 64)
            .sum();
        self.oid.len() + self.body.len() + files + 64
    }

    fn encode(&self, out: &mut Writer) {
        out.str(&self.oid);
        out.str(&self.body);
        out.bool(self.merge);
        out.len(self.files.len());
        for file in &self.files {
            out.u32(u32::from(file.status));
            out.str(&file.path);
            out.opt_str(file.old_path.as_deref());
            out.u32(file.additions);
            out.u32(file.deletions);
            out.bool(file.binary);
            out.u64(file.size);
        }
    }

    fn decode(input: &mut Reader) -> Option<Self> {
        let oid = input.str()?.into();
        let body = input.str()?.to_owned();
        let merge = input.bool()?;
        let count = input.len()?;
        let mut files = Vec::new();
        for _ in 0..count {
            files.push(FileChange {
                status: char::from_u32(input.u32()?)?,
                path: input.str()?.into(),
                old_path: input.opt_str()?.map(Into::into),
                additions: input.u32()?,
                deletions: input.u32()?,
                binary: input.bool()?,
                size: input.u64()?,
            });
        }
        // Only counted details are cached.
        Some(Self {
            oid,
            body,
            merge,
            files,
            counted: true,
        })
    }
}

/// Reads what a commit changed, counts included. A set `cancel` stops it with
/// [`ops::CANCELLED`], which matters because a commit of thousands of files takes a while
/// to list.
pub fn read(repo: &Path, oid: &str, cancel: &Cancel) -> Result<CommitDetails, String> {
    count(repo, read_names(repo, oid, cancel)?, cancel)
}

/// Reads which files a commit changed and how big they are, without diffing any of them,
/// so it answers at once whatever the files hold. The counts come from [`count`].
pub fn read_names(repo: &Path, oid: &str, cancel: &Cancel) -> Result<CommitDetails, String> {
    // One call for both, since they are fields of the same commit: the parents, a NUL, then
    // the body.
    let head = ops::run_cancellable(
        repo,
        &["log", "-1", "--no-show-signature", "--format=%P%x00%b", oid],
        cancel,
    )?;
    let (parents, body) = head.split_once('\0').unwrap_or((&head, ""));
    let merge = parents.split_whitespace().count() > 1;
    let output = ops::run_cancellable(
        repo,
        &[
            "diff-tree",
            "--root",
            "-r",
            "-z",
            "-M",
            "--raw",
            "--no-commit-id",
        ]
        .into_iter()
        .chain(base(oid, merge).as_deref())
        .chain([oid])
        .collect::<Vec<_>>(),
        cancel,
    )?;
    let mut files = parse_changes(&output);
    set_sizes(repo, &output, &mut files, cancel)?;
    Ok(CommitDetails {
        oid: oid.into(),
        body: body.trim().to_owned(),
        merge,
        files,
        counted: false,
    })
}

/// Fills in each file's `+n −n`, which means diffing every file, so it is the slow half of
/// reading a commit. A [large](FileChange::large) file is left uncounted: git is told to
/// treat anything over [`LARGE_FILE_BYTES`] as binary for this one call, which skips it.
pub fn count(
    repo: &Path,
    mut details: CommitDetails,
    cancel: &Cancel,
) -> Result<CommitDetails, String> {
    let limit = format!("core.bigFileThreshold={LARGE_FILE_BYTES}");
    let output = ops::run_cancellable(
        repo,
        &[
            "-c",
            &limit,
            "diff-tree",
            "--root",
            "-r",
            "-z",
            "-M",
            "--numstat",
            "--no-commit-id",
        ]
        .into_iter()
        .chain(base(&details.oid, details.merge).as_deref())
        .chain([&*details.oid])
        .collect::<Vec<_>>(),
        cancel,
    )?;
    set_counts(&mut details.files, &output);
    // What git calls binary only because of the limit is large, not binary.
    for file in details.files.iter_mut().filter(|file| file.large()) {
        file.binary = false;
    }
    details.counted = true;
    Ok(details)
}

/// Looks up how big each file is from the blob ids in `--raw` output, in one call.
fn set_sizes(
    repo: &Path,
    raw: &str,
    files: &mut [FileChange],
    cancel: &Cancel,
) -> Result<(), String> {
    // Per file, its old and new blob. An absent side is all zeros, which has no size.
    let blobs: Vec<[&str; 2]> = raw
        .split('\0')
        .filter_map(|field| field.strip_prefix(':'))
        .map(|record| {
            let mut parts = record.split(' ').skip(2);
            [parts.next().unwrap_or(""), parts.next().unwrap_or("")]
        })
        .collect();
    let mut input = String::new();
    for blob in blobs.iter().flatten() {
        // Every line must answer, so an absent side asks about a name that can't exist.
        input.push_str(if blob.bytes().all(|byte| byte == b'0') {
            "missing"
        } else {
            blob
        });
        input.push('\n');
    }
    if input.is_empty() || cancel.is_cancelled() {
        return if cancel.is_cancelled() {
            Err(ops::CANCELLED.into())
        } else {
            Ok(())
        };
    }
    let sizes = ops::run_with_input(
        repo,
        &["cat-file", "--batch-check=%(objectsize)"],
        input.as_bytes(),
    )?;
    let mut sizes = sizes.lines().map(|line| line.parse::<u64>().unwrap_or(0));
    for file in files {
        let (old, new) = (sizes.next().unwrap_or(0), sizes.next().unwrap_or(0));
        file.size = old.max(new);
    }
    Ok(())
}

/// What a commit is read against: a merge's first parent, else nothing.
fn base(oid: &str, merge: bool) -> Option<String> {
    merge.then(|| format!("{oid}^1"))
}

/// A merge has no diff of its own, so it is read against its first parent: what it brought
/// onto the branch it was made on, the side `revert -m 1` undoes. Other commits need none.
pub(crate) fn first_parent(repo: &Path, oid: &str) -> Option<String> {
    let parents = ops::run(
        repo,
        &["log", "-1", "--no-show-signature", "--format=%P", oid],
    )
    .ok()?;
    base(oid, parents.split_whitespace().count() > 1)
}

/// Parses `--raw --numstat -z`: every file's raw record, then every file's numstat record.
pub(crate) fn parse_changes(output: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
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
        }
    }
    set_counts(&mut files, output);
    files
}

/// Reads the numstat records of `-z` output into the files, in order. Raw records are
/// skipped, so the output may hold both, or only one of them.
fn set_counts(files: &mut [FileChange], output: &str) {
    let mut index = 0;
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.starts_with(':') {
            // A raw record: its path follows, or two for a rename or copy.
            let status = field.split(' ').nth(4).unwrap_or("M");
            let paths = if status.starts_with(['R', 'C']) { 2 } else { 1 };
            fields.nth(paths - 1);
            continue;
        }
        let mut stats = field.splitn(3, '\t');
        let (additions, deletions) = (stats.next(), stats.next());
        // A rename's numstat leaves its path empty and gives both paths as fields.
        if stats.next().is_none_or(str::is_empty) {
            fields.next();
            fields.next();
        }
        if let Some(file) = files.get_mut(index) {
            file.binary = additions == Some("-");
            file.additions = additions.and_then(|n| n.parse().ok()).unwrap_or(0);
            file.deletions = deletions.and_then(|n| n.parse().ok()).unwrap_or(0);
        }
        index += 1;
    }
}

/// Reads one file's diff within a commit.
pub fn file_diff(
    repo: &Path,
    commit: &CommitDetails,
    file: &FileChange,
    view: DiffView,
) -> Result<FilePatch, String> {
    if file.waits_to_be_asked(view) {
        return Ok(FilePatch::empty(&file.path));
    }
    let oid = &*commit.oid;
    let command = ["diff-tree", "--root", "-r", "-p", "-M", "--no-commit-id"];
    let mut args = patch::diff_args(&command);
    args.extend(view.args());
    let base = base(oid, commit.merge);
    args.extend(base.as_deref());
    args.extend([oid, "--", &*file.path]);
    // A rename only shows as one if git can see both of its ends.
    if let Some(old_path) = &file.old_path {
        args.push(old_path);
    }
    let diff = ops::run(repo, &args)?;
    Ok(patch::parse(&diff).into_iter().next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;
    use std::fs;

    fn lines(count: usize, word: &str) -> String {
        (0..count)
            .map(|index| format!("{word} {index}\n"))
            .collect()
    }

    #[test]
    fn names_come_first_and_counts_after() {
        let repo = Repo::new();
        repo.write("a.txt", "one\n");
        repo.write("old.txt", "x\n");
        repo.commit("first");
        repo.write("a.txt", "one\ntwo\nthree\n");
        fs::remove_file(repo.path().join("old.txt")).unwrap();
        let oid = repo.commit("second");

        let names = read_names(repo.path(), &oid, &Cancel::new()).unwrap();
        assert!(!names.counted);
        let paths: Vec<_> = names.files.iter().map(|file| &*file.path).collect();
        assert_eq!(paths, ["a.txt", "old.txt"]);
        assert_eq!(names.files[0].additions, 0);
        assert_eq!(names.files[0].size, 14);

        let counted = count(repo.path(), names, &Cancel::new()).unwrap();
        assert!(counted.counted);
        assert_eq!(counted.files[0].additions, 2);
        assert_eq!(counted.files[1].deletions, 1);
        assert_eq!(
            counted,
            read(repo.path(), &oid, &Cancel::new()).unwrap(),
            "reading in two steps gives what one read does"
        );
    }

    #[test]
    fn a_large_file_waits_to_be_asked_for() {
        let repo = Repo::new();
        repo.write("scene.unity", "a\n");
        repo.write("small.txt", "a\n");
        repo.commit("first");
        let big = "line of text\n".repeat(LARGE_FILE_BYTES as usize / 13 + 10);
        repo.write("scene.unity", &big);
        repo.write("small.txt", "b\n");
        let oid = repo.commit("second");

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        let (scene, small) = (&details.files[0], &details.files[1]);
        assert!(scene.large() && !small.large());
        // Not counted, but not called binary either.
        assert!(!scene.binary && scene.additions == 0);
        assert_eq!((small.additions, small.deletions), (1, 1));

        let view = DiffView::default();
        assert!(scene.waits_to_be_asked(view));
        let waiting = file_diff(repo.path(), &details, scene, view).unwrap();
        assert!(waiting.hunks.is_empty());

        let asked = DiffView {
            allow_large: true,
            ..view
        };
        assert!(!scene.waits_to_be_asked(asked));
        let patch = file_diff(repo.path(), &details, scene, asked).unwrap();
        assert!(!patch.hunks.is_empty());
        assert!(
            scene
                .large_button()
                .starts_with("Show changes in scene.unity (")
        );
    }

    #[test]
    fn a_cancelled_read_stops_instead_of_listing_files() {
        let repo = Repo::new();
        repo.write("a", "one\n");
        let oid = repo.commit("first");

        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(
            read(repo.path(), &oid, &cancel),
            Err(ops::CANCELLED.to_owned())
        );
    }

    #[test]
    fn a_read_of_many_files_lists_them_all() {
        let repo = Repo::new();
        for index in 0..300 {
            repo.write(&format!("dir/file{index}"), "x\n");
        }
        let oid = repo.commit("many");

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        assert_eq!(details.files.len(), 300);
    }

    #[test]
    fn a_root_commit_lists_its_files_as_added() {
        let repo = Repo::new();
        repo.write("src/app.rs", "one\ntwo\n");
        let oid = repo.commit("first\n\nThe details.\nAcross lines.");

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        assert_eq!(details.body, "The details.\nAcross lines.");
        assert_eq!(details.files.len(), 1);
        let file = &details.files[0];
        assert_eq!((file.status, file.additions, file.deletions), ('A', 2, 0));
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

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
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

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
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
        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        assert_eq!(&*details.files[0].path, "with space/a\tb.txt");
        assert_eq!(details.files[0].additions, 1);
    }

    #[test]
    fn a_merge_shows_what_it_brought_in_over_its_first_parent() {
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
        let details = read(repo.path(), &merge, &Cancel::new()).unwrap();
        assert!(details.merge);
        assert_eq!(details.files.len(), 1);
        assert_eq!(&*details.files[0].path, "b");
        assert_eq!(details.files[0].status, 'A');
        let patch = file_diff(
            repo.path(),
            &details,
            &details.files[0],
            DiffView::default(),
        )
        .unwrap();
        assert_eq!(patch.path, "b");
        assert_eq!(patch.hunks[0].lines.len(), 1);
    }

    #[test]
    fn a_files_diff_is_read_on_its_own() {
        let repo = Repo::new();
        repo.write("a", "one\n");
        repo.write("b", "two\n");
        repo.commit("base");
        repo.write("a", "changed\n");
        repo.write("b", "also\n");
        let oid = repo.commit("both");

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        let patch = file_diff(
            repo.path(),
            &details,
            &details.files[0],
            DiffView::default(),
        )
        .unwrap();
        assert_eq!(patch.path, "a");
        assert_eq!(patch.hunks[0].lines.len(), 2);
    }

    #[test]
    fn a_renamed_files_diff_shows_only_what_changed() {
        let repo = Repo::new();
        repo.write("before", &lines(20, "same"));
        repo.commit("base");
        repo.git(&["mv", "before", "after"]);
        repo.write("after", &format!("{}extra\n", lines(20, "same")));
        let oid = repo.commit("rename");

        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        let patch = file_diff(
            repo.path(),
            &details,
            &details.files[0],
            DiffView::default(),
        )
        .unwrap();
        assert_eq!(patch.path, "after");
        let added = patch.hunks[0].lines.iter().filter(|l| l.old.is_none());
        assert_eq!(added.count(), 1, "not the whole file");
    }

    #[test]
    fn a_root_commits_file_diff_is_all_additions() {
        let repo = Repo::new();
        repo.write("a", "one\ntwo\n");
        let oid = repo.commit("root");
        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        let patch = file_diff(
            repo.path(),
            &details,
            &details.files[0],
            DiffView::default(),
        )
        .unwrap();
        assert_eq!(patch.hunks[0].lines.len(), 2);
    }

    #[test]
    fn a_commits_file_can_be_shown_whole_and_without_white_space_changes() {
        let repo = Repo::new();
        let lines = |tail: &str| (0..30).map(|n| format!("l{n}\n")).collect::<String>() + tail;
        repo.write("f", &lines("end\n"));
        repo.commit("base");
        repo.write("f", &lines("end  \n").replacen("l0\n", "changed\n", 1));
        let oid = repo.commit("edit");
        let details = read(repo.path(), &oid, &Cancel::new()).unwrap();
        let file = &details.files[0];

        let plain = file_diff(repo.path(), &details, file, DiffView::default()).unwrap();
        assert!(plain.hunks.len() > 1);
        let whole = DiffView {
            whole_file: true,
            ..DiffView::default()
        };
        let whole = file_diff(repo.path(), &details, file, whole).unwrap();
        assert_eq!(whole.hunks.len(), 1);
        let quiet = DiffView {
            ignore_whitespace: true,
            ..DiffView::default()
        };
        let quiet = file_diff(repo.path(), &details, file, quiet).unwrap();
        assert_eq!(
            quiet.hunks.len(),
            1,
            "only the edit on the first line is left"
        );
    }
}
