//! Reads history through the `git` executable.
//!
//! Going through the CLI keeps this working with everything the user's git understands
//! (reftables, SHA-256, partial clones, the commit-graph cache) without a libgit2 build.

use crate::graph::{GraphBuilder, Row};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;

const BATCH_SIZE: usize = 1024;

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
    pub fn short_oid(&self) -> &str {
        &self.oid[..self.oid.len().min(7)]
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
}

/// The state of a repository's refs at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub by_commit: HashMap<Arc<str>, Vec<Ref>>,
    pub head: Option<Arc<str>>,
    /// The top of the working tree, or the repository itself when it is bare.
    pub workdir: Option<PathBuf>,
    /// The commits to walk history from.
    tips: Vec<Arc<str>>,
    /// Compares equal exactly when no ref moved.
    fingerprint: String,
}

impl Refs {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub commit: Commit,
    pub row: Row,
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

    let mut builder = GraphBuilder::new();
    let result = stream_commits(repo, &refs.tips, |commits| {
        let batch = commits
            .into_iter()
            .map(|commit| Entry {
                row: builder.push(&commit.oid, &commit.parents),
                commit,
            })
            .collect();
        emit(LoadEvent::Batch(batch))
    });
    if let Err(message) = result {
        emit(LoadEvent::Failed(message));
    }
}

pub(crate) fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        // Launched from inside a git hook these would point at some other repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        // Only reading, so never contend with a git the user is running.
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn run_git(repo: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    git(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run git: {error}"))
}

pub fn read_refs(repo: &Path) -> Result<Refs, String> {
    const FORMAT: &str =
        "%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(refname)";
    let output = run_git(
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
    if !output.status.success() {
        return Err(stderr_message(&output.stderr));
    }
    let listing = String::from_utf8_lossy(&output.stdout);

    // Both of these fail, as expected, in some states: HEAD can be detached or unborn.
    let head_ref = run_git(repo, &["symbolic-ref", "-q", "HEAD"])?;
    let head_ref = head_ref
        .status
        .success()
        .then(|| String::from_utf8_lossy(&head_ref.stdout).trim().to_owned());
    let head = run_git(repo, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
    let head: Option<Arc<str>> = head
        .status
        .success()
        .then(|| String::from_utf8_lossy(&head.stdout).trim().into());

    let toplevel = run_git(repo, &["rev-parse", "--show-toplevel"])?;
    let workdir = toplevel
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&toplevel.stdout).trim_end()));

    let mut refs = Refs {
        workdir: workdir.or_else(|| Some(repo.to_owned())),
        fingerprint: format!("{listing}\0{head_ref:?}\0{head:?}"),
        ..Refs::default()
    };

    for line in listing.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [object, object_type, peeled, peeled_type, full_name] = fields[..] else {
            return Err(format!("unexpected output from git for-each-ref: {line:?}"));
        };
        // Annotated tags point at a tag object; history starts at the commit behind it.
        // Tags can also name trees and blobs, which have no place in the graph.
        let commit = match (object_type, peeled_type) {
            ("commit", _) => object,
            (_, "commit") => peeled,
            _ => continue,
        };
        let Some((kind, name)) = classify_ref(full_name) else {
            continue;
        };
        let oid = intern(&mut refs.tips, commit);
        refs.by_commit.entry(oid).or_default().push(Ref {
            kind,
            name: name.to_owned(),
            is_head: head_ref.as_deref() == Some(full_name),
        });
    }

    if let Some(head) = &head {
        let oid = intern(&mut refs.tips, head);
        if head_ref.is_none() {
            refs.by_commit.entry(oid).or_default().push(Ref {
                kind: RefKind::DetachedHead,
                name: "HEAD".into(),
                is_head: true,
            });
        }
    }
    refs.head = head;

    for commit_refs in refs.by_commit.values_mut() {
        commit_refs.sort_by_key(|r| (!r.is_head, r.kind));
    }
    Ok(refs)
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

pub(crate) fn stderr_message(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr);
    let message = message.trim();
    if message.is_empty() {
        "git failed without an error message".into()
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Repo(TempDir);

    impl Repo {
        fn new() -> Self {
            let repo = Repo(TempDir::new().unwrap());
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn git_output(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(["-c", "user.name=Tester", "-c", "user.email=t@example.com"])
                .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", self.path().join(".no-global-config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_DATE", "2024-01-02T03:04:05Z")
                .env("GIT_COMMITTER_DATE", "2024-01-02T03:04:05Z")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }

        fn git(&self, args: &[&str]) {
            self.git_output(args);
        }

        fn commit(&self, subject: &str) -> String {
            self.git(&["commit", "-q", "--allow-empty", "-m", subject]);
            self.git_output(&["rev-parse", "HEAD"])
        }
    }

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
        assert_eq!(entries[0].commit.author.as_ref(), "Tester");
        assert_eq!(entries[0].commit.time, 1704164645);

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
        let blob = repo.git_output(&["hash-object", "-w", "file"]);
        repo.git(&["tag", "blob-tag", &blob]);

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
