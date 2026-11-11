//! Runs the `git` executable. Every git process in the app starts here, and so every one
//! is recorded in [`crate::log`] without its caller having to think about it.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::log::{self, Handle, Stream};
use crate::repo::wsl;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A git command for `repo`, with the environment every call needs.
pub(crate) fn git(repo: &Path) -> Command {
    // A repository inside WSL gets its git from the distribution, started in its folder.
    if let Some(location) = wsl::locate(repo) {
        return configured(location.git());
    }
    let mut command = git_anywhere();
    command.arg("-C").arg(repo);
    command
}

/// A git command that belongs to no repository, such as one about the user's own config.
pub(crate) fn git_anywhere() -> Command {
    configured(Command::new("git"))
}

fn configured(mut command: Command) -> Command {
    command
        // Launched from inside a git hook these would point at some other repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        // The app refreshes on its own, so never contend with a git the user is running.
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Set on a command that must never ask the person anything. See [`quietly`].
const NEVER_ASK: &str = "MANTISGIT_NEVER_ASK";

/// Marks a command as one nobody is waiting on, such as a fetch the app makes by itself.
/// Where an ordinary command would open a question for a key's passphrase or a login, this
/// one fails, so a popup never appears for something the person didn't start.
pub(crate) fn quietly(command: &mut Command) -> &mut Command {
    command.env(NEVER_ASK, "1")
}

fn is_quiet(command: &Command) -> bool {
    command
        .get_envs()
        .any(|(name, value)| name == NEVER_ASK && value.is_some())
}

/// Runs git and returns its output, or what it said went wrong.
pub fn run(repo: &Path, args: &[&str]) -> Result<String, String> {
    run_command(git(repo).args(args), None)
}

/// A request to stop work that is no longer wanted. Clones share one flag, so the side
/// that starts the work keeps one and hands the other to the thread doing it.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What a cancelled run returns in place of git's output.
pub const CANCELLED: &str = "cancelled";

/// Runs git like [`run`], but stops the process and returns [`CANCELLED`] soon after
/// `cancel` is set, so that a slow command nobody is waiting for any more stops costing.
pub fn run_cancellable(repo: &Path, args: &[&str], cancel: &Cancel) -> Result<String, String> {
    if cancel.is_cancelled() {
        return Err(CANCELLED.to_owned());
    }
    let mut command = git(repo);
    let mut process = Process::spawn(command.args(args), false)?;
    let stdout = pump(
        process.stdout.take().expect("stdout is piped"),
        Stream::Out,
        process.record,
        None,
    );
    let (success, stderr) = process.finish_unless(cancel)?;
    if success {
        Ok(String::from_utf8_lossy(&stdout.join().unwrap_or_default()).into_owned())
    } else {
        Err(stderr_message(&stderr))
    }
}

/// Runs git with `input` on stdin.
pub fn run_with_input(repo: &Path, args: &[&str], input: &[u8]) -> Result<String, String> {
    run_command(git(repo).args(args), Some(input))
}

/// Whether `revision` names something in the repository.
pub fn resolves(repo: &Path, revision: &str) -> bool {
    run(repo, &["rev-parse", "-q", "--verify", revision]).is_ok()
}

/// Runs a command built from [`git`] and returns its output, or what it said went wrong.
pub fn run_command(command: &mut Command, input: Option<&[u8]>) -> Result<String, String> {
    let finished = run_unchecked(command, input)?;
    if finished.success {
        Ok(String::from_utf8_lossy(&finished.stdout).into_owned())
    } else {
        Err(stderr_message(&finished.stderr))
    }
}

/// What a finished process left behind.
pub(crate) struct Finished {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs a command to the end and hands back its output whatever its exit status was, for
/// commands like `git diff --no-index` that exit non-zero to mean "different".
pub(crate) fn run_unchecked(
    command: &mut Command,
    input: Option<&[u8]>,
) -> Result<Finished, String> {
    let mut process = Process::spawn(command, input.is_some())?;
    // Written from its own thread: a command that answers as it reads, like
    // `cat-file --batch-check`, fills its output pipe while input is still going in, and
    // if nothing reads that output at the same time both sides wait on each other forever.
    let writer = input.map(|input| {
        process.note(&format!("{} bytes on stdin", input.len()));
        let mut stdin = process.stdin.take().expect("stdin is piped");
        let input = input.to_vec();
        // A failed write means git quit early, and its exit status says why. Dropping
        // the pipe at the end is what tells it the input is over.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    let stdout = pump(
        process.stdout.take().expect("stdout is piped"),
        Stream::Out,
        process.record,
        None,
    );
    let (success, stderr) = process.finish()?;
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    Ok(Finished {
        success,
        stdout: stdout.join().unwrap_or_default(),
        stderr,
    })
}

/// A running git process. This is the one place one is started, so it is the one place
/// that records it: the command when it starts, its stderr as it arrives, and how it
/// ended. Callers that read stdout themselves, to stream it, still get all of that.
pub(crate) struct Process {
    child: Child,
    record: Handle,
    stderr: Option<JoinHandle<Vec<u8>>>,
    /// Present when asked for in [`Process::spawn`]; take it to write, then drop it.
    pub stdin: Option<ChildStdin>,
    /// Taken by whoever reads the output.
    pub stdout: Option<ChildStdout>,
    finished: bool,
}

impl Process {
    pub fn spawn(command: &mut Command, piped_stdin: bool) -> Result<Self, String> {
        // Git in WSL asks its questions in its output, which this reads and answers. Only
        // commands that can reach a remote keep stdin open for the answer; any other
        // might wait on an input that never comes.
        let asking = wsl::Invocation::of(command)
            .filter(|invocation| !piped_stdin && !is_quiet(command) && invocation.may_ask())
            .map(|_| {
                // One asker for the whole command, so ssh asking again means a wrong answer.
                let asker = NEXT_ASKER.fetch_add(1, Ordering::Relaxed);
                Asking {
                    token: wsl::new_token(),
                    ask: Box::new(move |prompt| crate::askpass::ask(asker, prompt)),
                }
            });
        Self::spawn_asking(command, piped_stdin, asking)
    }

    fn spawn_asking(
        command: &mut Command,
        piped_stdin: bool,
        asking: Option<Asking>,
    ) -> Result<Self, String> {
        if let Some(asking) = &asking {
            command.env(wsl::TOKEN_VAR, &asking.token);
        }
        command
            // There is no terminal to answer on, so fail rather than hang.
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if piped_stdin || asking.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // ssh would otherwise ask on the terminal the app was started from, where nobody
        // sees it and git waits forever. The question goes to the app's popup instead.
        #[cfg(unix)]
        if is_quiet(command) {
            // Not even a program of the user's own, which would open a window.
            command
                .env("SSH_ASKPASS", "/bin/false")
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("GIT_ASKPASS", "/bin/false");
        } else if asking.is_none() {
            crate::askpass::configure(command);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // CREATE_NO_WINDOW: a windowed app would otherwise flash a console per command.
            command.creation_flags(0x0800_0000);
        }
        let record = log::start(command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                record.finish(false, "could not start");
                return Err(format!("could not run git: {error}"));
            }
        };
        let answerer = asking.and_then(|asking| {
            Some(Answerer {
                stdin: child.stdin.take()?,
                asking,
            })
        });
        let stderr = pump(
            child.stderr.take().expect("stderr is piped"),
            Stream::Err,
            record,
            answerer,
        );
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            child,
            record,
            stderr: Some(stderr),
            finished: false,
        })
    }

    /// Adds a line to this command's log entry.
    pub fn note(&self, text: &str) {
        self.record.note(text);
    }

    /// Like [`Process::finish`], but gives the process up when `cancel` is set. Dropping it
    /// stops it, which closes its output so that whoever reads that ends as well.
    fn finish_unless(mut self, cancel: &Cancel) -> Result<(bool, Vec<u8>), String> {
        loop {
            if cancel.is_cancelled() {
                return Err(CANCELLED.to_owned());
            }
            match self.child.try_wait() {
                Ok(Some(_)) => return self.finish(),
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(error) => return Err(format!("could not wait for git: {error}")),
            }
        }
    }

    /// Waits for the process, and returns whether it succeeded and what it wrote to stderr.
    pub fn finish(mut self) -> Result<(bool, Vec<u8>), String> {
        self.finished = true;
        let status = self.child.wait().map_err(|error| {
            self.record.finish(false, "could not wait");
            format!("could not wait for git: {error}")
        })?;
        let stderr = self
            .stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        self.record.finish(status.success(), describe(status));
        Ok((status.success(), stderr))
    }
}

impl Drop for Process {
    /// A process abandoned partway through, such as a load that was superseded, is stopped.
    fn drop(&mut self) {
        if !self.finished {
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            self.record.finish(true, "stopped early");
        }
    }
}

/// Whether `text` is a question git or ssh is waiting on: no newline, ending in a colon and
/// a space, like "Enter passphrase for key '/home/me/.ssh/id_ed25519': ".
fn is_prompt(text: &[u8]) -> bool {
    text.len() < 300 && (text.ends_with(b": ") || text.ends_with(b"? "))
}

/// Copies a pipe into the log a line at a time as it arrives, and returns all of it. A
/// prompt has no newline, so it is logged as soon as it arrives rather than never.
///
/// With an `answerer`, a line that is a question from git in WSL (see [`wsl::BOOTSTRAP`]) is
/// not output: it is asked, and the reply is written to the process's stdin, where its
/// askpass program waits. It is left out of what is returned.
fn pump(
    pipe: impl Read + Send + 'static,
    stream: Stream,
    record: Handle,
    answerer: Option<Answerer>,
) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut pipe = pipe;
        let mut answerer = answerer;
        let mut all = Vec::new();
        let mut pending = Vec::new();
        // Git blocks writing until this reads, so a big diff is only as fast as this loop:
        // read in large pieces, and once the log is full stop handling lines one by one.
        let mut chunk = vec![0u8; 64 * 1024];
        let mut logged = 0usize;
        loop {
            let read = match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            if stream == Stream::Out && answerer.is_none() && logged >= log::MAX_LINES {
                // What is left of a line cut by the chunk before goes first, to keep order.
                all.append(&mut pending);
                all.extend_from_slice(&chunk[..read]);
                record.dropped(chunk[..read].iter().filter(|byte| **byte == b'\n').count());
                continue;
            }
            pending.extend_from_slice(&chunk[..read]);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                logged += 1;
                let line: Vec<u8> = pending.drain(..=end).collect();
                let text = String::from_utf8_lossy(&line);
                let question = answerer
                    .as_mut()
                    .and_then(|a| Some((wsl::question(&text, &a.asking.token)?, a)));
                if let Some((prompt, answerer)) = question {
                    record.line(stream, prompt);
                    answerer.answer(prompt);
                } else {
                    record.line(stream, &text);
                    all.extend_from_slice(&line);
                }
            }
            if is_prompt(&pending) {
                record.line(stream, &String::from_utf8_lossy(&pending));
                all.append(&mut pending);
            }
        }
        if !pending.is_empty() {
            record.line(stream, &String::from_utf8_lossy(&pending));
            all.append(&mut pending);
        }
        all
    })
}

/// Puts a question to the person and returns the answer, or `None` for a cancel.
type Ask = Box<dyn Fn(&str) -> Option<String> + Send>;

/// How one command's questions are put: `token` is what its askpass program proves itself
/// with, so that only the program the app started can ask.
struct Asking {
    token: String,
    ask: Ask,
}

/// The way back to a process whose askpass program is waiting on its stdin.
struct Answerer {
    stdin: ChildStdin,
    asking: Asking,
}

impl Answerer {
    fn answer(&mut self, prompt: &str) {
        let reply = wsl::reply((self.asking.ask)(prompt).as_deref());
        // A failed write means the process is gone, and its exit status says why.
        let _ = self.stdin.write_all(reply.as_bytes());
        let _ = self.stdin.flush();
    }
}

static NEXT_ASKER: AtomicU32 = AtomicU32::new(1);

/// How a process ended, in words.
fn describe(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "stopped by a signal".to_owned(),
    }
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
    use crate::testing::Repo;

    #[cfg(unix)]
    #[test]
    fn input_and_output_larger_than_a_pipe_both_get_through() {
        // `cat` answers as it reads, so a writer that waits to finish before reading
        // would stop with both pipes full.
        let input = "some text that fills the pipes\n".repeat(100_000);
        let started = std::time::Instant::now();
        let out = run_command(&mut Command::new("cat"), Some(input.as_bytes())).unwrap();
        assert_eq!(out, input);
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn long_output_comes_back_whole_and_the_log_counts_what_it_leaves_out() {
        let repo = Repo::new();
        // Past the log's limit and over several reads, with lines of every length so that
        // a read ends mid-line when the log fills.
        let text: String = (0..30_000)
            .map(|n| format!("line {n} {}\n", "x".repeat(n % 97)))
            .collect();
        repo.write("big", &text);
        repo.commit("big");
        let out = run(repo.path(), &["show", "HEAD:big"]).unwrap();
        assert_eq!(out, text);

        let entry = log::read(|entries| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.command.contains("HEAD:big"))
                .cloned()
        })
        .unwrap();
        assert_eq!(entry.lines.len(), log::MAX_LINES);
        // The line a read cut in two is counted with the next read, so allow one off.
        assert!(entry.dropped + 1 >= 30_000 - log::MAX_LINES);
        assert!(entry.dropped <= 30_000 - log::MAX_LINES);
    }

    #[cfg(unix)]
    #[test]
    fn a_prompt_without_a_newline_is_logged_while_the_command_still_waits() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'Enter passphrase for key: ' >&2; sleep 5"]);
        let mut process = Process::spawn(&mut command, false).unwrap();
        let started = std::time::Instant::now();
        let seen = loop {
            let seen = log::read(|entries| {
                entries.iter().any(|e| {
                    e.lines
                        .iter()
                        .any(|l| l.text.starts_with("Enter passphrase"))
                })
            });
            if seen || started.elapsed() > Duration::from_secs(3) {
                break seen;
            }
            thread::sleep(Duration::from_millis(20));
        };
        process.stdout.take();
        assert!(seen, "the prompt never reached the log");
    }

    /// Runs the script that starts git in WSL, under plain `sh` with a `git` that asks the
    /// way ssh does, and returns what it printed, whether it succeeded and its stderr.
    #[cfg(unix)]
    fn ask_like_ssh(reply: Option<&'static str>) -> (String, bool, String) {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let fake = bin.path().join("git");
        std::fs::write(
            &fake,
            "#!/bin/sh\n\
             echo \"ask:$SSH_ASKPASS_REQUIRE\"\n\
             echo \"file:$SSH_ASKPASS\"\n\
             printf '\\037\\037MANTISGIT-ASKPASS:forged\\tForged question: \\n' >&2\n\
             a=$(\"$SSH_ASKPASS\" \"Enter passphrase for key 'k': \" </dev/null) || { echo denied >&2; exit 3; }\n\
             echo \"got:$a\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("sh");
        command
            .args(["-c", wsl::BOOTSTRAP, "sh", "fetch"])
            .env("PATH", path)
            .env_remove("SSH_ASKPASS")
            .env_remove("GIT_ASKPASS");
        let ask = Box::new(move |prompt: &str| {
            assert_eq!(prompt, "Enter passphrase for key 'k': ");
            reply.map(str::to_owned)
        });
        let asking = Asking {
            token: "t0k3n".to_owned(),
            ask,
        };
        let mut process = Process::spawn_asking(&mut command, false, Some(asking)).unwrap();
        let mut out = String::new();
        process
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        let (success, stderr) = process.finish().unwrap();
        (out, success, String::from_utf8_lossy(&stderr).into_owned())
    }

    #[cfg(unix)]
    #[test]
    fn a_question_from_git_in_wsl_is_asked_and_the_answer_reaches_it() {
        let (out, success, stderr) = ask_like_ssh(Some("hunter2"));
        assert!(success, "out={out:?} err={stderr:?}");
        assert!(out.contains("ask:force"), "ssh must use it even with a tty");
        assert!(out.contains("got:hunter2"), "{out}");
        assert!(!stderr.contains("t0k3n"), "the question is not output");
        assert!(
            stderr.contains("Forged question"),
            "a marker with another token is ordinary output, and was not asked: {stderr:?}"
        );
        let file = out.lines().find_map(|l| l.strip_prefix("file:")).unwrap();
        assert!(!Path::new(file).exists(), "the helper is cleaned up");
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_a_question_from_git_in_wsl_fails_it() {
        let (out, success, stderr) = ask_like_ssh(None);
        assert!(!success);
        assert!(!out.contains("got:"));
        assert!(stderr.contains("denied"), "{stderr}");
        assert!(!stderr.contains("t0k3n"));
    }

    #[test]
    fn every_command_is_logged_with_its_output_and_how_it_ended() {
        let repo = Repo::new();
        run(repo.path(), &["rev-parse", "--is-inside-work-tree"]).unwrap();
        let _ = run(repo.path(), &["rev-parse", "no-such-revision"]);

        let entries: Vec<_> = log::read(|entries| {
            entries
                .iter()
                .filter(|entry| entry.repo == repo.path())
                .cloned()
                .collect()
        });
        let ok = entries
            .iter()
            .find(|e| e.command.ends_with("--is-inside-work-tree"))
            .unwrap();
        assert!(ok.lines.iter().any(|line| line.text == "true"));
        assert!(matches!(ok.state, log::State::Done { success: true, .. }));
        let failed = entries
            .iter()
            .find(|e| e.command.ends_with("no-such-revision"))
            .unwrap();
        assert!(matches!(
            failed.state,
            log::State::Done { success: false, .. }
        ));
        assert!(failed.lines.iter().any(|line| line.stream == Stream::Err));
    }

    #[test]
    fn a_process_dropped_early_is_stopped_and_logged() {
        let repo = Repo::new();
        let mut command = git(repo.path());
        command.args(["hash-object", "--stdin"]);
        let process = Process::spawn(&mut command, true).unwrap();
        // Holding stdin open keeps git waiting, as an abandoned load would be.
        drop(process);
        let entry = log::read(|entries| {
            entries
                .iter()
                .rev()
                .find(|e| e.repo == repo.path() && e.command.contains("hash-object"))
                .cloned()
                .unwrap()
        });
        assert!(
            matches!(&entry.state, log::State::Done { status, .. } if status == "stopped early")
        );
    }
}
