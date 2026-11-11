//! The app's caches, and the keys that make them safe.
//!
//! What a commit changed never changes, because a commit's hash covers its contents. But
//! *how git reports it* depends on more than the commit: config such as `diff.renames`,
//! attribute files that mark a file binary, the git version, and replacement refs. So every
//! key starts with a fingerprint of those, and a change to any of them makes old entries
//! unreachable instead of wrong.
//!
//! The caches themselves are in [`crate::cache`]; this file only says what goes in them.

use super::{Budget, Cache, Config, DiskSettings, hash64};
use crate::commit::details::{self, CommitDetails, FileChange};
use crate::history::Signature;
use crate::history::avatar::Gravatar;
use crate::ops::{self, Cancel};
use crate::patch::{DiffView, FilePatch};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MEMORY: usize = 64 * 1024 * 1024;

/// What one cache holds on disk, for the settings to show what the space is used by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub label: &'static str,
    pub bytes: u64,
}

/// Every cache the app keeps. Clones share entries.
#[derive(Clone)]
pub struct Caches {
    details: Cache<CommitDetails>,
    diffs: Cache<FilePatch>,
    signatures: Cache<Signature>,
    avatars: Cache<Gravatar>,
}

impl Caches {
    const COMMITS: &'static str = "Commits";

    const DETAILS: Config = Config {
        name: "commit-files",
        label: Self::COMMITS,
        memory_bytes: MEMORY,
    };
    /// Signatures sit beside a commit's files under one name, since both are what git says
    /// about a commit, and the settings show them as one part of the bar.
    const SIGNATURES: Config = Config {
        name: "signatures",
        label: Self::COMMITS,
        memory_bytes: 1024 * 1024,
    };
    const DIFFS: Config = Config {
        name: "file-diffs",
        label: "File diffs",
        memory_bytes: MEMORY,
    };

    const AVATARS: Config = Config {
        name: "avatars",
        label: "Avatars",
        memory_bytes: 4 * 1024 * 1024,
    };

    /// Caches that keep nothing on disk.
    pub fn memory_only() -> Self {
        Self {
            details: Cache::memory_only(Self::DETAILS),
            diffs: Cache::memory_only(Self::DIFFS),
            signatures: Cache::memory_only(Self::SIGNATURES),
            avatars: Cache::memory_only(Self::AVATARS),
        }
    }

    /// Caches that also keep entries under `root`, all within one size limit. Whichever
    /// cache fills up gets the room the others leave, and the oldest entries of any of
    /// them go first.
    pub fn on_disk(root: PathBuf, disk: DiskSettings) -> Self {
        let budget = Budget::new(root, disk.max_bytes);
        Self {
            details: Cache::in_pool(Self::DETAILS, &budget, disk.enabled),
            diffs: Cache::in_pool(Self::DIFFS, &budget, disk.enabled),
            signatures: Cache::in_pool(Self::SIGNATURES, &budget, disk.enabled),
            avatars: Cache::in_pool(Self::AVATARS, &budget, disk.enabled),
        }
    }

    /// Applies new settings at once, to caches already in use.
    pub fn set_disk(&self, disk: DiskSettings) {
        // The limit is shared, so one of them setting it is the same as all of them.
        self.details.set_disk(disk);
        self.diffs.set_enabled(disk.enabled);
        self.signatures.set_enabled(disk.enabled);
        self.avatars.set_enabled(disk.enabled);
    }

    /// Forgets everything, in memory now and on disk soon.
    pub fn clear(&self) {
        self.details.clear();
        self.diffs.clear();
        self.signatures.clear();
        self.avatars.clear();
    }

    /// How much is on disk. Blocks.
    pub fn disk_size(&self) -> u64 {
        self.usage().iter().map(|part| part.bytes).sum()
    }

    /// How much each cache holds on disk, largest first. Blocks.
    pub fn usage(&self) -> Vec<Usage> {
        let mut parts = vec![
            Usage {
                label: Self::COMMITS,
                bytes: self.details.disk_size() + self.signatures.disk_size(),
            },
            Usage {
                label: Self::DIFFS.label,
                bytes: self.diffs.disk_size(),
            },
            Usage {
                label: Self::AVATARS.label,
                bytes: self.avatars.disk_size(),
            },
        ];
        parts.sort_by_key(|part| std::cmp::Reverse(part.bytes));
        parts
    }

    /// What is known of an author's Gravatar from an earlier run: the picture, or that there
    /// was none. Blocks.
    pub fn cached_avatar(&self, email: &str) -> Option<Arc<Gravatar>> {
        self.avatars.get(&crate::history::avatar::key(email))
    }

    /// Keeps a Gravatar that was just fetched. Doesn't wait on the disk.
    pub fn store_avatar(&self, email: &str, bytes: Vec<u8>) {
        self.avatars.insert(
            &crate::history::avatar::key(email),
            Gravatar::Picture(bytes),
        );
    }

    /// Keeps that gravatar.com has no picture for an author, for a week.
    pub fn store_no_avatar(&self, email: &str) {
        self.avatars
            .insert(&crate::history::avatar::key(email), Gravatar::missing());
    }

    /// What git says about each of `oids`' signatures, from the cache where it has been
    /// asked before and from git for the rest, which is then kept. Blocks, and runs gpg for
    /// the commits that aren't cached, so call it off the UI thread and for few commits.
    pub fn read_signatures(
        &self,
        repo: &Path,
        oids: &[Arc<str>],
    ) -> Result<Vec<(Arc<str>, Signature)>, String> {
        let mut known = Vec::with_capacity(oids.len());
        let mut missing = Vec::new();
        for oid in oids {
            match self.signatures.get(&signature_key(oid)) {
                Some(signature) => known.push((oid.clone(), *signature)),
                None => missing.push(oid.clone()),
            }
        }
        for (oid, signature) in crate::history::signature::read(repo, &missing)? {
            if signature.is_final() {
                self.signatures.insert(&signature_key(&oid), signature);
            }
            known.push((oid, signature));
        }
        Ok(known)
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

    /// A commit's details from the cache alone, if they are there, in memory or on disk.
    /// Blocks.
    pub fn cached_details(&self, fingerprint: &str, oid: &str) -> Option<Arc<CommitDetails>> {
        self.details.get(&details_key(fingerprint, oid))
    }

    /// Remembers a commit's details once they are counted, which is the only way they are
    /// worth keeping. Doesn't wait on the disk.
    pub fn store_details(&self, fingerprint: &str, details: CommitDetails) -> Arc<CommitDetails> {
        self.details
            .insert(&details_key(fingerprint, &details.oid), details)
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
        commit: &CommitDetails,
        file: &FileChange,
    ) -> Result<Arc<FilePatch>, String> {
        // A diff left unread isn't the file's diff, so it must not be remembered as one.
        if file.waits_to_be_asked(DiffView::default()) {
            return details::file_diff(repo, commit, file, DiffView::default()).map(Arc::new);
        }
        self.diffs
            .get_or_load(&diff_key(fingerprint, &commit.oid, file), || {
                details::file_diff(repo, commit, file, DiffView::default())
            })
    }
}

fn signature_key(oid: &str) -> String {
    format!("signature/{oid}")
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
        let common = crate::repo::wsl::from_git(repo, common.trim());
        add("grafts", read(common.join("info").join("grafts")));
        add(
            "info-attributes",
            read(common.join("info").join("attributes")),
        );
    }
    add("attributes", read(repo.join(".gitattributes")));
    if let Some(home) = crate::settings::state::config_dir() {
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

    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    #[test]
    fn a_fetched_gravatar_is_kept_on_disk_for_the_next_run() {
        let root = tempfile::tempdir().unwrap();
        let first = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        assert!(first.cached_avatar("ada@example.com").is_none());
        first.store_avatar("Ada@Example.com ", PNG.to_vec());
        assert!(
            matches!(
                &*first.cached_avatar("ada@example.com").unwrap(),
                Gravatar::Picture(bytes) if bytes == &PNG
            ),
            "the same address in another case is the same author"
        );
        first.avatars.flush();

        // A new run: nothing in memory, so it is the disk that answers.
        let second = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        assert!(matches!(
            &*second.cached_avatar("ada@example.com").unwrap(),
            Gravatar::Picture(bytes) if bytes == &PNG
        ));
        assert!(second.cached_avatar("grace@example.com").is_none());
        second.store_no_avatar("grace@example.com");
        second.avatars.flush();
        let after = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let grace = after.cached_avatar("grace@example.com").unwrap();
        assert!(matches!(&*grace, Gravatar::Missing { .. }) && !grace.is_stale());

        second.clear();
        second.avatars.flush();
        let third = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        assert!(third.cached_avatar("ada@example.com").is_none(), "cleared");
    }

    #[test]
    fn the_limit_is_shared_so_one_cache_may_use_what_the_others_leave() {
        let root = tempfile::tempdir().unwrap();
        // Room for about twenty of these entries in all.
        let limit = 20 * 1100;
        let caches = Caches::on_disk(
            root.path().to_owned(),
            DiskSettings {
                enabled: true,
                max_bytes: limit,
            },
        );
        let picture = |n: u8| {
            let mut bytes = PNG.to_vec();
            bytes.extend(std::iter::repeat_n(n, 1000));
            bytes
        };
        // Only avatars are written, far more than a sixteenth of the limit.
        for n in 0..15u8 {
            caches.store_avatar(&format!("a{n}@example.com"), picture(n));
        }
        caches.avatars.flush();
        let usage = caches.usage();
        let avatars = usage.iter().find(|part| part.label == "Avatars").unwrap();
        assert!(avatars.bytes > limit / 16 * 4, "it isn't held to a share");
        assert!(
            caches.disk_size() <= limit,
            "the whole stays under the limit"
        );
        assert_eq!(usage[0].label, "Avatars", "largest first");
    }

    #[test]
    fn the_oldest_entries_of_any_cache_go_first_and_one_clear_empties_all() {
        let root = tempfile::tempdir().unwrap();
        let caches = Caches::on_disk(
            root.path().to_owned(),
            DiskSettings {
                enabled: true,
                max_bytes: 8 * 1100,
            },
        );
        let picture = |n: u8| {
            let mut bytes = PNG.to_vec();
            bytes.extend(std::iter::repeat_n(n, 1000));
            bytes
        };
        for n in 0..12u8 {
            caches.store_avatar(&format!("a{n}@example.com"), picture(n));
            caches.avatars.flush();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        assert!(caches.disk_size() <= 8 * 1100);
        let second = Caches::on_disk(
            root.path().to_owned(),
            DiskSettings {
                enabled: true,
                max_bytes: 8 * 1100,
            },
        );
        assert!(
            second.cached_avatar("a0@example.com").is_none(),
            "the oldest went"
        );
        assert!(
            second.cached_avatar("a11@example.com").is_some(),
            "the newest stayed"
        );
        second.clear();
        second.avatars.flush();
        assert_eq!(second.disk_size(), 0);
    }

    #[test]
    fn signatures_are_kept_with_the_commits_and_counted_as_one_part() {
        let root = tempfile::tempdir().unwrap();
        let repo = Repo::new();
        let oid: Arc<str> = repo.commit("one").into();
        let caches = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let first = caches
            .read_signatures(repo.path(), std::slice::from_ref(&oid))
            .unwrap();
        assert_eq!(first, vec![(oid.clone(), Signature::None)]);
        caches.signatures.flush();

        // A new run answers from the disk, so a repository that is gone isn't asked.
        let second = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let gone = std::path::Path::new("/nonexistent/repository");
        assert_eq!(
            second
                .read_signatures(gone, std::slice::from_ref(&oid))
                .unwrap(),
            first
        );

        let usage = second.usage();
        assert_eq!(usage.iter().filter(|u| u.label == "Commits").count(), 1);
        assert!(usage.iter().find(|u| u.label == "Commits").unwrap().bytes > 0);
        second.clear();
        second.signatures.flush();
        assert_eq!(second.disk_size(), 0);
    }

    #[test]
    fn a_damaged_cached_gravatar_is_not_used() {
        use crate::cache::Cacheable;
        use crate::cache::codec::{Reader, Writer};
        use crate::history::avatar::Gravatar;
        let decode = |bytes: &[u8]| {
            let mut out = Writer::new();
            out.bytes(bytes);
            let written = out.finish();
            let mut tagged = vec![0];
            tagged.extend_from_slice(&written);
            Gravatar::decode(&mut Reader::new(&tagged)).is_some()
        };
        assert!(decode(&PNG));
        assert!(!decode(b"<html>an error page</html>"));
        assert!(Gravatar::decode(&mut Reader::new(&[9, 9, 0, 0, 1])).is_none());
        assert!(
            Gravatar::decode(&mut Reader::new(&[1, 1, 2])).is_none(),
            "a cut-short date"
        );
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
        let patch = first.read_diff(repo.path(), "fp", &details, &file).unwrap();
        first.details.flush();
        first.diffs.flush();

        // A new process, and the repository is no longer needed to answer.
        let second = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let gone = Path::new("/nonexistent");
        assert_eq!(
            second.read_details(gone, "fp", &oid, &cancel).unwrap(),
            details
        );
        assert_eq!(
            second.read_diff(gone, "fp", &details, &file).unwrap(),
            patch
        );
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
