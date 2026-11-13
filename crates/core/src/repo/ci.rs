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
use serde_json::Value;
use std::collections::HashMap;

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
pub fn recent(
    target: &Target,
    http: &dyn Transport,
    token: &str,
) -> Reply<HashMap<String, Status>> {
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
    Ok(seen
        .into_iter()
        .filter_map(|(sha, all)| Some((sha, Status::worst(all)?)))
        .collect())
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
    })?;
    if reply.status == 404 {
        return Err(format!(
            "{} has no log for {}. It may not have started, or the server keeps logs elsewhere.",
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

fn session(target: &Target) -> Reply<String> {
    Ok(auth::fresh_token(&target.host, &Keychain, &Web)?.access)
}

/// `recent` against the real server, as whoever is signed in.
pub fn recent_live(target: &Target) -> Reply<HashMap<String, Status>> {
    recent(target, &Web, &session(target)?)
}

/// `for_commit` against the real server.
pub fn for_commit_live(target: &Target, sha: &str) -> Reply<Vec<Pipeline>> {
    for_commit(target, &Web, &session(target)?, sha)
}

/// `log` against the real server.
pub fn log_live(target: &Target, job: &Job) -> Reply<Vec<String>> {
    log(target, &Web, &session(target)?, job)
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
        assert_eq!(found["aaa"], Status::Failed);
        assert_eq!(found["bbb"], Status::Running);
        assert!(
            http.seen.lock().unwrap()[0]
                .starts_with("GET https://api.github.com/repos/ada/tool/actions/runs?per_page=100")
        );
    }

    #[test]
    fn gitlab_pipelines_use_the_encoded_project() {
        let http = Fake::answering(&[(200, r#"[{"sha":"aaa","status":"success"}]"#)]);
        let mut gitlab = target(Forge::GitLab, "https://gitlab.com");
        gitlab.path = "group/sub/tool".into();
        let found = recent(&gitlab, &http, "t").unwrap();
        assert_eq!(found["aaa"], Status::Passed);
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
    fn times_and_durations_read_short() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(200), "3m 20s");
        assert_eq!(duration(3700), "1h 1m");
        let a = parse_time("2024-02-28T23:59:30Z").unwrap();
        let b = parse_time("2024-03-01T00:00:10Z").unwrap();
        assert_eq!(b - a, 86_400 + 40, "2024 has a 29th of February");
    }
}
