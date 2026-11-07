//! Reads history through the `git` executable.
//!
//! Going through the CLI keeps this working with everything the user's git understands
//! (reftables, SHA-256, partial clones, the commit-graph cache) without a libgit2 build.

use crate::graph::{GraphBuilder, LaneColor, Plan, Row};
use crate::ops::{self, git, stderr_message};
use crate::status::{self, RepoStatus};
use crate::text::count;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::thread;

const BATCH_SIZE: usize = 1024;

/// What a default branch usually goes by, in the order they're tried.
const USUAL_DEFAULT_BRANCHES: [&str; 3] = ["main", "master", "trunk"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: Arc<str>,
    pub parents: Box<[Arc<str>]>,
    pub author: Arc<str>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    /// HEAD pointing straight at a commit.
    DetachedHead,
    Branch,
    Tag,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub kind: RefKind,
    /// The name without its `refs/...` prefix.
    pub name: String,
    /// Whether this is the branch HEAD is on.
    pub is_head: bool,
    /// The branch's colour, the same in the lanes and the chips. Unused for tags.
    pub color: LaneColor,
}

impl Ref {
    /// The branch this is, or follows: a remote's `origin/main` is `main`.
    pub fn branch_name(&self) -> &str {
        branch_key(self.kind, &self.name)
    }
}

/// A local branch, for the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub tip: Arc<str>,
    pub color: LaneColor,
    pub is_head: bool,
    /// Commits this has that the default branch doesn't, and the other way around.
    pub ahead_behind: Option<(u32, u32)>,
}

impl Branch {
    /// How the branch stands against the default branch, in a few characters.
    pub fn delta(&self, default_branch: Option<&str>) -> String {
        match self.ahead_behind {
            _ if default_branch == Some(self.name.as_str()) => String::new(),
            Some((0, _)) => "merged".to_owned(),
            Some((ahead, 0)) => format!("+{ahead}"),
            Some((ahead, behind)) => format!("+{ahead} −{behind}"),
            None => String::new(),
        }
    }
}

/// A remote branch or tag, for the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedRef {
    pub name: String,
    pub tip: Arc<str>,
    pub color: LaneColor,
}

/// The state of a repository's refs at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub by_commit: HashMap<Arc<str>, Vec<Ref>>,
    pub head: Option<Arc<str>>,
    /// The short name of the branch HEAD is on.
    pub head_branch: Option<String>,
    /// The branch history is shared on, usually `main`.
    pub default_branch: Option<String>,
    /// HEAD's branch, then the default branch, then the rest by name.
    pub branches: Vec<Branch>,
    pub remotes: Vec<NamedRef>,
    pub tags: Vec<NamedRef>,
    /// Stash subjects, newest first.
    pub stashes: Vec<String>,
    /// How to lay out the graph for this repository.
    pub plan: Plan,
    /// The top of the working tree, or the repository itself when it is bare.
    pub workdir: Option<PathBuf>,
    pub status: RepoStatus,
    /// The commits to walk history from.
    tips: Vec<Arc<str>>,
    /// Compares equal exactly when no ref moved.
    fingerprint: String,
}

impl Refs {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn is_head(&self, oid: &Arc<str>) -> bool {
        self.head.as_ref() == Some(oid)
    }

    /// What to call where HEAD is: its branch, when it's on one.
    pub fn head_name(&self) -> &str {
        self.head_branch.as_deref().unwrap_or("HEAD")
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
    Failed(String),
}

/// Loads a repository on its own thread. The channel closes when loading is done, and
/// dropping the receiver cancels the load and stops `git`.
pub fn spawn_load(repo: PathBuf) -> async_channel::Receiver<LoadEvent> {
    let (sender, receiver) = async_channel::bounded(4);
    thread::Builder::new()
        .name("git-load".into())
        .spawn(move || load(&repo, |event| sender.send_blocking(event).is_ok()))
        .expect("failed to spawn the git loader thread");
    receiver
}

/// Reads refs and history, passing each step to `emit` until it returns false.
pub fn load(repo: &Path, mut emit: impl FnMut(LoadEvent) -> bool) {
    let refs = match read_refs(repo) {
        Ok(refs) => Arc::new(refs),
        Err(message) => {
            emit(LoadEvent::Failed(message));
            return;
        }
    };
    if !emit(LoadEvent::Refs(refs.clone())) {
        return;
    }

    let mut builder = GraphBuilder::with_plan(refs.plan.clone());
    let files = refs.status.changed;
    let working = (files > 0).then(|| working_entry(files, &mut builder));
    if let Some(working) = &working
        && refs.tips.is_empty()
        && !emit(LoadEvent::Batch(vec![working.clone()]))
    {
        return;
    }
    let mut working = working.filter(|_| !refs.tips.is_empty());
    let result = stream_commits(repo, &refs.tips, |commits| {
        let batch = working
            .take()
            .into_iter()
            .chain(commits.into_iter().map(|commit| Entry {
                row: builder.push(&commit.oid, &commit.parents),
                commit,
                working: false,
            }))
            .collect();
        emit(LoadEvent::Batch(batch))
    });
    if let Err(message) = result {
        emit(LoadEvent::Failed(message));
    }
}

fn working_entry(files: u32, builder: &mut GraphBuilder) -> Entry {
    Entry {
        row: builder.working_row(),
        commit: Commit {
            oid: "".into(),
            parents: Box::default(),
            author: "".into(),
            time: 0,
            subject: format!("Working changes in {}", count(files, "file")).into(),
        },
        working: true,
    }
}

pub fn read_refs(repo: &Path) -> Result<Refs, String> {
    const FORMAT: &str =
        "%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(refname)%00%(symref)";
    let listing = ops::run(
        repo,
        &[
            "for-each-ref",
            &format!("--format={FORMAT}"),
            // Narrower than --all: stashes and notes are not history anyone wants drawn.
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;

    // Both of these fail, as expected, in some states: HEAD can be detached or unborn.
    let head_ref = ops::run(repo, &["symbolic-ref", "-q", "HEAD"]).ok();
    let head_ref = head_ref.map(|name| name.trim().to_owned());
    let head = ops::run(repo, &["rev-parse", "-q", "--verify", "HEAD^{commit}"]).ok();
    let head: Option<Arc<str>> = head.map(|oid| oid.trim().into());

    let toplevel = ops::run(repo, &["rev-parse", "--show-toplevel"]).ok();
    let workdir = toplevel.map(|path| PathBuf::from(path.trim_end()));

    let status = status::read(repo);

    let mut refs = Refs {
        workdir: workdir.or_else(|| Some(repo.to_owned())),
        fingerprint: format!("{listing}\0{head_ref:?}\0{head:?}\0{status:?}"),
        status,
        ..Refs::default()
    };

    let mut found = Vec::new();
    let mut origin_head = None;
    for line in listing.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [object, object_type, peeled, peeled_type, full_name, symref] = fields[..] else {
            return Err(format!("unexpected output from git for-each-ref: {line:?}"));
        };
        if full_name == "refs/remotes/origin/HEAD" {
            origin_head = symref.strip_prefix("refs/remotes/origin/");
        }
        // Annotated tags point at a tag object; history starts at the commit behind it.
        // Tags can also name trees and blobs, which have no place in the graph.
        let commit = match (object_type, peeled_type) {
            ("commit", _) => object,
            (_, "commit") => peeled,
            _ => continue,
        };
        if let Some((kind, name)) = classify_ref(full_name) {
            found.push((kind, name, commit, full_name));
        }
    }

    let head_branch = head_ref
        .as_deref()
        .and_then(|full_name| full_name.strip_prefix("refs/heads/"));
    let default_branch = default_branch(&found, origin_head);
    let colors = branch_colors(&found, default_branch.as_deref(), head_branch);
    let color_of = |kind: RefKind, name: &str| match kind {
        RefKind::DetachedHead => LaneColor::Head,
        _ => colors
            .get(branch_key(kind, name))
            .copied()
            .unwrap_or_default(),
    };

    for &(kind, name, commit, full_name) in &found {
        let color = color_of(kind, name);
        let oid = intern(&mut refs.tips, commit);
        if kind != RefKind::Tag && color != LaneColor::Head {
            let shared = default_branch.as_deref() == Some(branch_key(kind, name));
            let tip = refs.plan.tips.entry(oid.clone()).or_insert(color);
            if shared {
                *tip = color;
            }
        }
        refs.by_commit.entry(oid).or_default().push(Ref {
            kind,
            name: name.to_owned(),
            is_head: head_ref.as_deref() == Some(full_name),
            color,
        });
    }

    if let Some(head) = &head {
        let oid = intern(&mut refs.tips, head);
        if head_ref.is_none() {
            refs.by_commit.entry(oid).or_default().push(Ref {
                kind: RefKind::DetachedHead,
                name: "HEAD".into(),
                is_head: true,
                color: LaneColor::Head,
            });
        }
    }

    let ahead_behind = default_branch
        .as_deref()
        .map(|default| read_ahead_behind(repo, &found, default))
        .unwrap_or_default();
    for &(kind, name, commit, _) in &found {
        let tip: Arc<str> = commit.into();
        let color = color_of(kind, name);
        match kind {
            RefKind::Branch => refs.branches.push(Branch {
                name: name.to_owned(),
                tip,
                color,
                is_head: head_branch == Some(name),
                ahead_behind: ahead_behind.get(name).copied(),
            }),
            RefKind::Remote => refs.remotes.push(NamedRef {
                name: name.to_owned(),
                tip,
                color,
            }),
            RefKind::Tag => refs.tags.push(NamedRef {
                name: name.to_owned(),
                tip,
                color,
            }),
            RefKind::DetachedHead => {}
        }
    }
    refs.branches.sort_by_key(|branch| {
        let is_default = default_branch.as_deref() == Some(branch.name.as_str());
        (!branch.is_head, !is_default, branch.name.clone())
    });
    refs.tags.reverse();
    let stashes = ops::run(repo, &["stash", "list", "--format=%s"]).unwrap_or_default();
    refs.stashes = stashes.lines().map(str::to_owned).collect();

    refs.plan.fork = default_branch
        .as_deref()
        .filter(|&default| head.is_some() && head_branch != Some(default))
        .and_then(|default| fork_point(repo, shared_ref(&found, default)?.3));
    refs.plan.head = head.clone();
    refs.plan.shared_color = default_branch
        .as_deref()
        .and_then(|default| colors.get(default).copied())
        .filter(|color| *color != LaneColor::Head)
        .unwrap_or_default();
    refs.plan.first_free_color = colors.len() as u32 + 1;
    refs.head = head;
    refs.head_branch = head_branch.map(str::to_owned);
    refs.default_branch = default_branch;

    for commit_refs in refs.by_commit.values_mut() {
        commit_refs.sort_by_key(|r| (!r.is_head, r.kind));
    }
    Ok(refs)
}

/// A remote branch and the local branch of the same name are one branch to the eye.
fn branch_key(kind: RefKind, name: &str) -> &str {
    match kind {
        RefKind::Remote => name.split_once('/').map_or(name, |(_, branch)| branch),
        _ => name,
    }
}

type FoundRef<'a> = (RefKind, &'a str, &'a str, &'a str);

/// The branch history is shared on: what origin calls its default, or a usual name.
pub(crate) fn pick_default_branch(
    origin_head: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Option<&str> {
    origin_head
        .filter(|name| exists(name))
        .or_else(|| USUAL_DEFAULT_BRANCHES.into_iter().find(|n| exists(n)))
}

fn default_branch(found: &[FoundRef], origin_head: Option<&str>) -> Option<String> {
    let exists = |wanted: &str| {
        found
            .iter()
            .any(|r| r.0 != RefKind::Tag && branch_key(r.0, r.1) == wanted)
    };
    pick_default_branch(origin_head, exists).map(str::to_owned)
}

/// The ref to compare with the default branch: the local one, or failing that a remote's.
fn shared_ref<'a, 'b>(found: &'a [FoundRef<'b>], default: &str) -> Option<&'a FoundRef<'b>> {
    found
        .iter()
        .find(|r| r.0 == RefKind::Branch && r.1 == default)
        .or_else(|| {
            found
                .iter()
                .find(|r| r.0 == RefKind::Remote && branch_key(r.0, r.1) == default)
        })
}

/// Gives every branch its colour. HEAD's is Blue, the default branch takes the first of
/// the palette, and the rest follow in name order, so colours don't depend on commits.
fn branch_colors(
    found: &[FoundRef],
    default_branch: Option<&str>,
    head_branch: Option<&str>,
) -> HashMap<String, LaneColor> {
    let mut names: Vec<&str> = found
        .iter()
        .filter(|r| matches!(r.0, RefKind::Branch | RefKind::Remote))
        .map(|r| branch_key(r.0, r.1))
        .filter(|name| Some(*name) != default_branch && Some(*name) != head_branch)
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut colors: HashMap<String, LaneColor> = names
        .into_iter()
        .zip(1..)
        .map(|(name, index)| (name.to_owned(), LaneColor::Branch(index)))
        .collect();
    if let Some(default) = default_branch {
        colors.insert(default.to_owned(), LaneColor::Branch(0));
    }
    if let Some(head) = head_branch {
        colors.insert(head.to_owned(), LaneColor::Head);
    }
    colors
}

/// How each local branch stands against the default branch, in one git call. Old versions
/// of git don't know this format, which only costs the sidebar its counts.
fn read_ahead_behind(
    repo: &Path,
    found: &[FoundRef],
    default: &str,
) -> HashMap<String, (u32, u32)> {
    let Some(base) = shared_ref(found, default) else {
        return HashMap::new();
    };
    let format = format!("--format=%(refname:short)%00%(ahead-behind:{})", base.3);
    let listing = ops::run(repo, &["for-each-ref", &format, "refs/heads"]).unwrap_or_default();
    listing
        .lines()
        .filter_map(|line| {
            let (name, counts) = line.split_once('\0')?;
            let (ahead, behind) = counts.split_once(' ')?;
            Some((name.to_owned(), (ahead.parse().ok()?, behind.parse().ok()?)))
        })
        .collect()
}

/// Where HEAD's first-parent line joins `base`. This can't be `git merge-base`: after
/// merging `base` back into a branch, that commit isn't on HEAD's line at all.
fn fork_point(repo: &Path, base: &str) -> Option<Arc<str>> {
    let exclude = format!("^{base}");
    let args = ["rev-list", "--first-parent", "--parents", "HEAD", &exclude];
    let listing = ops::run(repo, &args).ok()?;
    let Some(oldest) = listing.lines().last() else {
        // Nothing of HEAD's is outside base, so all of its line is shared.
        return ops::run(repo, &["rev-parse", "HEAD"])
            .ok()
            .map(|head| head.trim().into());
    };
    oldest.split_whitespace().nth(1).map(Arc::from)
}

fn intern(tips: &mut Vec<Arc<str>>, oid: &str) -> Arc<str> {
    let oid: Arc<str> = oid.into();
    tips.push(oid.clone());
    oid
}

fn classify_ref(full_name: &str) -> Option<(RefKind, &str)> {
    if let Some(name) = full_name.strip_prefix("refs/heads/") {
        Some((RefKind::Branch, name))
    } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
        Some((RefKind::Tag, name))
    } else {
        // refs/remotes/origin/HEAD only repeats what origin's default branch already shows.
        let name = full_name.strip_prefix("refs/remotes/")?;
        (!name.ends_with("/HEAD")).then_some((RefKind::Remote, name))
    }
}

/// Streams commits reachable from `tips`, children before parents, until `sink` returns
/// false.
fn stream_commits(
    repo: &Path,
    tips: &[Arc<str>],
    mut sink: impl FnMut(Vec<Commit>) -> bool,
) -> Result<(), String> {
    if tips.is_empty() {
        return Ok(());
    }

    let mut child = git(repo)
        .args([
            "log",
            "--stdin",
            "--topo-order",
            "--encoding=UTF-8",
            "--no-color",
            // log.showSignature would otherwise mix gpg output into the records.
            "--no-show-signature",
            "--format=%H%x00%P%x00%aN%x00%at%x00%s",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run git: {error}"))?;
    let guard = KillOnDrop(&mut child);

    let mut stdin = guard.0.stdin.take().expect("stdin is piped");
    let stdout = guard.0.stdout.take().expect("stdout is piped");
    let mut stderr = guard.0.stderr.take().expect("stderr is piped");
    let stderr = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    // git reads all of stdin before it writes anything, so this can't deadlock. If git
    // quits early the write fails, and its exit status is what gets reported below.
    let _ = tips.iter().try_for_each(|tip| writeln!(stdin, "{tip}"));
    drop(stdin);

    let mut reader = BufReader::with_capacity(1 << 16, stdout);
    let mut line = Vec::new();
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read from git: {error}"))?;
        if read == 0 {
            break;
        }
        batch.push(parse_commit(&line)?);
        if batch.len() == BATCH_SIZE
            && !sink(std::mem::replace(
                &mut batch,
                Vec::with_capacity(BATCH_SIZE),
            ))
        {
            return Ok(());
        }
    }
    if !batch.is_empty() && !sink(batch) {
        return Ok(());
    }

    let status = guard
        .0
        .wait()
        .map_err(|error| format!("could not wait for git: {error}"))?;
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(stderr_message(&stderr))
    }
}

/// Stops `git` when a load is abandoned partway through.
struct KillOnDrop<'a>(&'a mut Child);

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn parse_commit(line: &[u8]) -> Result<Commit, String> {
    let line = String::from_utf8_lossy(line);
    let line = line.strip_suffix('\n').unwrap_or(&line);
    let fields: Vec<&str> = line.split('\0').collect();
    let [oid, parents, author, time, subject] = fields[..] else {
        return Err(format!("unexpected output from git log: {line:?}"));
    };
    Ok(Commit {
        oid: oid.into(),
        parents: parents.split_ascii_whitespace().map(Arc::from).collect(),
        author: author.into(),
        time: time.parse().unwrap_or(0),
        subject: subject.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AUTHOR, Repo};
    use std::process::Command;
    use tempfile::TempDir;

    fn load_all(repo: &Path) -> (Option<Arc<Refs>>, Vec<Entry>, Option<String>) {
        let (mut refs, mut entries, mut error) = (None, Vec::new(), None);
        load(repo, |event| {
            match event {
                LoadEvent::Refs(loaded) => refs = Some(loaded),
                LoadEvent::Batch(batch) => entries.extend(batch),
                LoadEvent::Failed(message) => error = Some(message),
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
    fn stashes_are_not_drawn() {
        let repo = Repo::new();
        std::fs::write(repo.path().join("file"), "one").unwrap();
        repo.git(&["add", "file"]);
        repo.commit("base");
        std::fs::write(repo.path().join("file"), "two").unwrap();
        repo.git(&["stash"]);

        let (_, entries, _) = load_all(repo.path());
        assert_eq!(subjects(&entries), ["base"]);
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
        load(repo.path(), |event| {
            if let LoadEvent::Batch(batch) = event {
                batches.push(batch.len());
            }
            true
        });
        assert_eq!(batches, [BATCH_SIZE, 10]);

        let mut seen = 0;
        load(repo.path(), |event| {
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

        let refs = read_refs(repo.path()).unwrap();
        let names: Vec<_> = refs.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["alpha", "main", "merged-one", "zebra"]);

        let default = refs.default_branch.as_deref();
        let delta = |name: &str| {
            let branch = refs.branches.iter().find(|b| b.name == name).unwrap();
            branch.delta(default)
        };
        assert_eq!(delta("main"), "", "the default has nothing to compare with");
        assert_eq!(delta("alpha"), "+1");
        assert_eq!(delta("zebra"), "+2 −1");
        assert_eq!(delta("merged-one"), "merged");
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
