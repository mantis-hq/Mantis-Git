//! The app's caches, and the keys that make them safe.
//!
//! What a commit changed never changes, because a commit's hash covers its contents. But
//! *how git reports it* depends on more than the commit: config such as `diff.renames`,
//! attribute files that mark a file binary, the git version, and replacement refs. So every
//! key starts with a fingerprint of those, and a change to any of them makes old entries
//! unreachable instead of wrong.
//!
//! The caches themselves are in [`crate::cache`]; this file only says what goes in them.

use super::{Cache, Config, DiskSettings, hash64};
use crate::details::{self, CommitDetails, FileChange};
use crate::ops::{self, Cancel};
use crate::patch::{DiffView, FilePatch};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MEMORY: usize = 64 * 1024 * 1024;

/// Every cache the app keeps. Clones share entries.
#[derive(Clone)]
pub struct Caches {
    details: Cache<CommitDetails>,
    diffs: Cache<FilePatch>,
}

impl Caches {
    const DETAILS: Config = Config {
        name: "commit-files",
        memory_bytes: MEMORY,
    };
    const DIFFS: Config = Config {
        name: "file-diffs",
        memory_bytes: MEMORY,
    };

    /// Caches that keep nothing on disk.
    pub fn memory_only() -> Self {
        Self {
            details: Cache::memory_only(Self::DETAILS),
            diffs: Cache::memory_only(Self::DIFFS),
        }
    }

    /// Caches that also keep entries under `root`. The size limit is shared by the two.
    pub fn on_disk(root: PathBuf, disk: DiskSettings) -> Self {
        let half = DiskSettings {
            max_bytes: disk.max_bytes / 2,
            ..disk
        };
        Self {
            details: Cache::new(Self::DETAILS, root.clone(), half),
            diffs: Cache::new(Self::DIFFS, root, half),
        }
    }

    /// Applies new settings at once, to caches already in use.
    pub fn set_disk(&self, disk: DiskSettings) {
        let half = DiskSettings {
            max_bytes: disk.max_bytes / 2,
            ..disk
        };
        self.details.set_disk(half);
        self.diffs.set_disk(half);
    }

    /// Forgets everything, in memory now and on disk soon.
    pub fn clear(&self) {
        self.details.clear();
        self.diffs.clear();
    }

    /// How much is on disk. Blocks.
    pub fn disk_size(&self) -> u64 {
        self.details.disk_size() + self.diffs.disk_size()
    }

    /// A commit's details if they are in memory. Safe on the UI thread.
    pub fn peek_details(&self, fingerprint: &str, oid: &str) -> Option<Arc<CommitDetails>> {
        self.details.peek(&details_key(fingerprint, oid))
    }

    /// A commit's details from the cache, or else from git, which are then cached. Blocks.
    pub fn read_details(
        &self,
        repo: &Path,
        fingerprint: &str,
        oid: &str,
        cancel: &Cancel,
    ) -> Result<Arc<CommitDetails>, String> {
        self.details
            .get_or_load(&details_key(fingerprint, oid), || {
                details::read(repo, oid, cancel)
            })
    }

    /// One file's diff in a commit, if it is in memory. Safe on the UI thread.
    pub fn peek_diff(
        &self,
        fingerprint: &str,
        oid: &str,
        file: &FileChange,
    ) -> Option<Arc<FilePatch>> {
        self.diffs.peek(&diff_key(fingerprint, oid, file))
    }

    /// One file's diff from the cache, or else from git, which is then cached. Blocks.
    pub fn read_diff(
        &self,
        repo: &Path,
        fingerprint: &str,
        oid: &str,
        file: &FileChange,
    ) -> Result<Arc<FilePatch>, String> {
        self.diffs
            .get_or_load(&diff_key(fingerprint, oid, file), || {
                details::file_diff(repo, oid, file, DiffView::default())
            })
    }
}

fn details_key(fingerprint: &str, oid: &str) -> String {
    format!("details/{fingerprint}/{oid}")
}

fn diff_key(fingerprint: &str, oid: &str, file: &FileChange) -> String {
    let from = file.old_path.as_deref().unwrap_or_default();
    format!("diff/{fingerprint}/{oid}/{from}\0{}", file.path)
}

/// Everything besides a commit that decides what git reports about it: the git version,
/// `diff.*` and `core.*` config, replacement refs, and the attribute and graft files.
/// Blocks, so call it off the UI thread, and again when the repository's config may have
/// changed.
pub fn fingerprint(repo: &Path) -> String {
    let mut text = String::new();
    let mut add = |label: &str, value: String| {
        text.push_str(label);
        text.push('\0');
        text.push_str(&value);
        text.push('\0');
    };
    add(
        "version",
        ops::run(repo, &["--version"]).unwrap_or_default(),
    );
    // Exit status 1 means no such setting, which is a value like any other.
    add(
        "config",
        ops::run(repo, &["config", "--get-regexp", r"^(diff|core)\."]).unwrap_or_default(),
    );
    add(
        "replace",
        ops::run(repo, &["for-each-ref", "refs/replace"]).unwrap_or_default(),
    );

    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap_or_default();
    if let Ok(common) = ops::run(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        let common = PathBuf::from(common.trim());
        add("grafts", read(common.join("info").join("grafts")));
        add(
            "info-attributes",
            read(common.join("info").join("attributes")),
        );
    }
    add("attributes", read(repo.join(".gitattributes")));
    if let Some(home) = crate::state::config_dir() {
        add(
            "global-attributes",
            read(home.join("git").join("attributes")),
        );
    }
    format!("{:016x}", hash64(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    fn root_commit() -> (Repo, String) {
        let repo = Repo::new();
        repo.write("a.txt", "one\n");
        repo.write("b.txt", "two\n");
        let oid = repo.commit("first");
        (repo, oid)
    }

    #[test]
    fn the_second_read_comes_from_the_cache() {
        let (repo, oid) = root_commit();
        let caches = Caches::memory_only();
        let cancel = Cancel::new();
        assert!(caches.peek_details("fp", &oid).is_none());
        let first = caches
            .read_details(repo.path(), "fp", &oid, &cancel)
            .unwrap();
        assert!(caches.peek_details("fp", &oid).is_some());

        // The repository is gone, yet the commit's details are still there.
        let path = repo.path().to_owned();
        drop(repo);
        let again = caches.read_details(&path, "fp", &oid, &cancel).unwrap();
        assert_eq!(first, again);
    }

    #[test]
    fn a_cancelled_read_is_not_cached() {
        let (repo, oid) = root_commit();
        let caches = Caches::memory_only();
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(
            caches
                .read_details(repo.path(), "fp", &oid, &cancel)
                .is_err()
        );
        assert!(caches.peek_details("fp", &oid).is_none());
        assert!(
            caches
                .read_details(repo.path(), "fp", &oid, &Cancel::new())
                .is_ok()
        );
    }

    #[test]
    fn details_and_diffs_survive_a_restart_through_the_disk() {
        let (repo, oid) = root_commit();
        let root = tempfile::tempdir().unwrap();
        let first = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let cancel = Cancel::new();
        let details = first
            .read_details(repo.path(), "fp", &oid, &cancel)
            .unwrap();
        let file = details.files[0].clone();
        let patch = first.read_diff(repo.path(), "fp", &oid, &file).unwrap();
        first.details.flush();
        first.diffs.flush();

        // A new process, and the repository is no longer needed to answer.
        let second = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let gone = Path::new("/nonexistent");
        assert_eq!(
            second.read_details(gone, "fp", &oid, &cancel).unwrap(),
            details
        );
        assert_eq!(second.read_diff(gone, "fp", &oid, &file).unwrap(), patch);
        assert!(second.disk_size() > 0);
        second.clear();
        assert_eq!(second.disk_size(), 0);
    }

    #[test]
    fn a_different_fingerprint_reaches_nothing() {
        let (repo, oid) = root_commit();
        let caches = Caches::memory_only();
        caches
            .read_details(repo.path(), "one", &oid, &Cancel::new())
            .unwrap();
        assert!(caches.peek_details("two", &oid).is_none());
    }

    #[test]
    fn the_fingerprint_follows_what_changes_how_git_reports_a_commit() {
        let (repo, _) = root_commit();
        let before = fingerprint(repo.path());
        assert_eq!(
            before,
            fingerprint(repo.path()),
            "stable while nothing changes"
        );

        repo.git(&["config", "diff.renames", "false"]);
        let renames_off = fingerprint(repo.path());
        assert_ne!(before, renames_off);

        repo.write(".gitattributes", "*.bin -diff\n");
        assert_ne!(renames_off, fingerprint(repo.path()));
    }

    #[test]
    fn a_patch_with_every_kind_of_line_round_trips() {
        use crate::cache::{Cacheable, Reader, Writer};
        let diff =
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@ fn main\n keep\n-old\n+new\n";
        let patch = crate::patch::parse(diff).into_iter().next().unwrap();
        let mut out = Writer::new();
        patch.encode(&mut out);
        let bytes = out.finish();
        let mut input = Reader::new(&bytes);
        let back = FilePatch::decode(&mut input).unwrap();
        assert!(input.is_empty());
        assert_eq!(back, patch);
        // Cut short anywhere, it is refused and never panics.
        for cut in 0..bytes.len() {
            assert!(FilePatch::decode(&mut Reader::new(&bytes[..cut])).is_none());
        }
    }
}
