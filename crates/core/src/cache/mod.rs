//! A cache for things that are slow to work out and never change, such as what a commit
//! changed (a commit's hash covers its contents). It has two layers: an in-memory LRU
//! bounded by weight, in front of one file per entry on disk, bounded by total size.
//!
//! Nothing here knows what it caches. A type opts in by implementing [`Cacheable`], and the
//! caller owns the key, which must name everything the value depends on.
//!
//! # What blocks
//!
//! - [`Cache::peek`] only looks in memory. It is safe on the UI thread.
//! - [`Cache::insert`] stores in memory and hands the disk write to a background thread, so
//!   it never waits on the disk either.
//! - [`Cache::get`], [`Cache::get_or_load`] and [`Cache::disk_size`] read the disk, so call
//!   them off the UI thread, as all of core's blocking calls are.
//!
//! The disk layer is a speed-up and nothing more. A damaged, out-of-date or unwritable
//! entry is a miss, never an error.

pub mod codec;
mod disk;
pub mod keys;
mod lru;

pub use codec::{Reader, Writer};
pub use disk::hash64;

use disk::DiskStore;
use lru::Lru;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

/// A value a [`Cache`] can hold, and write to disk and read back.
pub trait Cacheable: Send + Sync + Sized + 'static {
    /// Names the layout of [`Cacheable::encode`]. Change it whenever the layout does, so
    /// that entries written by an older version are skipped instead of misread.
    const FORMAT: u32;

    /// About how many bytes this takes in memory, which bounds the in-memory layer.
    fn weight(&self) -> usize;

    fn encode(&self, out: &mut Writer);

    /// `None` for anything that isn't a whole, valid value.
    fn decode(input: &mut Reader) -> Option<Self>;
}

/// What differs between one cache and the next.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Names the cache's folder on disk.
    pub name: &'static str,
    /// The in-memory layer's size, in the units of [`Cacheable::weight`].
    pub memory_bytes: usize,
}

/// Whether the disk layer is used, and how big it may grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskSettings {
    pub enabled: bool,
    pub max_bytes: u64,
}

pub const DEFAULT_DISK_MB: u32 = 256;

impl Default for DiskSettings {
    fn default() -> Self {
        Self::from_mb(true, DEFAULT_DISK_MB)
    }
}

impl DiskSettings {
    pub fn from_mb(enabled: bool, megabytes: u32) -> Self {
        Self {
            enabled,
            max_bytes: u64::from(megabytes) * 1024 * 1024,
        }
    }
}

type Job = Box<dyn FnOnce(&DiskStore) + Send>;

struct Disk {
    store: Arc<DiskStore>,
    /// Work for the thread that owns disk writes. It stops when the cache is dropped.
    jobs: Sender<Job>,
    enabled: AtomicBool,
}

struct Inner<V> {
    memory: Mutex<Lru<V>>,
    disk: Option<Disk>,
}

/// A cache. Clones share the same entries, so hand one to each thread that needs it.
pub struct Cache<V> {
    inner: Arc<Inner<V>>,
}

impl<V> Clone for Cache<V> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<V: Cacheable> Cache<V> {
    /// A cache that keeps nothing on disk, as tests and systems without a cache folder
    /// want.
    pub fn memory_only(config: Config) -> Self {
        Self {
            inner: Arc::new(Inner {
                memory: Mutex::new(Lru::new(config.memory_bytes)),
                disk: None,
            }),
        }
    }

    /// A cache that also keeps entries under `root/<name>`. Nothing is read or written
    /// until it is asked to, and writes happen on a thread of their own.
    pub fn new(config: Config, root: PathBuf, disk: DiskSettings) -> Self {
        let store = Arc::new(DiskStore::new(root.join(config.name), disk.max_bytes));
        let (jobs, queue) = mpsc::channel::<Job>();
        let worker = store.clone();
        thread::Builder::new()
            .name(format!("cache-{}", config.name))
            .spawn(move || {
                worker.scan();
                for job in queue {
                    job(&worker);
                }
            })
            .expect("a thread for cache writes");
        Self {
            inner: Arc::new(Inner {
                memory: Mutex::new(Lru::new(config.memory_bytes)),
                disk: Some(Disk {
                    store,
                    jobs,
                    enabled: AtomicBool::new(disk.enabled),
                }),
            }),
        }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Lru<V>> {
        self.inner
            .memory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The disk layer, if there is one and it is on.
    fn disk(&self) -> Option<&Disk> {
        self.inner
            .disk
            .as_ref()
            .filter(|disk| disk.enabled.load(Ordering::Relaxed))
    }

    /// Turns the disk layer on or off and sets its size limit. Off keeps what is on disk
    /// but neither reads nor writes it. A smaller limit trims the folder in the background.
    pub fn set_disk(&self, settings: DiskSettings) {
        let Some(disk) = &self.inner.disk else {
            return;
        };
        disk.enabled.store(settings.enabled, Ordering::Relaxed);
        let _ = disk.jobs.send(Box::new(move |store| {
            store.set_max_bytes(settings.max_bytes)
        }));
    }

    /// The value for `key` if it is in memory. Never touches the disk.
    pub fn peek(&self, key: &str) -> Option<Arc<V>> {
        self.memory().get(key)
    }

    /// The value for `key` from memory, or failing that from disk, which then keeps it in
    /// memory too. Blocks on the disk.
    pub fn get(&self, key: &str) -> Option<Arc<V>> {
        if let Some(value) = self.peek(key) {
            return Some(value);
        }
        let payload = self.disk()?.store.read(key, V::FORMAT)?;
        let mut input = Reader::new(&payload);
        let value = V::decode(&mut input).filter(|_| input.is_empty())?;
        let value = Arc::new(value);
        self.memory().insert(key, value.clone(), value.weight());
        Some(value)
    }

    /// Stores `value` in memory now and on disk soon, and returns it shared.
    pub fn insert(&self, key: &str, value: V) -> Arc<V> {
        let value = Arc::new(value);
        self.memory().insert(key, value.clone(), value.weight());
        if let Some(disk) = self.disk() {
            let (key, value) = (key.to_owned(), value.clone());
            let _ = disk.jobs.send(Box::new(move |store| {
                let mut out = Writer::new();
                value.encode(&mut out);
                // A failed write only loses the speed-up.
                let _ = store.write(&key, V::FORMAT, &out.finish());
            }));
        }
        value
    }

    /// The cached value, or else whatever `load` makes, which is then cached. An `Err` is
    /// passed on and cached as nothing, so a cancelled or failed load is tried again next
    /// time. Blocks on the disk and on `load`.
    ///
    /// Two callers asking for the same missing key at once both load it. That wastes work
    /// but is harmless, since both would make the same value.
    pub fn get_or_load<E>(
        &self,
        key: &str,
        load: impl FnOnce() -> Result<V, E>,
    ) -> Result<Arc<V>, E> {
        match self.get(key) {
            Some(value) => Ok(value),
            None => load().map(|value| self.insert(key, value)),
        }
    }

    /// Forgets everything in memory, and on disk soon.
    pub fn clear(&self) {
        self.memory().clear();
        if let Some(disk) = &self.inner.disk {
            // Sent even while the disk layer is off, so that "clear" means everything.
            let _ = disk.jobs.send(Box::new(|store| {
                let _ = store.clear();
            }));
        }
    }

    /// How many bytes the cache holds on disk. Blocks, and waits behind writes.
    pub fn disk_size(&self) -> u64 {
        let Some(disk) = &self.inner.disk else {
            return 0;
        };
        let (reply, answer) = mpsc::channel();
        let sent = disk.jobs.send(Box::new(move |store| {
            let _ = reply.send(store.scan());
        }));
        sent.ok().and_then(|()| answer.recv().ok()).unwrap_or(0)
    }

    /// Waits until the disk writes asked for so far have happened. For tests.
    pub fn flush(&self) {
        self.disk_size();
    }
}

/// Where the app keeps caches on this system: `$XDG_CACHE_HOME` (or `~/.cache`) on Linux,
/// `~/Library/Caches` on macOS and `%LOCALAPPDATA%` on Windows, then `gitclient`.
pub fn default_dir() -> Option<PathBuf> {
    Some(cache_home(|name| std::env::var_os(name))?.join("gitclient"))
}

fn cache_home(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| {
        env(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        set("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        set("HOME").map(|home| home.join("Library").join("Caches"))
    } else {
        // The XDG spec says a relative XDG_CACHE_HOME is to be ignored.
        set("XDG_CACHE_HOME")
            .filter(|path| path.is_absolute())
            .or_else(|| set("HOME").map(|home| home.join(".cache")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Debug, PartialEq, Eq)]
    struct Note(String);

    impl Cacheable for Note {
        const FORMAT: u32 = 1;
        fn weight(&self) -> usize {
            self.0.len()
        }
        fn encode(&self, out: &mut Writer) {
            out.str(&self.0);
        }
        fn decode(input: &mut Reader) -> Option<Self> {
            Some(Self(input.str()?.to_owned()))
        }
    }

    const CONFIG: Config = Config {
        name: "notes",
        memory_bytes: 1000,
    };

    fn on_disk(root: &tempfile::TempDir) -> Cache<Note> {
        Cache::new(CONFIG, root.path().to_owned(), DiskSettings::default())
    }

    #[test]
    fn a_value_survives_a_restart_through_the_disk() {
        let root = tempfile::tempdir().unwrap();
        let first = on_disk(&root);
        first.insert("k", Note("hello".into()));
        first.flush();

        let second = on_disk(&root);
        assert!(second.peek("k").is_none(), "memory starts empty");
        assert_eq!(*second.get("k").unwrap(), Note("hello".into()));
        assert!(second.peek("k").is_some(), "a disk hit is kept in memory");
    }

    #[test]
    fn load_runs_once_and_a_failed_load_is_not_remembered() {
        let cache = Cache::<Note>::memory_only(CONFIG);
        let calls = AtomicUsize::new(0);
        let load = || {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, String>(Note("v".into()))
        };
        cache.get_or_load("k", load).unwrap();
        cache.get_or_load("k", load).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let failed: Result<_, String> = cache.get_or_load("other", || Err("nope".to_owned()));
        assert_eq!(failed.unwrap_err(), "nope");
        assert!(cache.peek("other").is_none());
    }

    #[test]
    fn a_value_of_another_format_on_disk_is_ignored() {
        struct Newer(String);
        impl Cacheable for Newer {
            const FORMAT: u32 = 2;
            fn weight(&self) -> usize {
                1
            }
            fn encode(&self, out: &mut Writer) {
                out.str(&self.0);
            }
            fn decode(input: &mut Reader) -> Option<Self> {
                Some(Self(input.str()?.to_owned()))
            }
        }
        let root = tempfile::tempdir().unwrap();
        let old = on_disk(&root);
        old.insert("k", Note("v".into()));
        old.flush();
        let newer = Cache::<Newer>::new(CONFIG, root.path().to_owned(), DiskSettings::default());
        assert!(newer.get("k").is_none());
    }

    #[test]
    fn a_value_with_trailing_bytes_is_refused() {
        struct Lax;
        impl Cacheable for Lax {
            const FORMAT: u32 = 1;
            fn weight(&self) -> usize {
                1
            }
            fn encode(&self, out: &mut Writer) {
                out.u8(1);
                out.u8(2);
            }
            fn decode(input: &mut Reader) -> Option<Self> {
                input.u8().map(|_| Lax)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let cache = Cache::<Lax>::new(CONFIG, root.path().to_owned(), DiskSettings::default());
        cache.insert("k", Lax);
        cache.flush();
        let fresh = Cache::<Lax>::new(CONFIG, root.path().to_owned(), DiskSettings::default());
        assert!(fresh.get("k").is_none());
    }

    #[test]
    fn with_the_disk_off_nothing_is_read_or_written() {
        let root = tempfile::tempdir().unwrap();
        let off = DiskSettings {
            enabled: false,
            ..DiskSettings::default()
        };
        let cache = Cache::<Note>::new(CONFIG, root.path().to_owned(), off);
        cache.insert("k", Note("v".into()));
        cache.flush();
        assert_eq!(cache.disk_size(), 0);

        // Turned on, new inserts reach the disk.
        cache.set_disk(DiskSettings::default());
        cache.insert("k2", Note("v".into()));
        cache.flush();
        assert!(cache.disk_size() > 0);
    }

    #[test]
    fn clear_empties_memory_and_disk_and_a_smaller_limit_trims() {
        let root = tempfile::tempdir().unwrap();
        let cache = on_disk(&root);
        for index in 0..20 {
            cache.insert(&format!("k{index}"), Note("x".repeat(100)));
        }
        cache.flush();
        let full = cache.disk_size();
        assert!(full > 2000);

        cache.set_disk(DiskSettings {
            enabled: true,
            max_bytes: full / 2,
        });
        assert!(cache.disk_size() <= full / 2);

        cache.clear();
        assert_eq!(cache.disk_size(), 0);
        assert!(cache.peek("k0").is_none());
    }

    #[test]
    fn the_cache_folder_follows_xdg() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        if cfg!(all(unix, not(target_os = "macos"))) {
            let got = cache_home(env(&[("XDG_CACHE_HOME", "/x/cache"), ("HOME", "/h")]));
            assert_eq!(got, Some(PathBuf::from("/x/cache")));
            let got = cache_home(env(&[("XDG_CACHE_HOME", "relative"), ("HOME", "/h")]));
            assert_eq!(got, Some(PathBuf::from("/h/.cache")));
            assert_eq!(cache_home(env(&[("HOME", "/h")])), Some("/h/.cache".into()));
            assert_eq!(cache_home(env(&[])), None);
        }
    }
}
