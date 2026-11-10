//! The on-disk half of a cache: one file per entry in a folder, bounded by total size,
//! with the least recently used files going first.
//!
//! Every failure here is survivable. An entry that can't be read, is damaged, or was
//! written by another version of the app is a miss, and a write that fails costs only the
//! speed-up it would have given. Nothing here panics on what is in the folder.

use super::codec::{Reader, Writer};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

const MAGIC: &[u8; 4] = b"GCCE";
const ENTRY: &str = "entry";
const PARTIAL: &str = "tmp";
/// A partial write older than this was left by a crash.
const STALE_PARTIAL: Duration = Duration::from_secs(60);

/// FNV-1a. It is written out here, not taken from `std`, because the hash names files
/// and so must not change from one Rust version to the next.
pub fn hash64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The size limit shared by every cache folder under one root, so that the limit is for
/// the app's caches as a whole and room one doesn't use is there for the others. When the
/// folders together are over it, the least recently used entries of any of them go first.
pub struct Budget {
    root: PathBuf,
    max_bytes: AtomicU64,
    /// About how much all the folders hold. Writes add to it and a scan corrects it.
    used: AtomicU64,
    /// Held while trimming, so that two caches going over at once trim once.
    trimming: Mutex<()>,
}

impl Budget {
    /// Touches nothing yet: the folders are made by their first write.
    pub fn new(root: PathBuf, max_bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            root,
            max_bytes: AtomicU64::new(max_bytes),
            used: AtomicU64::new(0),
            trimming: Mutex::new(()),
        })
    }

    /// Where the cache called `name` keeps its entries.
    pub fn folder(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Changes the limit, and brings the folders under it if they are over.
    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
        self.scan();
        if self.used.load(Ordering::Relaxed) > max_bytes {
            self.evict();
        }
    }

    fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Counts everything under the root, and clears out partial writes a crash left behind.
    pub fn scan(&self) -> u64 {
        let total = self.files().iter().map(|file| file.size).sum();
        self.used.store(total, Ordering::Relaxed);
        total
    }

    /// Every entry of every cache folder.
    fn files(&self) -> Vec<Listed> {
        let Ok(root) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        root.flatten()
            .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
            .flat_map(|folder| list_folder(&folder.path()))
            .collect()
    }

    /// Drops the oldest entries, whichever cache they belong to, until the folders are at
    /// 90% of the limit, so that the next write doesn't have to do it again.
    fn evict(&self) {
        let _one_at_a_time = self.trimming.lock().unwrap_or_else(PoisonError::into_inner);
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|file| file.size).sum();
        files.sort_by_key(|file| file.modified);
        let target = self.max_bytes() / 10 * 9;
        for file in files {
            if total <= target {
                break;
            }
            if fs::remove_file(&file.path).is_ok() {
                total -= file.size;
            }
        }
        self.used.store(total, Ordering::Relaxed);
    }
}

pub struct DiskStore {
    dir: PathBuf,
    budget: Arc<Budget>,
    writes: AtomicU64,
}

impl DiskStore {
    /// A store alone under its own limit, with `dir`'s parent as the root of its budget.
    #[cfg(test)]
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        let root = dir.parent().unwrap_or(&dir).to_owned();
        Self::in_budget(dir, Budget::new(root, max_bytes))
    }

    /// A store that shares `budget` with the others under the same root. Touches nothing
    /// yet: the folder is made by the first write.
    pub fn in_budget(dir: PathBuf, budget: Arc<Budget>) -> Self {
        Self {
            dir,
            budget,
            writes: AtomicU64::new(0),
        }
    }

    /// Changes the limit this store shares with the others.
    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.budget.set_max_bytes(max_bytes);
    }

    fn max_bytes(&self) -> u64 {
        self.budget.max_bytes()
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let name = format!("{:016x}", hash64(key.as_bytes()));
        self.dir.join(name).with_extension(ENTRY)
    }

    /// The payload stored for `key` under `format`, or `None` for anything else.
    pub fn read(&self, key: &str, format: u32) -> Option<Vec<u8>> {
        let path = self.path_for(key);
        let bytes = fs::read(&path).ok()?;
        match parse(&bytes, key, format) {
            Parsed::Payload(range) => {
                touch(&path);
                Some(bytes[range].to_vec())
            }
            // Another key with the same file name: not ours to remove.
            Parsed::OtherKey => None,
            Parsed::Unusable => {
                let _ = fs::remove_file(&path);
                None
            }
        }
    }

    /// Stores `payload` for `key`, replacing what was there in one step so that a reader
    /// never sees half of it.
    pub fn write(&self, key: &str, format: u32, payload: &[u8]) -> io::Result<()> {
        let mut head = Writer::new();
        head.str(key);
        let key_part = head.finish();
        let mut file = Vec::with_capacity(payload.len() + key_part.len() + 24);
        file.extend_from_slice(MAGIC);
        file.extend_from_slice(&format.to_le_bytes());
        file.extend_from_slice(&key_part);
        file.extend_from_slice(&hash64(payload).to_le_bytes());
        file.extend_from_slice(payload);
        let size = file.len() as u64;
        if size > self.max_bytes() {
            return Ok(());
        }

        fs::create_dir_all(&self.dir)?;
        let path = self.path_for(key);
        let number = self.writes.fetch_add(1, Ordering::Relaxed);
        let partial = path.with_extension(format!("{number}.{PARTIAL}"));
        let result = fs::write(&partial, &file).and_then(|()| fs::rename(&partial, &path));
        if result.is_err() {
            let _ = fs::remove_file(&partial);
            return result;
        }
        if self.budget.used.fetch_add(size, Ordering::Relaxed) + size > self.max_bytes() {
            self.budget.evict();
        }
        Ok(())
    }

    /// Counts what is in this store's folder, and corrects the shared count.
    pub fn scan(&self) -> u64 {
        self.budget.scan();
        list_folder(&self.dir).iter().map(|file| file.size).sum()
    }

    /// Removes every entry of this store.
    pub fn clear(&self) -> io::Result<()> {
        for file in list_folder(&self.dir) {
            fs::remove_file(file.path)?;
        }
        self.budget.scan();
        Ok(())
    }
}

/// The entries in one cache folder. Partial writes that went stale are removed on the way.
fn list_folder(dir: &Path) -> Vec<Listed> {
    let Ok(items) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut found = Vec::new();
    for item in items.flatten() {
        let path = item.path();
        let Ok(meta) = item.metadata() else { continue };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ENTRY) => found.push(Listed {
                path,
                size: meta.len(),
                modified,
            }),
            Some(PARTIAL) => {
                let age = now.duration_since(modified).unwrap_or_default();
                if age > STALE_PARTIAL {
                    let _ = fs::remove_file(&path);
                }
            }
            _ => {}
        }
    }
    found
}

struct Listed {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

enum Parsed {
    Payload(std::ops::Range<usize>),
    OtherKey,
    Unusable,
}

fn parse(bytes: &[u8], key: &str, format: u32) -> Parsed {
    let Some(rest) = bytes.strip_prefix(MAGIC) else {
        return Parsed::Unusable;
    };
    let mut input = Reader::new(rest);
    let (Some(stored_format), Some(stored_key), Some(checksum)) =
        (input.u32(), input.str(), input.u64())
    else {
        return Parsed::Unusable;
    };
    if stored_format != format {
        return Parsed::Unusable;
    }
    if stored_key != key {
        return Parsed::OtherKey;
    }
    let start = bytes.len() - input.remaining();
    if hash64(&bytes[start..]) != checksum {
        return Parsed::Unusable;
    }
    Parsed::Payload(start..bytes.len())
}

/// Marks an entry as just used, so that eviction takes older ones first.
fn touch(path: &Path) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(max: u64) -> (tempfile::TempDir, DiskStore) {
        let root = tempfile::tempdir().unwrap();
        let store = DiskStore::new(root.path().join("cache"), max);
        (root, store)
    }

    #[test]
    fn an_entry_reads_back_and_a_missing_one_does_not() {
        let (_root, store) = store(1 << 20);
        assert_eq!(store.read("a", 1), None);
        store.write("a", 1, b"hello").unwrap();
        assert_eq!(store.read("a", 1), Some(b"hello".to_vec()));
        assert_eq!(store.read("b", 1), None);
    }

    #[test]
    fn an_entry_of_another_format_is_a_miss_and_is_cleared() {
        let (_root, store) = store(1 << 20);
        store.write("a", 1, b"old").unwrap();
        assert_eq!(store.read("a", 2), None);
        assert_eq!(store.read("a", 1), None);
    }

    #[test]
    fn a_damaged_entry_is_a_miss_not_a_panic() {
        let (_root, store) = store(1 << 20);
        store.write("a", 1, b"payload").unwrap();
        let path = store.path_for("a");
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        assert_eq!(store.read("a", 1), None);
        assert!(!path.exists());

        store.write("b", 1, b"payload").unwrap();
        let path = store.path_for("b");
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert_eq!(store.read("b", 1), None);
        fs::write(&path, b"junk").unwrap();
        assert_eq!(store.read("b", 1), None);
    }

    #[test]
    fn the_oldest_entries_go_when_the_folder_is_full() {
        let (_root, store) = store(1000);
        let payload = [7u8; 300];
        for key in ["a", "b", "c"] {
            store.write(key, 1, &payload).unwrap();
            // File times are coarse on some systems, so make the order unmistakable.
            let past = SystemTime::now() - Duration::from_secs(100 - key.as_bytes()[0] as u64);
            let file = fs::OpenOptions::new()
                .write(true)
                .open(store.path_for(key))
                .unwrap();
            file.set_modified(past).unwrap();
        }
        // Reading `a` makes it the newest.
        assert!(store.read("a", 1).is_some());
        store.write("d", 1, &payload).unwrap();
        assert!(store.read("b", 1).is_none());
        assert!(store.read("a", 1).is_some());
        assert!(store.read("d", 1).is_some());
    }

    #[test]
    fn something_bigger_than_the_whole_folder_is_not_written() {
        let (_root, store) = store(100);
        store.write("a", 1, &[0u8; 500]).unwrap();
        assert_eq!(store.read("a", 1), None);
    }

    #[test]
    fn clear_removes_entries_and_scan_counts_what_is_left() {
        let (_root, store) = store(1 << 20);
        store.write("a", 1, b"one").unwrap();
        store.write("b", 1, b"two").unwrap();
        assert!(store.scan() > 0);
        store.clear().unwrap();
        assert_eq!(store.scan(), 0);
        assert_eq!(store.read("a", 1), None);
    }

    #[test]
    fn the_hash_that_names_files_does_not_change() {
        assert_eq!(hash64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
