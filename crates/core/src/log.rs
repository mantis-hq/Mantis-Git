//! A record of every git command the app runs: the command as it was put to git, its
//! output as it arrives, and how it ended. The log dock draws it, so that someone
//! waiting on a slow command can see what git is doing.
//!
//! The log is shared by the whole process. Writers are any thread; readers call
//! [`read`] and ask to be woken with [`subscribe`].

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Commands kept. Older ones are dropped first.
const MAX_ENTRIES: usize = 50;
/// Tests share the one log while running side by side, and each reads back what it wrote.
/// Trimming is tested on a log of its own, with the real limit.
const KEPT: usize = if cfg!(test) { 1_000_000 } else { MAX_ENTRIES };
/// Lines kept per command, so that a huge diff can't fill memory.
const MAX_LINES: usize = 1000;
/// Characters kept per line.
const MAX_LINE: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
    /// Something the app says about the command, such as how much it read.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub stream: Stream,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Done {
        success: bool,
        /// How it ended in words, such as "exit 128".
        status: String,
        took: Duration,
    },
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub repo: PathBuf,
    /// The command line as the app ran it.
    pub command: String,
    /// What the command does, without the repository it ran in: "status --short".
    pub title: String,
    pub started: Instant,
    pub state: State,
    pub lines: Vec<Line>,
    /// Lines past the limit, which were not kept.
    pub dropped: usize,
}

impl Entry {
    pub fn is_running(&self) -> bool {
        self.state == State::Running
    }

    /// How long it has run, or ran for.
    pub fn elapsed(&self) -> Duration {
        match &self.state {
            State::Running => self.started.elapsed(),
            State::Done { took, .. } => *took,
        }
    }

    /// A short form for a list: "running", how long it took, or why it failed.
    pub fn brief(&self) -> String {
        match &self.state {
            State::Running => "running".to_owned(),
            State::Done {
                success: true,
                took,
                ..
            } => format_duration(*took),
            State::Done { status, .. } => status.clone(),
        }
    }

    /// Where the command stands, in words: "running · 12s" or "exit 128 · 0.2s".
    pub fn summary(&self) -> String {
        let took = format_duration(self.elapsed());
        match &self.state {
            State::Running => format!("running · {took}"),
            State::Done { status, .. } => format!("{status} · {took}"),
        }
    }
}

/// A duration as it reads in a log: "0.2s", "12s", "1m 05s".
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else if seconds >= 10 {
        format!("{seconds}s")
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[derive(Default)]
struct Log {
    next_id: u64,
    entries: Vec<Entry>,
    listeners: Vec<async_channel::Sender<()>>,
}

static LOG: Mutex<Log> = Mutex::new(Log {
    next_id: 0,
    entries: Vec::new(),
    listeners: Vec::new(),
});

fn with<T>(f: impl FnOnce(&mut Log) -> T) -> T {
    let mut log = LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut log)
}

fn wake(log: &mut Log) {
    // A full channel already has a wake-up waiting, so a failed send loses nothing.
    log.listeners
        .retain(|listener| match listener.try_send(()) {
            Ok(()) => true,
            Err(error) => !error.is_closed(),
        });
}

/// Wakes when anything in the log changes. Changes that arrive together wake once.
pub fn subscribe() -> async_channel::Receiver<()> {
    let (sender, receiver) = async_channel::bounded(1);
    with(|log| log.listeners.push(sender));
    receiver
}

/// Reads the log, oldest first, without copying it.
pub fn read<T>(f: impl FnOnce(&[Entry]) -> T) -> T {
    with(|log| f(&log.entries))
}

/// Forgets every entry that has finished. Running commands stay, so that they can still
/// be watched.
pub fn clear() {
    with(|log| {
        log.clear_finished();
        wake(log);
    });
}

impl Log {
    fn clear_finished(&mut self) {
        self.entries.retain(Entry::is_running);
    }

    /// Keeps the newest `max`, dropping the oldest finished ones first so that a command
    /// still running is never the one forgotten.
    fn trim(&mut self, max: usize) {
        let mut extra = self.entries.len().saturating_sub(max);
        self.entries.retain(|entry| {
            if extra > 0 && !entry.is_running() {
                extra -= 1;
                return false;
            }
            true
        });
    }
}

/// A command being recorded.
#[derive(Clone, Copy, Debug)]
pub struct Handle(u64);

/// Starts recording `command`, which is shown exactly as it will run.
pub fn start(command: &Command) -> Handle {
    let repo = repository_of(command);
    let text = format_command(command);
    let title = title_of(command);
    with(|log| {
        let id = log.next_id;
        log.next_id += 1;
        log.entries.push(Entry {
            id,
            repo,
            command: text,
            title,
            started: Instant::now(),
            state: State::Running,
            lines: Vec::new(),
            dropped: 0,
        });
        log.trim(KEPT);
        wake(log);
        Handle(id)
    })
}

impl Handle {
    fn update(self, f: impl FnOnce(&mut Entry)) {
        with(|log| {
            if let Some(entry) = log.entries.iter_mut().rev().find(|e| e.id == self.0) {
                f(entry);
                wake(log);
            }
        });
    }

    pub fn line(self, stream: Stream, text: &str) {
        let text: String = text
            .trim_end_matches(['\n', '\r'])
            .chars()
            .take(MAX_LINE)
            .collect();
        // Progress meters redraw one line with carriage returns. Only the last draw is
        // what git is saying now.
        let text = text.rsplit('\r').next().unwrap_or_default().to_owned();
        self.update(|entry| {
            if entry.lines.len() < MAX_LINES {
                entry.lines.push(Line { stream, text });
            } else {
                entry.dropped += 1;
            }
        });
    }

    pub fn note(self, text: &str) {
        self.line(Stream::Note, text);
    }

    /// Records how the command ended. Only the first call counts.
    pub fn finish(self, success: bool, status: impl Into<String>) {
        let status = status.into();
        self.update(|entry| {
            if entry.is_running() {
                entry.state = State::Done {
                    success,
                    status,
                    took: entry.started.elapsed(),
                };
            }
        });
    }
}

/// The repository a git command runs in, which `ops::git` puts after `-C`.
fn repository_of(command: &Command) -> PathBuf {
    let mut args = command.get_args();
    match (args.next(), args.next()) {
        (Some(flag), Some(path)) if flag == "-C" => PathBuf::from(path),
        _ => command
            .get_current_dir()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    }
}

/// The command as a shell would take it: arguments with spaces or other special
/// characters are quoted, everything else is as given.
pub fn format_command(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| quote(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The command without `git -C <repo>`, which every command in the log shares.
fn title_of(command: &Command) -> String {
    let mut args = command.get_args().peekable();
    if args.peek().is_some_and(|flag| *flag == "-C") {
        args.nth(1);
    }
    args.map(|arg| quote(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./=:@%+,^~".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(repo: &str, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(repo).args(args);
        command
    }

    fn entry_in(repo: &str) -> Entry {
        read(|entries| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.repo == Path::new(repo))
                .cloned()
                .expect("entry is logged")
        })
    }

    #[test]
    fn a_command_is_shown_as_it_runs_with_awkward_arguments_quoted() {
        let command = command(
            "/my repo",
            &["commit", "-m", "it's done", "--format=%H%x00"],
        );
        assert_eq!(
            format_command(&command),
            r"git -C '/my repo' commit -m 'it'\''s done' --format=%H%x00"
        );
    }

    #[test]
    fn the_title_leaves_out_where_the_command_ran() {
        let repo = "/log-test/title";
        start(&command(repo, &["commit", "-m", "two words"]));
        let entry = entry_in(repo);
        assert_eq!(entry.title, "commit -m 'two words'");
        assert!(entry.command.starts_with("git -C /log-test/title commit"));
    }

    #[test]
    fn output_arrives_while_the_command_is_running_and_the_end_is_recorded() {
        let repo = "/log-test/output";
        let handle = start(&command(repo, &["fetch"]));
        handle.line(Stream::Err, "remote: Counting objects\n");
        let entry = entry_in(repo);
        assert!(entry.is_running());
        assert_eq!(entry.repo, Path::new(repo));
        assert_eq!(entry.command, format!("git -C {repo} fetch"));
        assert_eq!(entry.lines[0].text, "remote: Counting objects");

        handle.finish(false, "exit 128");
        handle.finish(true, "ignored");
        let State::Done {
            success, status, ..
        } = entry_in(repo).state
        else {
            panic!("finished");
        };
        assert!(!success);
        assert_eq!(status, "exit 128");
    }

    #[test]
    fn a_progress_meter_keeps_only_its_latest_draw() {
        let repo = "/log-test/progress";
        let handle = start(&command(repo, &["clone"]));
        handle.line(
            Stream::Err,
            "Receiving: 10%\rReceiving: 60%\rReceiving: 100%\n",
        );
        assert_eq!(entry_in(repo).lines[0].text, "Receiving: 100%");
    }

    #[test]
    fn lines_past_the_limit_are_counted_not_kept() {
        let repo = "/log-test/flood";
        let handle = start(&command(repo, &["diff"]));
        for _ in 0..MAX_LINES + 5 {
            handle.line(Stream::Out, "x");
        }
        let entry = entry_in(repo);
        assert_eq!(entry.lines.len(), MAX_LINES);
        assert_eq!(entry.dropped, 5);
    }

    #[test]
    fn durations_read_naturally() {
        let ms = Duration::from_millis;
        assert_eq!(format_duration(ms(240)), "0.2s");
        assert_eq!(format_duration(ms(12_900)), "12s");
        assert_eq!(format_duration(ms(65_000)), "1m 05s");
    }

    #[test]
    fn the_summary_says_whether_it_is_still_running() {
        let repo = "/log-test/summary";
        let handle = start(&command(repo, &["push"]));
        assert!(entry_in(repo).summary().starts_with("running · "));
        assert_eq!(entry_in(repo).brief(), "running");
        handle.finish(false, "exit 1");
        assert!(entry_in(repo).summary().starts_with("exit 1 · "));
        assert_eq!(entry_in(repo).brief(), "exit 1", "a failure says why");
    }

    #[test]
    fn subscribers_are_woken_by_changes() {
        let wakes = subscribe();
        start(&command("/log-test/wake", &["status"]));
        assert!(wakes.try_recv().is_ok());
    }

    #[test]
    fn only_the_last_fifty_commands_are_kept_and_a_running_one_is_never_dropped() {
        let entry = |id: u64, state| Entry {
            id,
            repo: PathBuf::new(),
            command: String::new(),
            title: String::new(),
            started: Instant::now(),
            state,
            lines: Vec::new(),
            dropped: 0,
        };
        let done = || State::Done {
            success: true,
            status: "exit 0".into(),
            took: Duration::ZERO,
        };
        let mut log = Log::default();
        // The oldest is still running, as a slow fetch would be.
        log.entries.push(entry(0, State::Running));
        log.entries.extend((1..=60).map(|id| entry(id, done())));
        log.trim(MAX_ENTRIES);
        let ids: Vec<u64> = log.entries.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), MAX_ENTRIES);
        assert_eq!(ids[0], 0, "the running command stays");
        assert_eq!(*ids.last().unwrap(), 60, "the newest stay");
        assert_eq!(ids[1], 12, "the oldest finished ones went");
    }

    #[test]
    fn clearing_forgets_what_finished_and_keeps_what_is_running() {
        // On a log of its own: clearing the shared one would race the other tests.
        let entry = |state| Entry {
            id: 0,
            repo: PathBuf::new(),
            command: String::new(),
            title: String::new(),
            started: Instant::now(),
            state,
            lines: Vec::new(),
            dropped: 0,
        };
        let done = State::Done {
            success: true,
            status: "exit 0".into(),
            took: Duration::ZERO,
        };
        let mut log = Log::default();
        log.entries = vec![entry(done), entry(State::Running)];
        log.clear_finished();
        assert_eq!(log.entries.len(), 1);
        assert!(log.entries[0].is_running());
    }
}
