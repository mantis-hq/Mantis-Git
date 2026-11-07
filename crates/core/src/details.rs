//! What a single commit changed, and the commands that act on one.

use crate::cache::{Cacheable, Reader, Writer};
use crate::ops::{self, Cancel};
use crate::patch::{self, DiffView, FilePatch};
use crate::text::{count, short_oid};
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
fn first_parent(repo: &Path, oid: &str) -> Option<String> {
    let parents = ops::run(
        repo,
        &["log", "-1", "--no-show-signature", "--format=%P", oid],
    )
    .ok()?;
    (parents.split_whitespace().count() > 1).then(|| format!("{oid}^1"))
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
