//! Runs the `git` executable. Every git process in the app starts here.
//!
//! Everything here blocks, so call it off the UI thread.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// A git command for `repo`, with the environment every call needs.
pub(crate) fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
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

pub fn run_command(command: &mut Command, input: Option<&[u8]>) -> Result<String, String> {
    let mut child = command
        // There is no terminal to answer on, so fail rather than hang.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run git: {error}"))?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // A failed write means git quit early, and its exit status says why.
        let _ = stdin.write_all(input);
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not wait for git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(stderr_message(&output.stderr))
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
