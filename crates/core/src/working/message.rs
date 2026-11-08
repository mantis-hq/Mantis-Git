//! The words of a commit: the Conventional Commit message the form builds, and what its
//! button and the band after it say.

use crate::text::count;

/// The kinds of change the commit form offers, in its order.
pub const COMMIT_TYPES: [&str; 7] = ["feat", "fix", "docs", "refactor", "test", "chore", "ci"];

/// Whether a commit has what it needs to be written: something staged, and a subject.
pub fn can_commit(staged: usize, subject: &str) -> bool {
    staged > 0 && !subject.trim().is_empty()
}

/// What the commit button says it will do.
pub fn commit_label(staged: usize) -> String {
    match staged {
        0 => "Stage a file to commit".to_owned(),
        files => format!("Commit {}", count(files, "file")),
    }
}

/// What to tell the user once a commit landed, with how to take it back.
pub fn committed_notice(line: &str) -> String {
    format!(
        "Committed “{line}”. `git reset --soft HEAD~1` undoes it and keeps your changes staged."
    )
}

/// Whether a kind of change can be marked as breaking.
pub fn can_break(kind: &str) -> bool {
    kind == "feat"
}

/// Builds a Conventional Commit message.
pub fn message(kind: &str, breaking: bool, subject: &str, details: &str) -> String {
    let mut message = format!("{}\n", subject_line(kind, breaking, subject));
    let details = details.trim();
    if !details.is_empty() {
        message.push('\n');
        message.push_str(details);
        message.push('\n');
    }
    message
}

pub fn subject_line(kind: &str, breaking: bool, subject: &str) -> String {
    let mark = if breaking && can_break(kind) { "!" } else { "" };
    format!("{kind}{mark}: {}", subject.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_name_the_result() {
        assert_eq!(commit_label(0), "Stage a file to commit");
        assert_eq!(commit_label(2), "Commit 2 files");
        assert!(can_commit(1, "add it"));
        assert!(!can_commit(1, "  "));
        assert!(!can_commit(0, "add it"));
        assert!(committed_notice("fix: x").contains("git reset --soft HEAD~1"));
    }

    #[test]
    fn messages_follow_conventional_commits() {
        assert_eq!(message("fix", false, " a bug ", ""), "fix: a bug\n");
        assert_eq!(message("feat", true, "x", ""), "feat!: x\n");
        assert_eq!(message("docs", true, "x", ""), "docs: x\n");
        assert_eq!(message("feat", false, "x", " why\n"), "feat: x\n\nwhy\n");
    }
}
