//! What a forge's CI says about a commit, read through the API once signed in (see
//! `crate::auth`): GitHub Actions, GitLab pipelines and Gitea/Forgejo Actions. Git alone
//! can't say any of it, so nothing here exists without an account on the repository's
//! server.
//!
//! A commit has pipelines (GitHub calls them workflow runs), a pipeline has jobs, and a job
//! has a log. The graph's column wants one word per commit for many commits, so it reads
//! the newest pipelines in one call (`recent`); the inspector reads one commit in full
//! (`for_commit`) and a job's log on demand (`log`).
//!
//! Everything here blocks, so call it off the UI thread.

use super::forge::{Forge, Site};
use crate::auth::token::Keychain;
use crate::auth::transport::{Request, Transport, Web};
use crate::auth::{self, Account, Host};
use crate::cache::keys::Caches;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What the server's "no log" says between the server's name and the job's, so the sentence
/// and the memory of it can't drift apart.
const NO_LOG_MARKER: &str = " has no log for ";

/// How many of the newest pipelines the column reads, one call each page.
const RECENT: usize = 100;

/// Where one job stands, from the one most worth looking at (`Failed`) down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Failed,
    Running,
    Queued,
    Cancelled,
    Passed,
    Skipped,
}

impl Status {
    /// A forge's own words for it. GitHub says `status` and then `conclusion`, GitLab and
    /// Gitea say one word, so both are taken and the conclusion wins when there is one.
    pub fn from_words(status: &str, conclusion: Option<&str>) -> Self {
        let word = conclusion
            .filter(|word| !word.is_empty() && *word != "null")
            .unwrap_or(status)
            .to_ascii_lowercase();
        match word.as_str() {
            "success" | "passed" | "succeeded" => Self::Passed,
            "failure" | "failed" | "timed_out" | "error" | "startup_failure"
            | "action_required" => Self::Failed,
            "cancelled" | "canceled" | "stale" => Self::Cancelled,
            "skipped" | "neutral" => Self::Skipped,
            "in_progress" | "running" | "preparing" => Self::Running,
            // `completed` without a conclusion is as good as we can tell.
            "completed" => Self::Passed,
            _ => Self::Queued,
        }
    }

    /// The word for it, which carries the meaning the colour only helps with.
    pub fn label(self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Running => "Running",
            Self::Queued => "Waiting",
            Self::Cancelled => "Cancelled",
            Self::Passed => "Passed",
            Self::Skipped => "Skipped",
        }
    }

    /// Whether it is still going, so what is shown may change.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Queued)
    }

    /// The one status that stands for several: a failure beats everything, then what is
    /// still going, and a pass beats what was skipped.
    pub fn worst(statuses: impl IntoIterator<Item = Status>) -> Option<Status> {
        statuses.into_iter().min()
    }
}

/// One job of a pipeline, which has a log to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub name: String,
    /// GitLab's stage, which groups jobs. Empty elsewhere.
    pub group: String,
    pub status: Status,
    pub seconds: Option<u64>,
    /// The job's page on the forge.
    pub url: String,
}

/// A pipeline on a commit: a GitHub workflow run, a GitLab pipeline or a Gitea run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub id: String,
    pub name: String,
    pub status: Status,
    pub url: String,
    pub jobs: Vec<Job>,
}

impl Pipeline {
    /// `3 of 4 jobs passed`, or what the one status says when there are no jobs.
    pub fn summary(&self) -> String {
        let total = self.jobs.len();
        if total == 0 {
            return self.status.label().to_owned();
        }
        let count = |status| self.jobs.iter().filter(|job| job.status == status).count();
        match self.status {
            Status::Failed => format!("{} of {total} jobs failed", count(Status::Failed)),
            Status::Passed => format!("{total} jobs passed"),
            Status::Running | Status::Queued => format!(
                "{} of {total} jobs done",
                self.jobs.iter().filter(|j| !j.status.is_active()).count()
            ),
            other => other.label().to_owned(),
        }
    }
}

/// The server and repository CI is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: Host,
    pub forge: Forge,
    /// `owner/repo`, or `group/subgroup/repo`.
    pub path: String,
    /// The repository's page, which is where "open on the forge" goes.
    pub web: String,
}

/// Where this repository's CI can be read: its remote's server, when someone is signed in
/// there. `None` hides everything about CI, so a repository nobody can ask about shows no
/// empty column.
pub fn target(site: Option<&Site>, accounts: &[Account]) -> Option<Target> {
    let site = site?;
    let origin = site.origin();
    let account = accounts
        .iter()
        .find(|a| a.host.origin == origin && a.host.forge == site.forge)?;
    Some(Target {
        host: account.host.clone(),
        forge: site.forge,
        path: site.path().to_owned(),
        web: site.web(),
    })
}

/// Where CI would be read from, with nobody asked whether they are signed in there. The
/// keychain is slow (a call to the secret service for each server), and what was kept from
/// the last read needs only the address, so the interface draws that first and checks the
/// sign-in after with [`signed_in`].
pub fn target_unchecked(site: Option<&Site>, hosts: &[Host]) -> Option<Target> {
    let site = site?;
    let origin = site.origin();
    let host = hosts
        .iter()
        .find(|host| host.origin == origin && host.forge == site.forge)?;
    Some(Target {
        host: host.clone(),
        forge: site.forge,
        path: site.path().to_owned(),
        web: site.web(),
    })
}

/// Whether someone is signed in to the target's server. One keychain read, not one for each
/// server there is.
pub fn signed_in(target: &Target) -> bool {
    !auth::accounts_in(&Keychain, std::slice::from_ref(&target.host)).is_empty()
}

/// What a read of the forge can answer.
pub type Reply<T> = Result<T, String>;

fn get(target: &Target, http: &dyn Transport, token: &str, path: &str) -> Reply<Value> {
    let url = target.host.api(path);
    let reply = http.send(&Request {
        method: "GET",
        url: &url,
        bearer: Some(token),
        form: None,
        json: None,
        if_none_match: None,
    })?;
    check(target, reply.status)?;
    serde_json::from_str(&reply.body).map_err(|_| {
        format!(
            "{} didn't answer in a way Mantis Git understands.",
            target.host.name()
        )
    })
}

/// Words for the answers that mean "not for you", which are the ones a person can fix.
fn check(target: &Target, status: u16) -> Reply<()> {
    let name = target.host.name();
    match status {
        200..=299 => Ok(()),
        401 => Err(format!(
            "{name} didn't accept the sign-in. Sign in again in Settings → Accounts."
        )),
        403 => Err(format!(
            "{name} won't let this sign-in read CI. Sign in again and allow it."
        )),
        404 => Err(format!(
            "{name} has no CI for {} that this sign-in can see. It may be off for the \
             repository, or the server too old to have it.",
            target.path
        )),
        other => Err(format!("{name} answered {other}.")),
    }
}

/// A GitLab project is named in an address by its whole path, with the slashes encoded.
fn project(target: &Target) -> String {
    target.path.replace('/', "%2F")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn number(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn status_of(value: &Value) -> Status {
    let conclusion = value.get("conclusion").and_then(Value::as_str);
    Status::from_words(text(value, "status"), conclusion)
}

/// The newest pipelines, as the one status each commit stands for. A commit that has none
/// among them is missing, not passing: the column leaves it empty.
pub fn recent(target: &Target, http: &dyn Transport, token: &str) -> Reply<Newest> {
    let (path, list_key, sha_key) = match target.forge {
        Forge::GitHub => (
            format!("/repos/{}/actions/runs?per_page={RECENT}", target.path),
            Some("workflow_runs"),
            "head_sha",
        ),
        Forge::Gitea => (
            format!("/repos/{}/actions/runs?limit={RECENT}", target.path),
            Some("workflow_runs"),
            "head_sha",
        ),
        Forge::GitLab => (
            format!("/projects/{}/pipelines?per_page={RECENT}", project(target)),
            None,
            "sha",
        ),
    };
    let body = get(target, http, token, &path)?;
    let runs = match list_key {
        Some(key) => body.get(key),
        None => Some(&body),
    };
    let mut seen: HashMap<String, Vec<Status>> = HashMap::new();
    for run in runs.and_then(Value::as_array).into_iter().flatten() {
        let sha = text(run, sha_key);
        if !sha.is_empty() {
            seen.entry(sha.to_owned()).or_default().push(status_of(run));
        }
    }
    // Whether this is every pipeline the repository has, which is what makes a commit that
    // is missing from it a commit with none. GitHub and Gitea say how many there are; GitLab
    // doesn't, but never gives a page shorter than asked for unless that was all.
    let listed = runs.and_then(Value::as_array).map_or(0, Vec::len);
    let complete = match body.get("total_count").and_then(Value::as_u64) {
        Some(total) => listed as u64 >= total,
        None => target.forge == Forge::GitLab && listed < RECENT,
    };
    let statuses = seen
        .into_iter()
        .filter_map(|(sha, all)| Some((sha, Status::worst(all)?)))
        .collect();
    // When the oldest of them started, if every one says. Nothing older is in the list.
    let oldest = runs
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|run| parse_time(text(run, "created_at")))
        .collect::<Option<Vec<_>>>()
        .and_then(|times| times.into_iter().min());
    Ok(Newest {
        statuses,
        complete,
        oldest,
    })
}

/// How much newer than the oldest listed pipeline a commit must be for the list to be
/// trusted to hold its pipelines: clocks differ between an author and a server, and
/// `parse_time` ignores a time zone, so a day more than covers both.
const CLOCK_SLACK: u64 = 86_400;

/// What the newest pipelines say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Newest {
    /// The one status each commit among them stands for.
    pub statuses: HashMap<String, Status>,
    /// Whether they are all the repository has. Then a commit that isn't among them has no
    /// pipelines, and nobody needs asking.
    pub complete: bool,
    /// When the oldest of them was created, in seconds since the epoch, if all say.
    pub oldest: Option<u64>,
}

impl Newest {
    /// Whether a commit is known to have no pipelines without asking the forge. Either the
    /// list is all there is and the commit isn't in it, or the commit was made well after
    /// the oldest pipeline listed: its pipelines can't be older than it is, so they would be
    /// among these, being the newest. `committed` is seconds since the epoch.
    pub fn lacks(&self, sha: &str, committed: i64) -> bool {
        if self.statuses.contains_key(sha) {
            return false;
        }
        self.complete
            || self
                .oldest
                .zip(u64::try_from(committed).ok())
                .is_some_and(|(oldest, committed)| committed >= oldest.saturating_add(CLOCK_SLACK))
    }
}

/// Every pipeline on one commit, each with its jobs.
pub fn for_commit(
    target: &Target,
    http: &dyn Transport,
    token: &str,
    sha: &str,
) -> Reply<Vec<Pipeline>> {
    let runs = match target.forge {
        Forge::GitHub | Forge::Gitea => {
            let body = get(
                target,
                http,
                token,
                &format!("/repos/{}/actions/runs?head_sha={sha}", target.path),
            )?;
            let all = body.get("workflow_runs").and_then(Value::as_array);
            // Gitea has ignored the filter on some versions, so it is applied here too.
            all.into_iter()
                .flatten()
                .filter(|run| text(run, "head_sha") == sha)
                .cloned()
                .collect::<Vec<_>>()
        }
        Forge::GitLab => {
            let body = get(
                target,
                http,
                token,
                &format!("/projects/{}/pipelines?sha={sha}", project(target)),
            )?;
            body.as_array().cloned().unwrap_or_default()
        }
    };
    let mut pipelines = Vec::new();
    for run in runs {
        let id = number(&run, "id");
        let jobs = jobs_of(target, http, token, &id)?;
        let name = match target.forge {
            Forge::GitLab => format!("Pipeline #{id}"),
            _ => ["display_title", "name", "title"]
                .iter()
                .map(|key| text(&run, key))
                .find(|word| !word.is_empty())
                .unwrap_or("Workflow")
                .to_owned(),
        };
        let url = match text(&run, "html_url") {
            "" => text(&run, "web_url").to_owned(),
            url => url.to_owned(),
        };
        pipelines.push(Pipeline {
            status: status_of(&run),
            id,
            name,
            url,
            jobs,
        });
    }
    // Failures first, so the thing to look at is at the top.
    pipelines.sort_by_key(|pipeline| pipeline.status);
    Ok(pipelines)
}

fn jobs_of(target: &Target, http: &dyn Transport, token: &str, run: &str) -> Reply<Vec<Job>> {
    let (path, key) = match target.forge {
        Forge::GitHub | Forge::Gitea => (
            format!("/repos/{}/actions/runs/{run}/jobs", target.path),
            Some("jobs"),
        ),
        Forge::GitLab => (
            format!(
                "/projects/{}/pipelines/{run}/jobs?per_page=100",
                project(target)
            ),
            None,
        ),
    };
    let body = get(target, http, token, &path)?;
    let list = match key {
        Some(key) => body.get(key),
        None => Some(&body),
    };
    Ok(list
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|job| Job {
            id: number(job, "id"),
            name: text(job, "name").to_owned(),
            group: text(job, "stage").to_owned(),
            status: status_of(job),
            seconds: seconds_of(job),
            url: match text(job, "html_url") {
                "" => text(job, "web_url").to_owned(),
                url => url.to_owned(),
            },
        })
        .collect())
}

/// How long a job ran: GitLab says, the others give two times.
fn seconds_of(job: &Value) -> Option<u64> {
    if let Some(seconds) = job.get("duration").and_then(Value::as_f64) {
        return Some(seconds.round() as u64);
    }
    let started = parse_time(text(job, "started_at"))?;
    let finished = parse_time(text(job, "completed_at"))?;
    finished.checked_sub(started)
}

/// Seconds since the epoch of `2024-05-01T12:30:45Z`, ignoring any fraction and offset.
/// Only the difference of two of them is ever used.
fn parse_time(word: &str) -> Option<u64> {
    let (date, time) = word.split_once('T')?;
    let mut d = date.split('-').map(|n| n.parse::<i64>().ok());
    let (year, month, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.split(['Z', '+', '.']).next()?;
    let mut t = time.split(':').map(|n| n.parse::<i64>().ok());
    let (h, m, s) = (t.next()??, t.next()??, t.next()??);
    // Days from civil date, so month lengths and leap years are right.
    let (y, mo) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + m * 60 + s).ok()
}

/// How long a job took, as short as it can be said: `45s`, `3m 20s`.
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// A job's log as lines, cleaned for reading: no colour codes, no progress rewrites and no
/// time stamp in front of each line.
pub fn log(target: &Target, http: &dyn Transport, token: &str, job: &Job) -> Reply<Vec<String>> {
    let path = match target.forge {
        Forge::GitHub | Forge::Gitea => {
            format!("/repos/{}/actions/jobs/{}/logs", target.path, job.id)
        }
        Forge::GitLab => format!("/projects/{}/jobs/{}/trace", project(target), job.id),
    };
    let url = target.host.api(&path);
    let reply = http.send(&Request {
        method: "GET",
        url: &url,
        bearer: Some(token),
        form: None,
        json: None,
        if_none_match: None,
    })?;
    if reply.status == 404 {
        return Err(format!(
            "{}{NO_LOG_MARKER}{}. It may not have started, or the server keeps logs elsewhere.",
            target.host.name(),
            job.name
        ));
    }
    check(target, reply.status)?;
    Ok(clean_log(&reply.body))
}

/// Takes a raw log apart into lines worth reading.
pub fn clean_log(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| {
            // A progress bar rewrites its line with `\r`; only the last write is on screen.
            let line = line
                .rsplit('\r')
                .find(|part| !part.is_empty())
                .unwrap_or("");
            let line = strip_ansi(line);
            strip_timestamp(&line).to_owned()
        })
        .collect()
}

/// Removes `ESC [ … letter` sequences, which colour a terminal and mean nothing here.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// GitHub and Gitea put `2024-05-01T12:30:45.1234567Z ` in front of every line.
fn strip_timestamp(line: &str) -> &str {
    let bytes = line.as_bytes();
    let stamped = bytes.len() > 21
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[..4].iter().all(u8::is_ascii_digit);
    if !stamped {
        return line;
    }
    match line.find("Z ") {
        Some(end) if end < 40 => &line[end + 2..],
        _ => line,
    }
}

/// The sentence over the column's tooltip and the Checks tab's heading when a commit has
/// nothing.
pub fn none_sentence(forge: Forge) -> String {
    match forge {
        Forge::GitLab => "No pipelines ran on this commit.".to_owned(),
        _ => "No workflows ran on this commit.".to_owned(),
    }
}

// Ways in with the real server and the system keychain, for the interface.
//
// Every read goes through the page store (`Revalidating`), which keeps each answer with its
// `ETag`, running or finished, in memory and on disk. So the interface can draw from the
// store the moment it opens (`*_stored`, which never touches the network) and then ask the
// server (`*_live`), which is a 304 when nothing moved.

use crate::auth::transport::{Revalidating, Stored};
use std::sync::OnceLock;

fn web(caches: &Caches) -> &'static Revalidating<Web> {
    static WEB: OnceLock<Revalidating<Web>> = OnceLock::new();
    WEB.get_or_init(|| Revalidating::new(Web, caches.pages()))
}

fn session(target: &Target) -> Reply<String> {
    Ok(auth::fresh_token(&target.host, &Keychain, &Web)?.access)
}

/// `recent` against the real server, as whoever is signed in.
pub fn recent_live(target: &Target, caches: &Caches) -> Reply<Newest> {
    recent(target, web(caches), &session(target)?)
}

/// `recent` as the store last had it. Asks nobody, so it is quick; `Err` when it has never
/// been read.
pub fn recent_stored(target: &Target, caches: &Caches) -> Reply<Newest> {
    recent(target, &Stored::new(caches.pages()), "")
}

/// `for_commit` against the real server.
pub fn for_commit_live(target: &Target, caches: &Caches, sha: &str) -> Reply<Vec<Pipeline>> {
    for_commit(target, web(caches), &session(target)?, sha)
}

/// `for_commit` as the store last had it, which is only whole when every page it needs is
/// there. Asks nobody.
pub fn for_commit_stored(target: &Target, caches: &Caches, sha: &str) -> Reply<Vec<Pipeline>> {
    for_commit(target, &Stored::new(caches.pages()), "", sha)
}

/// How long a server's "no log" is believed. A job that is waiting gets one when it starts,
/// which is a change of status, and that asks again at once.
const NO_LOG_FOR: Duration = Duration::from_secs(60);

/// A job's log. One that is over never changes, so a kept copy is the answer and nobody is
/// asked; a running one is asked each time. A job that is waiting or was skipped has no log
/// and is not asked at all, and a server that says a job has none is not asked again about
/// it, in the status it is in, for a minute, so opening it again is instant.
pub fn log_live(target: &Target, caches: &Caches, job: &Job) -> Reply<Vec<String>> {
    if matches!(job.status, Status::Queued | Status::Skipped) {
        return Ok(Vec::new());
    }
    if !job.status.is_active()
        && let Ok(kept) = log(target, &Stored::new(caches.pages()), "", job)
    {
        return Ok(kept);
    }
    static MISSING: LazyLock<Mutex<HashMap<String, (Instant, String)>>> =
        LazyLock::new(Mutex::default);
    let key = format!(
        "{}/{}/{}/{:?}",
        target.host.origin, target.path, job.id, job.status
    );
    let said = |map: &HashMap<String, (Instant, String)>| {
        map.get(&key)
            .filter(|(at, _)| at.elapsed() < NO_LOG_FOR)
            .map(|(_, message)| message.clone())
    };
    if let Some(message) = said(&MISSING.lock().unwrap_or_else(PoisonError::into_inner)) {
        return Err(message);
    }
    let result = log(target, web(caches), &session(target)?, job);
    if let Err(message) = &result
        && message.contains(NO_LOG_MARKER)
    {
        MISSING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, (Instant::now(), message.clone()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::transport::fake::Fake;

    fn target(forge: Forge, origin: &str) -> Target {
        Target {
            host: Host::new(forge, origin, "id").unwrap(),
            forge,
            path: "ada/tool".into(),
            web: format!("{origin}/ada/tool"),
        }
    }

    #[test]
    fn a_forges_words_become_one_status() {
        use Status::*;
        let cases = [
            ("completed", Some("success"), Passed),
            ("completed", Some("failure"), Failed),
            ("completed", Some("timed_out"), Failed),
            ("completed", Some("cancelled"), Cancelled),
            ("completed", Some("skipped"), Skipped),
            ("in_progress", None, Running),
            ("in_progress", Some("null"), Running),
            ("queued", None, Queued),
            ("success", None, Passed),
            ("failed", None, Failed),
            ("canceled", None, Cancelled),
            ("manual", None, Queued),
            ("waiting", None, Queued),
        ];
        for (status, conclusion, expect) in cases {
            assert_eq!(Status::from_words(status, conclusion), expect, "{status}");
        }
    }

    #[test]
    fn a_failure_stands_for_the_rest() {
        use Status::*;
        assert_eq!(Status::worst([Passed, Failed, Running]), Some(Failed));
        assert_eq!(Status::worst([Passed, Running]), Some(Running));
        assert_eq!(Status::worst([Skipped, Passed]), Some(Passed));
        assert_eq!(Status::worst([]), None);
    }

    #[test]
    fn ci_is_only_read_where_someone_is_signed_in() {
        use crate::repo::forge::{self, Hosting};
        use crate::repo::remote::Remote;
        let remote = Remote {
            name: "origin".into(),
            url: "git@github.com:ada/tool.git".into(),
            push_url: None,
        };
        let Some(Hosting::Known(site)) = forge::hosting(&remote, None) else {
            panic!("github.com is known by its name");
        };
        let host = Host::new(Forge::GitHub, "https://github.com", "id").unwrap();
        let account = Account {
            host,
            login: "ada".into(),
        };
        assert!(super::target(Some(&site), &[]).is_none());
        let other = Account {
            host: Host::new(Forge::GitLab, "https://gitlab.com", "id").unwrap(),
            login: "ada".into(),
        };
        assert!(super::target(Some(&site), &[other]).is_none());
        let found = super::target(Some(&site), &[account]).unwrap();
        assert_eq!(found.path, "ada/tool");
        assert_eq!(found.web, "https://github.com/ada/tool");
        assert!(super::target(None, &[]).is_none());
    }

    #[test]
    fn github_runs_are_read_per_commit() {
        let body = r#"{"workflow_runs":[
            {"head_sha":"aaa","status":"completed","conclusion":"success"},
            {"head_sha":"aaa","status":"completed","conclusion":"failure"},
            {"head_sha":"bbb","status":"in_progress","conclusion":null}]}"#;
        let http = Fake::answering(&[(200, body)]);
        let found = recent(&target(Forge::GitHub, "https://github.com"), &http, "t").unwrap();
        assert_eq!(found.statuses["aaa"], Status::Failed);
        assert_eq!(found.statuses["bbb"], Status::Running);
        assert!(
            http.seen.lock().unwrap()[0]
                .starts_with("GET https://api.github.com/repos/ada/tool/actions/runs?per_page=100")
        );
    }

    #[test]
    fn the_list_is_complete_only_when_the_forge_says_nothing_is_left_out() {
        let run = r#"{"head_sha":"aaa","status":"completed","conclusion":"success"}"#;
        let github = |total: u32| {
            let body = format!(r#"{{"total_count":{total},"workflow_runs":[{run}]}}"#);
            let http = Fake::answering(&[(200, &body)]);
            recent(&target(Forge::GitHub, "https://github.com"), &http, "t")
                .unwrap()
                .complete
        };
        assert!(github(1), "it listed every run there is");
        assert!(!github(250), "there are older runs it didn't list");

        // Without a count, only GitLab's short page says it was all.
        let gitea = Fake::answering(&[(200, &format!(r#"{{"workflow_runs":[{run}]}}"#))]);
        let found = recent(&target(Forge::Gitea, "https://codeberg.org"), &gitea, "t");
        assert!(!found.unwrap().complete, "Gitea may cap a page below ours");
        let gitlab = Fake::answering(&[(200, r#"[{"sha":"aaa","status":"success"}]"#)]);
        let found = recent(&target(Forge::GitLab, "https://gitlab.com"), &gitlab, "t");
        assert!(found.unwrap().complete);
        let full: Vec<String> = (0..RECENT)
            .map(|n| format!(r#"{{"sha":"s{n}","status":"success"}}"#))
            .collect();
        let gitlab = Fake::answering(&[(200, &format!("[{}]", full.join(",")))]);
        let found = recent(&target(Forge::GitLab, "https://gitlab.com"), &gitlab, "t");
        assert!(
            !found.unwrap().complete,
            "a full page may have more behind it"
        );
    }

    #[test]
    fn a_commit_newer_than_the_whole_list_reaches_back_has_none() {
        // 250 runs exist and the newest two are listed, the older made on 2024-05-01.
        let body = r#"{"total_count":250,"workflow_runs":[
            {"head_sha":"aaa","status":"completed","conclusion":"success","created_at":"2024-05-03T10:00:00Z"},
            {"head_sha":"bbb","status":"completed","conclusion":"success","created_at":"2024-05-01T12:00:00Z"}]}"#;
        let http = Fake::answering(&[(200, body)]);
        let newest = recent(&target(Forge::GitHub, "https://github.com"), &http, "t").unwrap();
        assert!(!newest.complete);
        let oldest = parse_time("2024-05-01T12:00:00Z").unwrap() as i64;
        let day = 86_400;
        assert!(
            newest.lacks("new", oldest + day),
            "made a day after the oldest listed"
        );
        assert!(
            !newest.lacks("new", oldest + day - 1),
            "inside the clock slack"
        );
        assert!(
            !newest.lacks("old", oldest - 10 * day),
            "may be among the older ones"
        );
        assert!(!newest.lacks("aaa", oldest + 5 * day), "it is listed");
        assert!(!newest.lacks("new", -1), "a date before 1970 says nothing");

        // One run without a time and nothing is known of how far back the list goes.
        let body = r#"{"total_count":250,"workflow_runs":[
            {"head_sha":"aaa","status":"completed","conclusion":"success","created_at":"2024-05-03T10:00:00Z"},
            {"head_sha":"bbb","status":"completed","conclusion":"success"}]}"#;
        let http = Fake::answering(&[(200, body)]);
        let newest = recent(&target(Forge::GitHub, "https://github.com"), &http, "t").unwrap();
        assert_eq!(newest.oldest, None);
        assert!(!newest.lacks("new", oldest + 100 * day));
    }

    #[test]
    fn gitlab_pipelines_use_the_encoded_project() {
        let http = Fake::answering(&[(200, r#"[{"sha":"aaa","status":"success"}]"#)]);
        let mut gitlab = target(Forge::GitLab, "https://gitlab.com");
        gitlab.path = "group/sub/tool".into();
        let found = recent(&gitlab, &http, "t").unwrap();
        assert_eq!(found.statuses["aaa"], Status::Passed);
        assert!(
            http.seen.lock().unwrap()[0].contains("/api/v4/projects/group%2Fsub%2Ftool/pipelines")
        );
    }

    #[test]
    fn a_commit_lists_pipelines_with_their_jobs_failures_first() {
        let runs = r#"{"workflow_runs":[
            {"id":1,"name":"lint","head_sha":"aaa","status":"completed","conclusion":"success","html_url":"https://x/1"},
            {"id":2,"name":"test","head_sha":"aaa","status":"completed","conclusion":"failure","html_url":"https://x/2"},
            {"id":3,"name":"other","head_sha":"zzz","status":"completed","conclusion":"success"}]}"#;
        let jobs_1 = r#"{"jobs":[{"id":10,"name":"clippy","status":"completed","conclusion":"success","started_at":"2024-05-01T12:00:00Z","completed_at":"2024-05-01T12:01:30Z","html_url":"https://x/j10"}]}"#;
        let jobs_2 = r#"{"jobs":[{"id":20,"name":"unit","status":"completed","conclusion":"failure"},{"id":21,"name":"e2e","status":"queued"}]}"#;
        let http = Fake::answering(&[(200, runs), (200, jobs_1), (200, jobs_2)]);
        let found = for_commit(
            &target(Forge::GitHub, "https://github.com"),
            &http,
            "t",
            "aaa",
        )
        .unwrap();
        assert_eq!(found.len(), 2, "a run on another commit is left out");
        assert_eq!(found[0].name, "test");
        assert_eq!(found[0].summary(), "1 of 2 jobs failed");
        assert_eq!(found[0].jobs[1].status, Status::Queued);
        assert_eq!(found[1].jobs[0].seconds, Some(90));
        assert_eq!(found[1].summary(), "1 jobs passed");
        assert_eq!(found[1].jobs[0].url, "https://x/j10");
    }

    #[test]
    fn gitlab_jobs_carry_their_stage_and_duration() {
        let pipelines = r#"[{"id":7,"sha":"aaa","status":"failed","web_url":"https://g/7"}]"#;
        let jobs = r#"[{"id":70,"name":"rspec","stage":"test","status":"failed","duration":12.6,"web_url":"https://g/j70"}]"#;
        let http = Fake::answering(&[(200, pipelines), (200, jobs)]);
        let found = for_commit(
            &target(Forge::GitLab, "https://gitlab.com"),
            &http,
            "t",
            "aaa",
        )
        .unwrap();
        assert_eq!(found[0].name, "Pipeline #7");
        assert_eq!(found[0].jobs[0].group, "test");
        assert_eq!(found[0].jobs[0].seconds, Some(13));
    }

    #[test]
    fn what_the_person_can_fix_is_said() {
        let t = target(Forge::GitHub, "https://github.com");
        for (status, expect) in [(401, "Sign in again"), (403, "allow it"), (404, "no CI")] {
            let http = Fake::answering(&[(status, "{}")]);
            let err = recent(&t, &http, "t").unwrap_err();
            assert!(err.contains(expect), "{err}");
        }
    }

    #[test]
    fn a_log_reads_without_colour_stamps_or_progress() {
        let raw = "2024-05-01T12:00:00.1234567Z Run cargo test\r\n\
                   2024-05-01T12:00:01.0000000Z \u{1b}[31merror\u{1b}[0m: boom\n\
                   10%\r50%\r100%\n\
                   plain line";
        assert_eq!(
            clean_log(raw),
            ["Run cargo test", "error: boom", "100%", "plain line"]
        );
    }

    #[test]
    fn a_missing_log_says_why() {
        let t = target(Forge::Gitea, "https://codeberg.org");
        let job = Job {
            id: "5".into(),
            name: "build".into(),
            group: String::new(),
            status: Status::Queued,
            seconds: None,
            url: String::new(),
        };
        let http = Fake::answering(&[(404, "")]);
        assert!(
            log(&t, &http, "t", &job)
                .unwrap_err()
                .contains("no log for build")
        );
        let http = Fake::answering(&[(200, "a\nb")]);
        assert_eq!(log(&t, &http, "t", &job).unwrap(), ["a", "b"]);
        assert!(
            http.seen.lock().unwrap()[0].contains("/api/v1/repos/ada/tool/actions/jobs/5/logs")
        );
    }

    #[test]
    fn what_was_read_once_is_there_at_the_next_start_with_nobody_asked() {
        use crate::auth::transport::{Revalidating, Stored};
        use crate::cache::DiskSettings;
        let root = tempfile::tempdir().unwrap();
        let t = target(Forge::GitHub, "https://github.com");
        let runs = r#"{"workflow_runs":[
            {"id":1,"name":"ci","head_sha":"aaa","status":"in_progress","conclusion":null}]}"#;
        let jobs = r#"{"jobs":[{"id":10,"name":"unit","status":"in_progress"}]}"#;

        // A first run reads the server. The answers are about something still running, and
        // are kept all the same.
        let caches = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let http = Revalidating::new(Fake::answering(&[(200, runs), (200, jobs)]), caches.pages());
        let live = for_commit(&t, &http, "t", "aaa").unwrap();
        assert_eq!(live[0].status, Status::Running);
        caches.pages().flush();

        // The next run draws it before asking anyone, in a store that was never told of it.
        let again = Caches::on_disk(root.path().to_owned(), DiskSettings::default());
        let stored = Stored::new(again.pages());
        assert_eq!(for_commit(&t, &stored, "", "aaa").unwrap(), live);
        assert!(for_commit(&t, &stored, "", "bbb").is_err(), "never read");
    }

    #[test]
    fn a_job_that_has_not_run_has_no_log_and_nobody_is_asked() {
        // No server and no keychain here, so anything asked would be an error.
        let caches = Caches::memory_only();
        let t = target(Forge::GitHub, "https://github.com");
        for status in [Status::Queued, Status::Skipped] {
            let job = Job {
                id: "5".into(),
                name: "build".into(),
                group: String::new(),
                status,
                seconds: None,
                url: String::new(),
            };
            assert_eq!(log_live(&t, &caches, &job), Ok(Vec::new()), "{status:?}");
        }
    }

    #[test]
    fn times_and_durations_read_short() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(200), "3m 20s");
        assert_eq!(duration(3700), "1h 1m");
        let a = parse_time("2024-02-28T23:59:30Z").unwrap();
        let b = parse_time("2024-03-01T00:00:10Z").unwrap();
        assert_eq!(b - a, 86_400 + 40, "2024 has a 29th of February");
    }
}
