//! Commands that change a repository, or read something the graph doesn't load up front.
//!
//! Everything here blocks, so call it off the UI thread.

use crate::git::{git, stderr_message};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Runs git and returns its output, or what it said went wrong.
pub fn run(repo: &Path, args: &[&str]) -> Result<String, String> {
    run_command(git(repo).args(args), None)
}

/// Runs git with `input` on stdin.
pub fn run_with_input(repo: &Path, args: &[&str], input: &[u8]) -> Result<String, String> {
    run_command(git(repo).args(args), Some(input))
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
