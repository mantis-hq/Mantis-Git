//! Runs the `git` executable. Every git process in the app starts here, and so every one
//! is recorded in [`crate::log`] without its caller having to think about it.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::log::{self, Handle, Stream};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

/// A git command for `repo`, with the environment every call needs.
pub(crate) fn git(repo: &Path) -> Command {
    let mut command = git_anywhere();
    command.arg("-C").arg(repo);
    command
}

/// A git command that belongs to no repository, such as one about the user's own config.
pub(crate) fn git_anywhere() -> Command {
    let mut command = Command::new("git");
    command
        // Launched from inside a git hook these would point at some other repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        // The app refreshes on its own, so never contend with a git the user is running.
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Runs git and returns its output, or what it said went wrong.
pub fn run(repo: &Path, args: &[&str]) -> Result<String, String> {
    run_command(git(repo).args(args), None)
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
    if let Some(input) = input {
        process.note(&format!("{} bytes on stdin", input.len()));
        let mut stdin = process.stdin.take().expect("stdin is piped");
        // A failed write means git quit early, and its exit status says why.
        let _ = stdin.write_all(input);
    }
    let stdout = pump(
        process.stdout.take().expect("stdout is piped"),
        Stream::Out,
        process.record,
    );
    let (success, stderr) = process.finish()?;
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
        command
            // There is no terminal to answer on, so fail rather than hang.
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if piped_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        let stderr = pump(
            child.stderr.take().expect("stderr is piped"),
            Stream::Err,
            record,
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

/// Copies a pipe into the log a line at a time as it arrives, and returns all of it.
fn pump(pipe: impl Read + Send + 'static, stream: Stream, record: Handle) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut all = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    record.line(stream, &String::from_utf8_lossy(&line));
                    all.extend_from_slice(&line);
                }
            }
        }
        all
    })
}

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
