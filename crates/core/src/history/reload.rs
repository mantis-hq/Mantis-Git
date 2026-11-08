//! Loading history again after refs moved, without reading again what is already loaded.
//!
//! When every commit already loaded is still in history, which is so when refs only moved
//! forward (new commits, a fetch, a merge, a new branch), git lists just the commits the
//! new refs reach and the old ones didn't. None of those can come before a loaded commit,
//! so they go first, and every row is laid out again from scratch, which takes no git at
//! all. Anything else (a rewrite, a reset, a deleted branch) is an ordinary full load.
//!
//! Either way the events are those of a full load, so whoever shows them can't tell the
//! two apart, and older history keeps loading where the list stops.

use super::refs::{Refs, read_refs};
use super::{BATCH_SIZE, Commit, LoadEvent, begin, load_from, spawn, stream_commits};
use crate::ops::run_with_input;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What was on show: the refs it was loaded for, and the commits loaded for them, in the
/// order they came. Any start of a load is fine, since history comes children first.
#[derive(Clone, Debug, Default)]
pub struct Previous {
    pub refs: Arc<Refs>,
    pub commits: Vec<Commit>,
}

/// Which way a reload went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reloaded {
    /// Git listed only this many new commits.
    New(u32),
    Full,
}

/// Reloads on its own thread. The channel closes when loading is done, and dropping the
/// receiver cancels the load and stops `git`.
pub fn spawn_reload(
    repo: PathBuf,
    previous: Previous,
    limit: u32,
) -> async_channel::Receiver<LoadEvent> {
    spawn(move |emit| {
        reload(&repo, &previous, limit, emit);
    })
}

/// Reads refs again and shows history for them, passing each step to `emit` until it
/// returns false. A full load reads at most `limit` commits.
pub fn reload(
    repo: &Path,
    previous: &Previous,
    limit: u32,
    mut emit: impl FnMut(LoadEvent) -> bool,
) -> Reloaded {
    let refs = match read_refs(repo) {
        Ok(refs) => Arc::new(refs),
        Err(message) => {
            emit(LoadEvent::Failed(message));
            return Reloaded::Full;
        }
    };
    match new_commits(repo, previous, &refs, limit) {
        Some(new) => {
            let read = new.len() as u32;
            lay_out_again(refs, new, &previous.commits, &mut emit);
            Reloaded::New(read)
        }
        None => {
            load_from(repo, refs, limit, &mut emit);
            Reloaded::Full
        }
    }
}

/// The commits `refs` reach that the previous refs didn't, children first, or `None` when
/// only a full load will do: some commit already loaded has left history, more than
/// `limit` are new, or git couldn't say.
fn new_commits(repo: &Path, previous: &Previous, refs: &Refs, limit: u32) -> Option<Vec<Commit>> {
    let (old, tips) = (&previous.refs.tips, &refs.tips);
    if previous.commits.is_empty() || old.is_empty() || tips.is_empty() {
        return None;
    }
    // Each loaded commit is in the history of an old tip, so they all stay when every old
    // tip is still reachable: none is listed here.
    let input: String = old
        .iter()
        .map(|tip| format!("{tip}\n"))
        .chain(tips.iter().map(|tip| format!("^{tip}\n")))
        .collect();
    let lost = run_with_input(repo, &["rev-list", "--count", "--stdin"], input.as_bytes()).ok()?;
    if lost.trim() != "0" {
        return None;
    }
    let mut new = Vec::new();
    // One past the limit says there are too many.
    stream_commits(repo, tips, old, limit.saturating_add(1), |batch| {
        new.extend(batch);
        true
    })
    .ok()?;
    (new.len() <= limit as usize).then_some(new)
}

/// Lays out the new commits and then the loaded ones under fresh lanes for `refs`, and
/// says where older history goes on, if it does.
fn lay_out_again(
    refs: Arc<Refs>,
    new: Vec<Commit>,
    loaded: &[Commit],
    emit: &mut impl FnMut(LoadEvent) -> bool,
) {
    if !emit(LoadEvent::Refs(refs.clone())) {
        return;
    }
    let (mut more, mut working) = begin(&refs);
    let mut commits = new.into_iter().chain(loaded.iter().cloned()).peekable();
    while commits.peek().is_some() {
        let batch: Vec<Commit> = commits.by_ref().take(BATCH_SIZE).collect();
        if !emit(LoadEvent::Batch(more.place(working.take(), batch))) {
            return;
        }
    }
    // Lanes still waiting for a parent, or a tip not listed yet, mean there is more.
    if !more.starts().is_empty() {
        emit(LoadEvent::More(more));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Entry, load, load_more};
    use super::*;
    use crate::testing::Repo;

    /// Everything a load shows: its entries, and the rest of history paged in after.
    fn take_all(first: impl FnOnce(&mut dyn FnMut(LoadEvent) -> bool), repo: &Path) -> Shown {
        let mut shown = Shown::default();
        let mut more = None;
        first(&mut |event| shown.take(event, &mut more));
        while let Some(next) = more.take() {
            load_more(repo, next, 2, |event| shown.take(event, &mut more));
        }
        shown
    }

    #[derive(Default)]
    struct Shown {
        refs: Option<Arc<Refs>>,
        entries: Vec<Entry>,
    }

    impl Shown {
        fn take(&mut self, event: LoadEvent, more: &mut Option<super::super::More>) -> bool {
            match event {
                LoadEvent::Refs(refs) => self.refs = Some(refs),
                LoadEvent::Batch(batch) => self.entries.extend(batch),
                LoadEvent::More(next) => *more = Some(next),
                LoadEvent::Failed(message) => panic!("{message}"),
            }
            true
        }

        fn previous(&self) -> Previous {
            Previous {
                refs: self.refs.clone().unwrap(),
                commits: self
                    .entries
                    .iter()
                    .filter(|entry| !entry.working)
                    .map(|entry| entry.commit.clone())
                    .collect(),
            }
        }
    }

    /// A load of `limit` commits, as the view first shows a repository.
    fn first_page(repo: &Repo, limit: u32) -> Previous {
        let mut shown = Shown::default();
        load(repo.path(), limit, |event| shown.take(event, &mut None));
        shown.previous()
    }

    /// Reloads after `previous`, pages in the rest, and checks the result against a fresh
    /// load: the same commits once each, children before parents, and every lane joining
    /// as it does there.
    fn reload_matches_a_fresh_load(repo: &Repo, previous: &Previous, limit: u32) -> Reloaded {
        let mut how = None;
        let shown = take_all(
            |emit| how = Some(reload(repo.path(), previous, limit, emit)),
            repo.path(),
        );
        let fresh = take_all(|emit| load(repo.path(), u32::MAX >> 1, emit), repo.path());

        let joins = |entries: &[Entry]| {
            let mut joins: Vec<_> = entries
                .iter()
                .map(|e| (e.commit.oid.clone(), e.working, e.row.incoming.len()))
                .collect();
            joins.sort();
            joins
        };
        assert_eq!(joins(&shown.entries), joins(&fresh.entries));
        for (at, entry) in shown.entries.iter().enumerate() {
            for parent in &entry.commit.parents {
                assert!(
                    shown.entries[at..].iter().any(|e| e.commit.oid == *parent),
                    "{} comes before its child {}",
                    parent,
                    entry.commit.subject
                );
            }
        }
        how.unwrap()
    }

    fn history(repo: &Repo) {
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("side 1");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("main 1");
        repo.commit("main 2");
    }

    #[test]
    fn a_new_commit_is_the_only_one_read() {
        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 100);
        repo.commit("main 3");
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 100),
            Reloaded::New(1)
        );
    }

    #[test]
    fn a_merge_a_new_branch_and_a_fast_forward_read_only_whats_new() {
        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 100);
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
        repo.git(&["checkout", "-q", "-b", "later", "side"]);
        repo.commit("later 1");
        repo.git(&["checkout", "-q", "side"]);
        repo.git(&["merge", "-q", "--ff-only", "later"]);
        repo.git(&["checkout", "-q", "main"]);
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 100),
            Reloaded::New(2)
        );
    }

    #[test]
    fn nothing_new_lays_out_again_without_reading() {
        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 100);
        // Working changes alone move nothing, but they get their row.
        repo.write("loose", "x");
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 100),
            Reloaded::New(0)
        );
    }

    #[test]
    fn a_partial_load_keeps_loading_older_history_after() {
        let repo = Repo::new();
        history(&repo);
        for index in 0..5 {
            repo.commit(&format!("more {index}"));
        }
        let previous = first_page(&repo, 2);
        assert_eq!(previous.commits.len(), 2);
        repo.commit("new");
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 2),
            Reloaded::New(1)
        );
    }

    #[test]
    fn rewritten_or_deleted_history_loads_in_full() {
        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 100);
        repo.git(&[
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "-m",
            "main 2, amended",
        ]);
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 100),
            Reloaded::Full
        );

        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 100);
        repo.git(&["branch", "-q", "-D", "side"]);
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 100),
            Reloaded::Full
        );

        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 100);
        repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 100),
            Reloaded::Full
        );
    }

    #[test]
    fn more_new_commits_than_the_limit_load_in_full() {
        let repo = Repo::new();
        history(&repo);
        let previous = first_page(&repo, 2);
        for index in 0..3 {
            repo.commit(&format!("new {index}"));
        }
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &previous, 2),
            Reloaded::Full
        );
    }

    #[test]
    fn nothing_loaded_yet_loads_in_full() {
        let repo = Repo::new();
        history(&repo);
        assert_eq!(
            reload_matches_a_fresh_load(&repo, &Previous::default(), 100),
            Reloaded::Full
        );
    }
}
