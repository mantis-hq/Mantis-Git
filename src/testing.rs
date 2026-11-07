//! A throwaway repository for tests. `tests/render.rs` includes this file too, and uses
//! only part of it.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

pub const AUTHOR: &str = "Ada";
const EMAIL: &str = "ada@example.com";
const DATE: &str = "2026-03-04T05:06:07Z";

const SETTINGS: [(&str, &str); 5] = [
    ("user.name", AUTHOR),
    ("user.email", EMAIL),
    ("commit.gpgsign", "false"),
    ("tag.gpgsign", "false"),
    ("core.autocrlf", "false"),
];

/// Runs git in `dir`, away from the user's own settings, and returns what it printed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    for (name, value) in SETTINGS {
        command.arg("-c").arg(format!("{name}={value}"));
    }
    let output = command
        .args(args)
        .env("GIT_CONFIG_GLOBAL", dir.join(".no-global-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", DATE)
        .env("GIT_COMMITTER_DATE", DATE)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Commits with no change at all, and returns the new commit.
pub fn empty_commit(dir: &Path, subject: &str) -> String {
    git(dir, &["commit", "-q", "--allow-empty", "-m", subject]);
    git(dir, &["rev-parse", "HEAD"])
}

pub struct Repo(TempDir);

impl Repo {
    /// An empty repository on `main`. Its settings live in the repository as well, because
    /// the code under test runs git without the ones [`git`] passes.
    pub fn new() -> Self {
        let repo = Self(TempDir::new().unwrap());
        repo.git(&["init", "-q", "-b", "main"]);
        for (name, value) in SETTINGS {
            repo.git(&["config", name, value]);
        }
        repo
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        git(self.path(), args)
    }

    pub fn write(&self, path: &str, content: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.path().join(path)).unwrap()
    }

    /// Commits everything in the working tree, and returns the new commit.
    pub fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        empty_commit(self.path(), message)
    }
}
