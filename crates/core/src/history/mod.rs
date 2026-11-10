//! A repository's history, and how the graph shows it: commits read through the `git`
//! executable and laid out in lanes as they stream in, the refs that point at them, and
//! which commits and branches are picked.
//!
//! Going through the CLI keeps this working with everything the user's git understands
//! (reftables, SHA-256, partial clones, the commit-graph cache) without a libgit2 build.

pub mod avatar;
pub mod columns;
pub mod lanes;
pub mod picks;
pub mod refs;
pub mod reload;
pub mod search;
pub mod selection;
pub mod spotlight;

use crate::ops::{Process, git, stderr_message};
use crate::text::count;
use lanes::{GraphBuilder, Row};
use refs::{Refs, StashGraph, read_refs};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

const BATCH_SIZE: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: Arc<str>,
    pub parents: Box<[Arc<str>]>,
    pub author: Arc<str>,
    /// The author's address, which the avatar is looked up by. Empty when unknown.
    pub email: Arc<str>,
    /// Author date, seconds since the Unix epoch.
    pub time: i64,
    pub subject: Arc<str>,
}

impl Commit {
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// Splits off a Conventional Commit prefix, which is drawn quieter than the description.
pub fn split_subject(subject: &str) -> (&str, &str) {
    let Some((kind, _)) = subject.split_once(": ") else {
        return ("", subject);
    };
    let word = kind.split('(').next().unwrap_or(kind).trim_end_matches('!');
    let scope_closes = !kind.contains('(') || kind.trim_end_matches('!').ends_with(')');
    let is_word = !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase());
    if is_word && scope_closes {
        subject.split_at(kind.len() + 2)
    } else {
        ("", subject)
    }
}

/// What can be done to a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitActions {
    /// The branch checking out switches to, or `None` for the bare commit.
    pub branch: Option<String>,
    pub can_check_out: bool,
    pub can_cherry_pick: bool,
    pub is_merge: bool,
}

impl CommitActions {
    pub fn check_out_label(&self) -> String {
        match &self.branch {
            Some(branch) => format!("Check out {branch}"),
            None => "Check out".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub commit: Commit,
    pub row: Row,
    /// This row is the working changes, not a commit, and `commit` only fills in its text.
    pub working: bool,
}

#[derive(Clone, Debug)]
pub enum LoadEvent {
    Refs(Arc<Refs>),
    Batch(Vec<Entry>),
    /// The load stopped at its limit, and history may go on past it.
    More(Box<More>),
    Failed(String),
}

/// Where a load that stopped at its limit left off, so the history below can be read
/// later and laid out in the same lanes.
///
/// History comes children first, so what is not read yet is exactly the history of the
/// parents the open lanes wait for and of the tips not listed yet. The next read starts
/// there, and git walks only the new commits.
#[derive(Clone)]
pub struct More {
    builder: GraphBuilder,
    /// Branch, tag and HEAD commits that have not been listed yet.
    unseen_tips: HashSet<Arc<str>>,
    stashes: StashGraph,
}

impl More {
    /// Where the next read starts, in a stable order.
    fn starts(&self) -> Vec<Arc<str>> {
        let mut starts: Vec<Arc<str>> = self
            .builder
            .awaited()
            .chain(&self.unseen_tips)
            .cloned()
            .collect();
        starts.sort_unstable();
        starts.dedup();
        starts
    }

    /// Lays out `commits` under everything laid out so far, after `working` if given.
    fn place(&mut self, working: Option<Entry>, commits: Vec<Commit>) -> Vec<Entry> {
        let commits = self.stashes.tidy(commits);
        for commit in &commits {
            self.unseen_tips.remove(&commit.oid);
        }
        working
            .into_iter()
            .chain(commits.into_iter().map(|commit| Entry {
                row: self.builder.push(&commit.oid, &commit.parents),
                commit,
                working: false,
            }))
            .collect()
    }
}

impl std::fmt::Debug for More {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("More")
            .field("starts", &self.starts())
            .finish()
    }
}

/// A fresh layout for `refs`: lanes planned from them, and the working changes row to go
/// first when there are any.
fn begin(refs: &Refs) -> (More, Option<Entry>) {
    let mut builder = GraphBuilder::with_plan(refs.plan.clone());
    let files = refs.status.changed;
    let working = (files > 0).then(|| working_entry(files, &mut builder));
    let more = More {
        builder,
        unseen_tips: refs.tips.iter().cloned().collect(),
        stashes: refs.stash_graph.clone(),
    };
    (more, working)
}

/// What the band under the header says when older history could not be read.
pub fn more_failed_message(message: &str) -> String {
    format!("Could not load older commits: {message}")
}

/// Reads up to `limit` more commits where an earlier load stopped, on its own thread. It
/// sends no refs.
pub fn spawn_load_more(
    repo: PathBuf,
    more: More,
    limit: u32,
) -> async_channel::Receiver<LoadEvent> {
    spawn(move |emit| load_more(&repo, more, limit, emit))
}

/// Runs a load on its own thread. The channel closes when loading is done, and dropping
/// the receiver cancels the load and stops `git`.
fn spawn(
    work: impl FnOnce(&mut dyn FnMut(LoadEvent) -> bool) + Send + 'static,
) -> async_channel::Receiver<LoadEvent> {
    let (sender, receiver) = async_channel::bounded(4);
    thread::Builder::new()
        .name("git-load".into())
        .spawn(move || work(&mut |event| sender.send_blocking(event).is_ok()))
        .expect("failed to spawn the git loader thread");
    receiver
}

/// Reads refs and history, passing each step to `emit` until it returns false.
/// Reads at most `limit` commits, and ends with `More` if there may be others.
pub fn load(repo: &Path, limit: u32, mut emit: impl FnMut(LoadEvent) -> bool) {
    match read_refs(repo) {
        Ok(refs) => load_from(repo, Arc::new(refs), limit, &mut emit),
        Err(message) => {
            emit(LoadEvent::Failed(message));
        }
    }
}

/// Loads history for refs already read, as `load` does once it has them.
fn load_from(repo: &Path, refs: Arc<Refs>, limit: u32, emit: &mut impl FnMut(LoadEvent) -> bool) {
    if !emit(LoadEvent::Refs(refs.clone())) {
        return;
    }
    let (more, working) = begin(&refs);
    if refs.tips.is_empty() {
        // No commits to read, so the working changes are all there is.
        if let Some(working) = working {
            emit(LoadEvent::Batch(vec![working]));
        }
        return;
    }
    read_more(repo, &refs.tips, more, limit, working, emit);
}

/// Reads up to `limit` commits after the ones `more` has read, passing each step to
/// `emit` until it returns false, and ends with `More` again if there may be others.
pub fn load_more(repo: &Path, more: More, limit: u32, mut emit: impl FnMut(LoadEvent) -> bool) {
    let starts = more.starts();
    read_more(repo, &starts, more, limit, None, &mut emit);
}

fn read_more(
    repo: &Path,
    starts: &[Arc<str>],
    mut more: More,
    limit: u32,
    mut working: Option<Entry>,
    emit: &mut impl FnMut(LoadEvent) -> bool,
) {
    let mut stopped = false;
    let result = stream_commits(repo, starts, &[], limit, |commits| {
        stopped = !emit(LoadEvent::Batch(more.place(working.take(), commits)));
        !stopped
    });
    match result {
        Err(message) => {
            emit(LoadEvent::Failed(message));
        }
        // A full page may have more under it. If it was exactly the last, the next read
        // finds nothing and says no more.
        Ok(read) if !stopped && limit > 0 && read == limit => {
            emit(LoadEvent::More(Box::new(more)));
        }
        Ok(_) => {}
    }
}

fn working_entry(files: u32, builder: &mut GraphBuilder) -> Entry {
    Entry {
        row: builder.working_row(),
        commit: Commit {
            oid: "".into(),
            parents: Box::default(),
            author: "".into(),
            email: "".into(),
            time: 0,
            subject: format!("Working changes in {}", count(files, "file")).into(),
        },
        working: true,
    }
}

/// Streams up to `limit` commits reachable from `tips` and not from `exclude`, and says how
/// many it read.
fn stream_commits(
    repo: &Path,
    tips: &[Arc<str>],
    exclude: &[Arc<str>],
    limit: u32,
    mut sink: impl FnMut(Vec<Commit>) -> bool,
) -> Result<u32, String> {
    if tips.is_empty() {
        return Ok(0);
    }

    let mut command = git(repo);
    command.args([
        "log",
        "--stdin",
        &format!("--max-count={}", limit.min(i32::MAX as u32)),
        "--date-order",
        "--encoding=UTF-8",
        "--no-color",
        // log.showSignature would otherwise mix gpg output into the records.
        "--no-show-signature",
        "--format=%H%x00%P%x00%aN%x00%aE%x00%at%x00%s",
    ]);
    // Dropping the process, as every early return below does, stops git.
    let mut process = Process::spawn(&mut command, true)?;

    let mut stdin = process.stdin.take().expect("stdin is piped");
    let stdout = process.stdout.take().expect("stdout is piped");

    // git reads all of stdin before it writes anything, so this can't deadlock. If git
    // quits early the write fails, and its exit status is what gets reported below.
    // `^` rather than a `--not` line, which `--stdin` only understands from git 2.42.
    let _ = tips
        .iter()
        .map(|tip| (tip, ""))
        .chain(exclude.iter().map(|tip| (tip, "^")))
        .try_for_each(|(tip, mark)| writeln!(stdin, "{mark}{tip}"));
    drop(stdin);

    let mut reader = BufReader::with_capacity(1 << 16, stdout);
    let mut line = Vec::new();
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut total = 0;
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read from git: {error}"))?;
        if read == 0 {
            break;
        }
        batch.push(parse_commit(&line)?);
        if batch.len() == BATCH_SIZE {
            total += batch.len();
            // The records are not worth printing, so the log shows progress instead.
            process.note(&format!("{} read", count(total as u32, "commit")));
            if !sink(std::mem::replace(
                &mut batch,
                Vec::with_capacity(BATCH_SIZE),
            )) {
                return Ok(total as u32);
            }
        }
    }
    total += batch.len();
    if !batch.is_empty() && !sink(batch) {
        return Ok(total as u32);
    }
    process.note(&format!("{} read", count(total as u32, "commit")));

    match process.finish()? {
        (true, _) => Ok(total as u32),
        (false, stderr) => Err(stderr_message(&stderr)),
    }
}

fn parse_commit(line: &[u8]) -> Result<Commit, String> {
    let line = String::from_utf8_lossy(line);
    let line = line.strip_suffix('\n').unwrap_or(&line);
    let fields: Vec<&str> = line.split('\0').collect();
    let [oid, parents, author, email, time, subject] = fields[..] else {
        return Err(format!("unexpected output from git log: {line:?}"));
    };
    Ok(Commit {
        oid: oid.into(),
        parents: parents.split_ascii_whitespace().map(Arc::from).collect(),
        author: author.into(),
        email: email.into(),
        time: time.parse().unwrap_or(0),
        subject: subject.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::lanes::LaneColor;
    use super::refs::*;
    use super::*;
    use crate::testing::{AUTHOR, Repo};
    use std::collections::HashMap;
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    #[test]
    fn loading_stops_at_the_commit_limit() {
        let repo = Repo::new();
        for name in ["a", "b", "c"] {
            repo.commit(name);
        }
        let mut subjects = Vec::new();
        load(repo.path(), 2, |event| {
            if let LoadEvent::Batch(batch) = event {
                subjects.extend(batch.into_iter().map(|e| e.commit.subject));
            }
            true
        });
        assert_eq!(subjects.len(), 2);
    }

    /// Loads a page, then the rest a page at a time, as scrolling down does.
    fn load_in_pages(repo: &Path, page: u32) -> (Vec<Entry>, usize) {
        let (mut entries, mut reads) = (Vec::new(), 1);
        let mut take = |more: &mut Option<More>, event| {
            match event {
                LoadEvent::Batch(batch) => entries.extend(batch),
                LoadEvent::More(next) => *more = Some(*next),
                LoadEvent::Refs(_) => {}
                LoadEvent::Failed(message) => panic!("{message}"),
            }
            true
        };
        let mut more = None;
        load(repo, page, |event| take(&mut more, event));
        while let Some(next) = more.take() {
            reads += 1;
            load_more(repo, next, page, |event| take(&mut more, event));
        }
        (entries, reads)
    }

    #[test]
    fn history_past_the_limit_loads_a_page_at_a_time_in_the_same_lanes() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("side 1");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("main 1");
        repo.git(&["checkout", "-q", "side"]);
        repo.commit("side 2");
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
        repo.commit("main 2");
        repo.git(&["checkout", "-q", "-b", "later", "HEAD~1"]);
        repo.commit("later 1");
        repo.git(&["checkout", "-q", "main"]);
        std::fs::write(repo.path().join("loose"), "x").unwrap();

        // A branch nothing else leads to, which a page can end before.
        repo.git(&["branch", "old", "HEAD~3"]);
        repo.git(&["checkout", "-q", "old"]);
        repo.commit("old 1");
        repo.git(&["checkout", "-q", "main"]);

        // A page starts where the last stopped, so git may order what is left differently
        // from one long read. What must hold: every commit once, children before parents,
        // and every line joined as it is in one read.
        let (_, all, _) = load_all(repo.path());
        let joins = |entries: &[Entry]| -> Vec<(Arc<str>, bool, usize)> {
            let mut joins: Vec<_> = entries
                .iter()
                .map(|e| (e.commit.oid.clone(), e.working, e.row.incoming.len()))
                .collect();
            joins.sort();
            joins
        };
        for page in 1..=4 {
            let (paged, _) = load_in_pages(repo.path(), page);
            assert_eq!(joins(&paged), joins(&all), "pages of {page}");
            for (at, entry) in paged.iter().enumerate() {
                for parent in &entry.commit.parents {
                    let below = paged[at..].iter().any(|e| e.commit.oid == *parent);
                    assert!(below, "pages of {page}: a parent came before its child");
                }
            }
        }
        // Eight commits in pages of two: four full pages, then a read that finds no more.
        assert_eq!(load_in_pages(repo.path(), 2).1, 5);
    }

    #[test]
    fn a_page_that_ends_history_exactly_finds_nothing_more() {
        let repo = Repo::new();
        repo.commit("a");
        repo.commit("b");
        let (entries, reads) = load_in_pages(repo.path(), 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(reads, 2);
    }

    fn load_all(repo: &Path) -> (Option<Arc<Refs>>, Vec<Entry>, Option<String>) {
        let (mut refs, mut entries, mut error) = (None, Vec::new(), None);
        load(repo, i32::MAX as u32, |event| {
            match event {
                LoadEvent::Refs(loaded) => refs = Some(loaded),
                LoadEvent::Batch(batch) => entries.extend(batch),
                LoadEvent::Failed(message) => error = Some(message),
                LoadEvent::More(_) => {}
            }
            true
        });
        (refs, entries, error)
    }

    fn subjects(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| &*e.commit.subject).collect()
    }

    fn ref_names(refs: &Refs, oid: &str) -> Vec<(RefKind, String)> {
        refs.by_commit[oid]
            .iter()
            .map(|r| (r.kind, r.name.clone()))
            .collect()
    }

    #[test]
    fn subjects_split_at_a_conventional_commit_prefix() {
        assert_eq!(split_subject("feat: add it"), ("feat: ", "add it"));
        assert_eq!(split_subject("fix(ui): add it"), ("fix(ui): ", "add it"));
        assert_eq!(split_subject("feat!: add it"), ("feat!: ", "add it"));
        assert_eq!(
            split_subject("feat(ui)!: add it"),
            ("feat(ui)!: ", "add it")
        );
    }

    #[test]
    fn other_subjects_stay_whole() {
        assert_eq!(split_subject("Fix a bug"), ("", "Fix a bug"));
        assert_eq!(split_subject("Note: read this"), ("", "Note: read this"));
        assert_eq!(split_subject("two words: no"), ("", "two words: no"));
        assert_eq!(split_subject("[rcore] Fix: x"), ("", "[rcore] Fix: x"));
        assert_eq!(split_subject(": empty"), ("", ": empty"));
    }

    #[test]
    fn loads_a_merge_with_parents_children_first() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("side work");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("main work");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);

        let (refs, entries, error) = load_all(repo.path());
        assert_eq!(error, None);
        let refs = refs.unwrap();

        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].commit.subject.as_ref(), "merge side");
        assert_eq!(entries[0].commit.parents.len(), 2);
        assert_eq!(entries[0].row.outgoing.len(), 2);
        assert_eq!(entries[3].commit.subject.as_ref(), "base");
        assert!(entries[3].commit.parents.is_empty());
        assert_eq!(entries[0].commit.author.as_ref(), AUTHOR);
        assert_eq!(entries[0].commit.time, 1772600767);

        assert_eq!(refs.head.as_ref(), Some(&entries[0].commit.oid));
        let main = &refs.by_commit[&entries[0].commit.oid][0];
        assert!(main.is_head);
        assert_eq!((main.kind, main.name.as_str()), (RefKind::Branch, "main"));
    }

    #[test]
    fn includes_every_branch_and_tag_not_just_head() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["checkout", "-q", "--orphan", "other"]);
        repo.commit("unrelated");
        repo.git(&["tag", "-a", "v1", "-m", "release"]);
        repo.git(&["checkout", "-q", "main"]);

        let (refs, entries, _) = load_all(repo.path());
        assert_eq!(entries.len(), 2);
        let tagged = entries
            .iter()
            .find(|e| &*e.commit.subject == "unrelated")
            .unwrap();
        assert_eq!(
            ref_names(&refs.unwrap(), &tagged.commit.oid),
            [
                (RefKind::Branch, "other".to_owned()),
                (RefKind::Tag, "v1".to_owned())
            ],
            "the annotated tag resolves to its commit"
        );
    }

    #[test]
    fn detached_head_gets_its_own_ref() {
        let repo = Repo::new();
        let first = repo.commit("one");
        repo.commit("two");
        repo.git(&["checkout", "-q", "--detach", &first]);

        let (refs, entries, _) = load_all(repo.path());
        assert_eq!(entries.len(), 2);
        let head = &refs.unwrap().by_commit[first.as_str()][0];
        assert_eq!(head.kind, RefKind::DetachedHead);
        assert!(head.is_head);
    }

    #[test]
    fn empty_repository_is_not_an_error() {
        let repo = Repo::new();
        let (refs, entries, error) = load_all(repo.path());
        assert_eq!(error, None);
        assert!(entries.is_empty());
        assert_eq!(refs.unwrap().head, None);
    }

    #[test]
    fn a_folder_that_is_not_a_repository_reports_gits_error() {
        let dir = TempDir::new().unwrap();
        let (refs, _, error) = load_all(dir.path());
        assert!(refs.is_none());
        assert!(error.unwrap().contains("not a git repository"));
    }

    #[test]
    fn tags_on_blobs_are_skipped() {
        let repo = Repo::new();
        repo.commit("base");
        std::fs::write(repo.path().join("file"), "content").unwrap();
        let blob = repo.git(&["hash-object", "-w", "file"]);
        repo.git(&["tag", "blob-tag", &blob]);
        std::fs::remove_file(repo.path().join("file")).unwrap();

        let (_, entries, error) = load_all(repo.path());
        assert_eq!(error, None);
        assert_eq!(subjects(&entries), ["base"]);
    }

    #[test]
    fn a_stash_is_drawn_as_one_commit_on_the_commit_it_was_made_on() {
        let repo = Repo::new();
        std::fs::write(repo.path().join("file"), "one").unwrap();
        repo.git(&["add", "file"]);
        repo.commit("base");
        repo.commit("tip");
        std::fs::write(repo.path().join("file"), "two").unwrap();
        std::fs::write(repo.path().join("new"), "x").unwrap();
        repo.git(&["stash", "push", "-q", "-u", "-m", "half done"]);

        let (refs, entries, _) = load_all(repo.path());
        let refs = refs.expect("refs are read");
        // The index and untracked commits are not shown, and the stash sits on its base.
        assert_eq!(entries.len(), 3);
        let stash = entries
            .iter()
            .find(|e| refs.stash_at(&e.commit.oid) == Some(0))
            .expect("the stash is a row");
        assert!(stash.commit.subject.contains("half done"));
        assert_eq!(stash.commit.parents.len(), 1);
        assert_eq!(&*stash.commit.parents[0], &*entries[1].commit.oid);
        assert_eq!(stash.row.outgoing.len(), 1);
    }

    #[test]
    fn remote_head_symref_is_not_shown() {
        let repo = Repo::new();
        let oid = repo.commit("base");
        repo.git(&["update-ref", "refs/remotes/origin/main", &oid]);
        repo.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);

        let (refs, _, _) = load_all(repo.path());
        assert_eq!(
            ref_names(&refs.unwrap(), &oid),
            [
                (RefKind::Branch, "main".to_owned()),
                (RefKind::Remote, "origin/main".to_owned())
            ]
        );
    }

    #[test]
    fn subjects_survive_separator_characters_and_unicode() {
        let repo = Repo::new();
        repo.commit("fix: emoji 🎉 | pipes, \"quotes\" %H %x00");
        let (_, entries, _) = load_all(repo.path());
        assert_eq!(
            subjects(&entries),
            ["fix: emoji 🎉 | pipes, \"quotes\" %H %x00"]
        );
    }

    #[test]
    fn large_histories_arrive_in_batches() {
        let repo = Repo::new();
        let script: String = (0..BATCH_SIZE + 10)
            .map(|index| {
                format!("commit refs/heads/main\ncommitter T <t@e> {index} +0000\ndata 1\nx\n\n")
            })
            .collect();
        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["fast-import", "--quiet"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());

        let mut batches = Vec::new();
        load(repo.path(), i32::MAX as u32, |event| {
            if let LoadEvent::Batch(batch) = event {
                batches.push(batch.len());
            }
            true
        });
        assert_eq!(batches, [BATCH_SIZE, 10]);

        let mut seen = 0;
        load(repo.path(), i32::MAX as u32, |event| {
            seen += matches!(event, LoadEvent::Batch(_)) as usize;
            seen == 0
        });
        assert_eq!(seen, 1, "returning false stops the load");
    }

    fn entry<'a>(entries: &'a [Entry], subject: &str) -> &'a Entry {
        entries
            .iter()
            .find(|e| &*e.commit.subject == subject)
            .unwrap()
    }

    #[test]
    fn head_runs_down_the_first_lane_past_newer_branches() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "mine"]);
        repo.commit("my work");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("newer on main");

        repo.git(&["checkout", "-q", "mine"]);
        let (refs, entries, _) = load_all(repo.path());
        let refs = refs.unwrap();

        assert_eq!(refs.head_branch.as_deref(), Some("mine"));
        assert_eq!(refs.default_branch.as_deref(), Some("main"));
        assert_eq!(entry(&entries, "my work").row.column, 0);
        assert_eq!(entry(&entries, "my work").row.color, LaneColor::Head);
        assert_eq!(entry(&entries, "newer on main").row.column, 1);
        assert_eq!(
            entry(&entries, "newer on main").row.color,
            LaneColor::Branch(0),
            "main takes the first palette colour"
        );
        assert_eq!(entry(&entries, "base").row.column, 0);
        assert_eq!(
            entry(&entries, "base").row.color,
            LaneColor::Branch(0),
            "shared history below the fork takes main's colour"
        );
    }

    #[test]
    fn the_fork_stays_on_heads_line_after_merging_main_back_in() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "mine"]);
        repo.commit("my work");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("main work");
        repo.git(&["checkout", "-q", "mine"]);
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge main", "main"]);

        let (refs, entries, _) = load_all(repo.path());
        let base = &entry(&entries, "base").commit.oid;
        assert_eq!(refs.unwrap().plan.fork.as_ref(), Some(base));
        assert_eq!(entry(&entries, "my work").row.column, 0);
        assert_eq!(entry(&entries, "my work").row.color, LaneColor::Head);
    }

    #[test]
    fn on_the_default_branch_the_whole_line_is_blue() {
        let repo = Repo::new();
        repo.commit("base");
        repo.commit("next");
        let (refs, entries, _) = load_all(repo.path());
        assert_eq!(refs.unwrap().plan.fork, None);
        assert!(entries.iter().all(|e| e.row.color == LaneColor::Head));
    }

    #[test]
    fn a_branch_and_its_remote_share_one_colour_in_refs_and_lanes() {
        let repo = Repo::new();
        let base = repo.commit("base");
        repo.git(&["branch", "side", &base]);
        repo.git(&["checkout", "-q", "side"]);
        let tip = repo.commit("side tip");
        repo.git(&["update-ref", "refs/remotes/origin/side", &tip]);
        repo.git(&["checkout", "-q", "main"]);

        let (refs, entries, _) = load_all(repo.path());
        let refs = refs.unwrap();
        let side = &refs.by_commit[tip.as_str()];
        assert_eq!(side.len(), 2);
        assert_eq!(side[0].color, side[1].color);
        assert_eq!(side[0].color, LaneColor::Branch(1));
        assert_eq!(entry(&entries, "side tip").row.color, LaneColor::Branch(1));
    }

    #[test]
    fn colours_do_not_depend_on_commit_order() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["branch", "aaa"]);
        repo.git(&["branch", "zzz"]);
        let colors = |repo: &Repo| {
            let refs = read_refs(repo.path()).unwrap();
            let oid = refs.head.clone().unwrap();
            refs.by_commit[&oid]
                .iter()
                .map(|r| (r.name.clone(), r.color))
                .collect::<HashMap<_, _>>()
        };
        let before = colors(&repo);
        assert_eq!(before["aaa"], LaneColor::Branch(1));
        assert_eq!(before["zzz"], LaneColor::Branch(2));

        repo.commit("next");
        repo.git(&["branch", "-f", "aaa"]);
        repo.git(&["branch", "-f", "zzz"]);
        assert_eq!(colors(&repo)["aaa"], before["aaa"]);
    }

    #[test]
    fn branches_list_head_first_then_the_default_and_say_how_they_stand() {
        let repo = Repo::new();
        repo.commit("base");
        repo.git(&["branch", "merged-one"]);
        repo.git(&["checkout", "-q", "-b", "zebra"]);
        repo.commit("z1");
        repo.commit("z2");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("on main");
        repo.git(&["checkout", "-q", "-b", "alpha"]);
        repo.commit("a1");

        let mut refs = read_refs(repo.path()).unwrap();
        // The counts come in a second step, so the refs needn't wait for them.
        refs.set_counts(&refs::read_counts(repo.path(), &refs));
        let names: Vec<_> = refs.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["alpha", "main", "merged-one", "zebra"]);

        let counts = |name: &str| {
            refs.branches
                .iter()
                .find(|b| b.name == name)
                .unwrap()
                .ahead_behind
        };
        assert_eq!(
            counts("main"),
            Some((0, 0)),
            "the default is level with itself"
        );
        assert_eq!(counts("alpha"), Some((1, 0)));
        assert_eq!(counts("zebra"), Some((2, 1)));
        assert_eq!(counts("merged-one"), Some((0, 1)));

        let default = refs.default_branch.as_deref();
        let branch = |name: &str| refs.branches.iter().find(|b| b.name == name).unwrap();
        assert_eq!(branch("zebra").delta(default), "+2 −1");
        assert_eq!(branch("alpha").delta(default), "+1");
        assert_eq!(branch("main").delta(default), "");
        assert_eq!(
            branch("merged-one").delta(default),
            "",
            "merged says it with a check"
        );
        assert_eq!(
            branch("merged-one").merged_hint(default).as_deref(),
            Some("Merged into main")
        );
        assert_eq!(branch("main").merged_hint(default), None);
        let mut level = branch("alpha").clone();
        level.ahead_behind = Some((0, 0));
        assert_eq!(level.merged_hint(default), None, "no commits of its own");
        assert_eq!(branch("zebra").merged_hint(default), None);
        assert_eq!(
            branch("zebra").upstream_gone_hint(),
            None,
            "it tracks nothing"
        );
    }

    #[test]
    fn stashes_are_listed_though_not_drawn() {
        let repo = Repo::new();
        std::fs::write(repo.path().join("file"), "one").unwrap();
        repo.git(&["add", "file"]);
        repo.commit("base");
        std::fs::write(repo.path().join("file"), "two").unwrap();
        repo.git(&["stash", "push", "-q", "-m", "half done"]);

        let refs = read_refs(repo.path()).unwrap();
        assert_eq!(refs.stashes.len(), 1);
        assert!(refs.stashes[0].contains("half done"));
    }

    #[test]
    fn working_changes_become_the_top_row_and_count_a_rename_once() {
        let repo = Repo::new();
        std::fs::write(repo.path().join("a"), "content to rename\n").unwrap();
        repo.git(&["add", "a"]);
        repo.commit("base");
        repo.git(&["mv", "a", "b"]);
        std::fs::write(repo.path().join("new"), "x").unwrap();

        let (refs, entries, _) = load_all(repo.path());
        assert_eq!(refs.unwrap().status.changed, 2);
        assert!(entries[0].working);
        assert_eq!(&*entries[0].commit.subject, "Working changes in 2 files");
        assert!(!entries[1].working);
    }

    #[test]
    fn a_repository_with_changes_but_no_commits_still_shows_them() {
        let repo = Repo::new();
        std::fs::write(repo.path().join("new"), "x").unwrap();
        let (_, entries, error) = load_all(repo.path());
        assert_eq!(error, None);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].working);
    }

    #[test]
    fn a_clean_tree_has_no_working_row_and_saving_a_file_changes_the_fingerprint() {
        let repo = Repo::new();
        repo.commit("base");
        let (refs, entries, _) = load_all(repo.path());
        assert!(!entries[0].working);
        let before = refs.unwrap();
        std::fs::write(repo.path().join("new"), "x").unwrap();
        let after = read_refs(repo.path()).unwrap();
        assert_ne!(before.fingerprint(), after.fingerprint());
    }

    #[test]
    fn fingerprint_changes_when_refs_move() {
        let repo = Repo::new();
        repo.commit("base");
        let before = read_refs(repo.path()).unwrap();
        assert_eq!(
            before.fingerprint(),
            read_refs(repo.path()).unwrap().fingerprint()
        );

        repo.commit("next");
        assert_ne!(
            before.fingerprint(),
            read_refs(repo.path()).unwrap().fingerprint()
        );

        let moved = read_refs(repo.path()).unwrap();
        repo.git(&["checkout", "-q", "-b", "same-commit"]);
        assert_ne!(
            moved.fingerprint(),
            read_refs(repo.path()).unwrap().fingerprint(),
            "switching branch moves no commit but does move HEAD"
        );
    }
}
