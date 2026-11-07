//! Where a repository stands, and the one thing to do next.

use crate::ops;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// The branch HEAD is on, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// The commit HEAD is at, or `None` before the first commit.
    pub head: Option<String>,
    /// The branch this one tracks, such as `origin/main`.
    pub upstream: Option<String>,
    /// The tracked branch no longer exists on the remote.
    pub upstream_gone: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Files with uncommitted changes, including new ones.
    pub changed: u32,
    /// The default branch and how many of its commits this branch doesn't have.
    pub base: Option<(String, u32)>,
    /// Where a branch would be published. `None` when the repository has no remote.
    pub remote: Option<String>,
}

/// What the header's one filled button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextAction {
    Publish {
        remote: String,
        branch: String,
    },
    Push {
        commits: u32,
    },
    Pull {
        commits: u32,
    },
    /// The branch and its remote have diverged, as they do after a rebase.
    ForcePush {
        commits: u32,
        replaced: u32,
    },
}

impl NextAction {
    pub fn label(&self) -> String {
        match self {
            Self::Publish { remote, .. } => format!("Push to {remote}"),
            Self::Push { commits } => format!("Push {}", count(*commits, "commit")),
            Self::Pull { commits } => format!("Pull {}", count(*commits, "commit")),
            Self::ForcePush { commits, .. } => format!("Force push {}", count(*commits, "commit")),
        }
    }
}

/// One stretch of the status sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    /// The branch you're on.
    Current(String),
    /// Some other branch.
    Branch(String),
}

fn count(number: u32, noun: &str) -> String {
    match number {
        1 => format!("1 {noun}"),
        _ => format!("{number} {noun}s"),
    }
}

impl RepoStatus {
    pub fn is_on_base(&self) -> bool {
        matches!((&self.branch, &self.base), (Some(b), Some((base, _))) if b == base)
    }

    pub fn next_action(&self) -> Option<NextAction> {
        let branch = self.branch.as_ref()?;
        self.head.as_ref()?;
        if self.upstream_gone || self.upstream.is_none() {
            let remote = self.remote.clone()?;
            let branch = branch.clone();
            return Some(NextAction::Publish { remote, branch });
        }
        match (self.ahead, self.behind) {
            (0, 0) => None,
            (ahead, 0) => Some(NextAction::Push { commits: ahead }),
            (0, behind) => Some(NextAction::Pull { commits: behind }),
            // Replacing the default branch's history is not a one-button step.
            _ if self.is_on_base() => None,
            (commits, replaced) => Some(NextAction::ForcePush { commits, replaced }),
        }
    }

    /// Says where you are in plain words.
    pub fn sentence(&self) -> Vec<Span> {
        let Some(head) = &self.head else {
            let branch = self.branch.clone().unwrap_or_else(|| "this branch".into());
            return vec![
                Span::Text("On ".into()),
                Span::Current(branch),
                Span::Text(", no commits yet.".into()),
            ];
        };

        let mut spans = match &self.branch {
            Some(branch) => {
                let state = match (&self.upstream, self.upstream_gone, self.ahead, self.behind) {
                    (None, ..) | (_, true, ..) => ", not on a remote yet.".to_owned(),
                    (_, _, 0, 0) => ", up to date.".to_owned(),
                    (_, _, ahead, 0) => format!(", {} to push.", count(ahead, "commit")),
                    (_, _, 0, behind) => format!(", {} to pull.", count(behind, "commit")),
                    (_, _, ahead, behind) => {
                        format!(", {} to push and {behind} to pull.", count(ahead, "commit"))
                    }
                };
                vec![
                    Span::Text("On ".into()),
                    Span::Current(branch.clone()),
                    Span::Text(state),
                ]
            }
            None => {
                let short = &head[..head.len().min(7)];
                vec![Span::Text(format!("HEAD is detached at {short}."))]
            }
        };

        if let Some((base, new)) = &self.base
            && *new > 0
            && !self.is_on_base()
        {
            spans.push(Span::Text(" ".into()));
            spans.push(Span::Branch(base.clone()));
            spans.push(Span::Text(format!(
                " has {} new {}.",
                new,
                if *new == 1 { "commit" } else { "commits" }
            )));
        }
        if self.changed > 0 {
            let files = count(self.changed, "file");
            spans.push(Span::Text(format!(" {files} changed.")));
        }
        spans
    }
}

/// Reads the status. A repository with no working tree simply reports nothing.
pub fn read(repo: &Path) -> RepoStatus {
    let args = ["status", "--porcelain=v2", "--branch", "-z"];
    let mut status = ops::run(repo, &args)
        .map(|output| parse(&output))
        .unwrap_or_default();

    let remotes = ops::run(repo, &["remote"]).unwrap_or_default();
    let mut remotes = remotes.lines();
    status.remote = remotes
        .clone()
        .find(|remote| *remote == "origin")
        .or_else(|| remotes.next())
        .map(str::to_owned);

    status.base = default_branch(repo).and_then(|(name, full)| {
        let range = format!("HEAD..{full}");
        let new = ops::run(repo, &["rev-list", "--count", &range]).ok()?;
        Some((name, new.trim().parse().unwrap_or(0)))
    });
    status
}

fn default_branch(repo: &Path) -> Option<(String, String)> {
    let symbolic = ["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"];
    if let Ok(remote) = ops::run(repo, &symbolic) {
        let remote = remote.trim();
        if let Some((_, name)) = remote.split_once('/') {
            return Some((name.to_owned(), remote.to_owned()));
        }
    }
    ["main", "master", "trunk"].into_iter().find_map(|name| {
        let full = format!("refs/heads/{name}");
        ops::run(repo, &["rev-parse", "-q", "--verify", &full]).ok()?;
        Some((name.to_owned(), name.to_owned()))
    })
}

fn parse(output: &str) -> RepoStatus {
    let mut status = RepoStatus::default();
    let mut records = output.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            match header.split_once(' ') {
                Some(("branch.oid", oid)) if oid != "(initial)" => {
                    status.head = Some(oid.to_owned());
                }
                Some(("branch.head", head)) if head != "(detached)" => {
                    status.branch = Some(head.to_owned());
                }
                Some(("branch.upstream", upstream)) => status.upstream = Some(upstream.to_owned()),
                Some(("branch.ab", counts)) => {
                    let mut counts = counts.split(' ');
                    let mut next = || {
                        let count = counts.next().unwrap_or("0");
                        count[1.min(count.len())..].parse().unwrap_or(0)
                    };
                    status.ahead = next();
                    status.behind = next();
                }
                _ => {}
            }
        } else if record.starts_with("2 ") {
            // A rename's old path is its own record.
            records.next();
            status.changed += 1;
        } else if record.starts_with(['1', 'u', '?']) {
            status.changed += 1;
        }
    }
    // Git reports no ahead/behind for an upstream that no longer exists.
    status.upstream_gone = status.upstream.is_some() && !output.contains("# branch.ab ");
    status
}

pub fn fetch(repo: &Path) -> Result<Option<String>, String> {
    ops::run(repo, &["fetch", "--all"]).map(|_| None)
}

pub fn run_next_action(repo: &Path, action: &NextAction) -> Result<Option<String>, String> {
    match action {
        NextAction::Publish { remote, branch } => {
            ops::run(repo, &["push", "-u", remote, branch]).map(|_| None)
        }
        NextAction::Push { .. } => ops::run(repo, &["push"]).map(|_| None),
        NextAction::Pull { .. } => ops::run(repo, &["pull", "--ff-only"]).map(|_| None),
        NextAction::ForcePush { .. } => {
            ops::run(repo, &["push", "--force-with-lease"]).map(|_| None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=T", "-c", "user.email=t@e.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", dir.join(".no-global-config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(dir: &Path, subject: &str) {
        git(dir, &["commit", "-q", "--allow-empty", "-m", subject]);
    }

    fn cloned() -> (TempDir, TempDir, TempDir) {
        let (remote, one, two) = (
            TempDir::new().unwrap(),
            TempDir::new().unwrap(),
            TempDir::new().unwrap(),
        );
        git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(one.path(), &["init", "-q", "-b", "main"]);
        commit(one.path(), "base");
        let url = remote.path().to_str().unwrap();
        git(one.path(), &["remote", "add", "origin", url]);
        git(one.path(), &["push", "-q", "-u", "origin", "main"]);
        git(two.path(), &["clone", "-q", url, "."]);
        (remote, one, two)
    }

    fn text(spans: &[Span]) -> String {
        spans
            .iter()
            .map(|span| match span {
                Span::Text(text) | Span::Current(text) | Span::Branch(text) => text.as_str(),
            })
            .collect()
    }

    #[test]
    fn a_synced_branch_is_up_to_date_and_has_nothing_to_do() {
        let (_remote, one, _two) = cloned();
        let status = read(one.path());
        assert_eq!(text(&status.sentence()), "On main, up to date.");
        assert_eq!(status.next_action(), None);
    }

    #[test]
    fn unpushed_commits_are_the_next_action() {
        let (_remote, one, _two) = cloned();
        commit(one.path(), "a");
        commit(one.path(), "b");
        let status = read(one.path());
        assert_eq!(text(&status.sentence()), "On main, 2 commits to push.");
        let action = status.next_action().unwrap();
        assert_eq!(action.label(), "Push 2 commits");

        run_next_action(one.path(), &action).unwrap();
        assert_eq!(read(one.path()).next_action(), None);
    }

    #[test]
    fn commits_on_the_remote_can_be_pulled_after_a_fetch() {
        let (_remote, one, two) = cloned();
        commit(one.path(), "a");
        git(one.path(), &["push", "-q"]);

        assert_eq!(read(two.path()).next_action(), None, "not fetched yet");
        fetch(two.path()).unwrap();
        let status = read(two.path());
        assert_eq!(text(&status.sentence()), "On main, 1 commit to pull.");
        let action = status.next_action().unwrap();
        assert_eq!(action.label(), "Pull 1 commit");

        run_next_action(two.path(), &action).unwrap();
        assert_eq!(text(&read(two.path()).sentence()), "On main, up to date.");
    }

    #[test]
    fn a_new_branch_offers_to_publish_and_mentions_new_commits_on_main() {
        let (_remote, one, _two) = cloned();
        git(one.path(), &["checkout", "-q", "-b", "feature"]);
        git(one.path(), &["checkout", "-q", "main"]);
        commit(one.path(), "a");
        commit(one.path(), "b");
        git(one.path(), &["checkout", "-q", "feature"]);

        let status = read(one.path());
        assert_eq!(
            text(&status.sentence()),
            "On feature, not on a remote yet. main has 2 new commits."
        );
        let action = status.next_action().unwrap();
        assert_eq!(action.label(), "Push to origin");

        run_next_action(one.path(), &action).unwrap();
        assert_eq!(read(one.path()).upstream.as_deref(), Some("origin/feature"));
    }

    #[test]
    fn a_rebased_branch_needs_a_force_push_and_only_that_replaces_the_remote() {
        let (_remote, one, _two) = cloned();
        git(one.path(), &["checkout", "-q", "-b", "feature"]);
        commit(one.path(), "mine");
        git(one.path(), &["push", "-q", "-u", "origin", "feature"]);
        git(
            one.path(),
            &[
                "commit",
                "-q",
                "--amend",
                "--allow-empty",
                "-m",
                "mine, amended",
            ],
        );

        let status = read(one.path());
        assert_eq!(
            text(&status.sentence()),
            "On feature, 1 commit to push and 1 to pull."
        );
        let action = status.next_action().unwrap();
        assert_eq!(
            action,
            NextAction::ForcePush {
                commits: 1,
                replaced: 1
            }
        );

        run_next_action(one.path(), &action).unwrap();
        assert_eq!(read(one.path()).next_action(), None);
    }

    #[test]
    fn a_diverged_default_branch_is_left_to_a_person() {
        let (_remote, one, two) = cloned();
        commit(one.path(), "theirs");
        git(one.path(), &["push", "-q"]);
        commit(two.path(), "mine");
        fetch(two.path()).unwrap();

        let status = read(two.path());
        assert!(status.behind > 0 && status.ahead > 0);
        assert_eq!(status.next_action(), None);
    }

    #[test]
    fn a_rejected_push_reports_what_git_said() {
        let (_remote, one, two) = cloned();
        commit(one.path(), "theirs");
        git(one.path(), &["push", "-q"]);
        commit(two.path(), "mine");

        let error = run_next_action(two.path(), &NextAction::Push { commits: 1 }).unwrap_err();
        assert!(error.contains("rejected"), "{error}");
    }

    #[test]
    fn changed_files_are_counted_and_renames_count_once() {
        let (_remote, one, _two) = cloned();
        std::fs::write(one.path().join("a"), "content for the rename check").unwrap();
        git(one.path(), &["add", "a"]);
        commit(one.path(), "add a");
        git(one.path(), &["mv", "a", "b"]);
        std::fs::write(one.path().join("new"), "x").unwrap();

        let status = read(one.path());
        assert_eq!(status.changed, 2);
        assert!(text(&status.sentence()).ends_with("2 files changed."));
    }

    #[test]
    fn detached_and_unborn_heads_are_said_plainly() {
        let (_remote, one, _two) = cloned();
        git(one.path(), &["checkout", "-q", "--detach"]);
        let status = read(one.path());
        assert!(text(&status.sentence()).starts_with("HEAD is detached at "));
        assert_eq!(status.next_action(), None);

        let empty = TempDir::new().unwrap();
        git(empty.path(), &["init", "-q", "-b", "main"]);
        let status = read(empty.path());
        assert_eq!(text(&status.sentence()), "On main, no commits yet.");
        assert_eq!(status.next_action(), None);
    }
}
