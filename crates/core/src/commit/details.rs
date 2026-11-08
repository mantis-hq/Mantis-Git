//! What a single commit changed: its files, and the diff of each.

use crate::cache::{Cacheable, Reader, Writer};
use crate::ops::{self, Cancel};
use crate::patch::{self, DiffView, FilePatch};
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

impl Cacheable for CommitDetails {
    // 2: merges are read against their first parent. Version 1 stored them with no files.
    const FORMAT: u32 = 2;

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
        out.len(self.files.len());
        for file in &self.files {
            out.u32(u32::from(file.status));
            out.str(&file.path);
            out.opt_str(file.old_path.as_deref());
            out.u32(file.additions);
            out.u32(file.deletions);
            out.bool(file.binary);
        }
    }

    fn decode(input: &mut Reader) -> Option<Self> {
        let oid = input.str()?.into();
        let body = input.str()?.to_owned();
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
            });
        }
        Some(Self { oid, body, files })
    }
}

/// Reads what a commit changed. A set `cancel` stops it with [`ops::CANCELLED`], which
/// matters because a commit of thousands of files takes a while to list.
pub fn read(repo: &Path, oid: &str, cancel: &Cancel) -> Result<CommitDetails, String> {
    let body = ops::run_cancellable(
        repo,
        &["log", "-1", "--no-show-signature", "--format=%b", oid],
        cancel,
    )?;
    let output = ops::run_cancellable(
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
        ]
        .into_iter()
        .chain(first_parent(repo, oid).as_deref())
        .chain([oid])
        .collect::<Vec<_>>(),
        cancel,
    )?;
    Ok(CommitDetails {
        oid: oid.into(),
        body: body.trim().to_owned(),
        files: parse_changes(&output),
    })
}

/// A merge has no diff of its own, so it is read against its first parent: what it brought
/// onto the branch it was made on, the side `revert -m 1` undoes. Other commits need none.
pub(crate) fn first_parent(repo: &Path, oid: &str) -> Option<String> {
    let parents = ops::run(
        repo,
        &["log", "-1", "--no-show-signature", "--format=%P", oid],
    )
    .ok()?;
    (parents.split_whitespace().count() > 1).then(|| format!("{oid}^1"))
}

/// Parses `--raw --numstat -z`: every file's raw record, then every file's numstat record.
pub(crate) fn parse_changes(output: &str) -> Vec<FileChange> {
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

/// Reads one file's diff within a commit.
pub fn file_diff(
    repo: &Path,
    oid: &str,
    file: &FileChange,
    view: DiffView,
) -> Result<FilePatch, String> {
    let command = ["diff-tree", "--root", "-r", "-p", "-M", "--no-commit-id"];
    let mut args = patch::diff_args(&command);
    args.extend(view.args());
    let base = first_parent(repo, oid);
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
        assert_eq!(details.files.len(), 1);
        assert_eq!(&*details.files[0].path, "b");
        assert_eq!(details.files[0].status, 'A');
        let patch = file_diff(repo.path(), &merge, &details.files[0], DiffView::default()).unwrap();
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
        let patch = file_diff(repo.path(), &oid, &details.files[0], DiffView::default()).unwrap();
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
        let patch = file_diff(repo.path(), &oid, &details.files[0], DiffView::default()).unwrap();
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
        let patch = file_diff(repo.path(), &oid, &details.files[0], DiffView::default()).unwrap();
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

        let plain = file_diff(repo.path(), &oid, file, DiffView::default()).unwrap();
        assert!(plain.hunks.len() > 1);
        let whole = DiffView {
            whole_file: true,
            ..DiffView::default()
        };
        let whole = file_diff(repo.path(), &oid, file, whole).unwrap();
        assert_eq!(whole.hunks.len(), 1);
        let quiet = DiffView {
            ignore_whitespace: true,
            ..DiffView::default()
        };
        let quiet = file_diff(repo.path(), &oid, file, quiet).unwrap();
        assert_eq!(
            quiet.hunks.len(),
            1,
            "only the edit on the first line is left"
        );
    }
}
