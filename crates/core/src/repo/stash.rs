//! Stashes: setting the staged changes aside, and applying or deleting stashes.

use crate::ops;
use crate::text::count;
use std::path::Path;

/// "1 stash", "2 stashes".
fn stashes(n: usize) -> String {
    format!("{n} {}", if n == 1 { "stash" } else { "stashes" })
}

/// Sets what is staged aside and cleans it from the working tree and the index, leaving
/// unstaged changes where they are. `message` names the stash; without one git writes its own.
pub fn push_staged(
    repo: &Path,
    staged: usize,
    message: Option<&str>,
) -> Result<Option<String>, String> {
    let mut args = vec!["stash", "push", "--staged"];
    let message = message.map(str::trim).filter(|message| !message.is_empty());
    if let Some(message) = message {
        args.extend(["-m", message]);
    }
    ops::run(repo, &args)?;
    Ok(Some(format!(
        "Stashed {}. To bring them back, apply the newest stash, or run `git stash pop --index`.",
        count(staged, "staged file")
    )))
}

/// Applies stashes and keeps them. Oldest first, so that where two touch the same lines the
/// newest wins. Stops at the first that git can't apply, and says which.
pub fn apply_many(repo: &Path, indexes: &[usize]) -> Result<Option<String>, String> {
    let mut ordered = indexes.to_vec();
    ordered.sort_unstable_by(|a, b| b.cmp(a));
    ordered.dedup();
    for (done, index) in ordered.iter().enumerate() {
        let name = format!("stash@{{{index}}}");
        ops::run(repo, &["stash", "apply", &name]).map_err(|error| {
            if done == 0 {
                error
            } else {
                format!("Applied {} then stopped at {name}: {error}", stashes(done))
            }
        })?;
    }
    Ok(Some(format!(
        "Applied {}. {} kept in the list.",
        stashes(ordered.len()),
        if ordered.len() == 1 {
            "It is"
        } else {
            "They are"
        }
    )))
}

/// Applies stashes and deletes each once it applied. Oldest first, as `apply_many` does, so
/// the numbers of the rest don't move under it. Stops at the first that git can't apply,
/// which stays in the list, and says which.
pub fn pop_many(repo: &Path, indexes: &[usize]) -> Result<Option<String>, String> {
    let mut ordered = indexes.to_vec();
    ordered.sort_unstable_by(|a, b| b.cmp(a));
    ordered.dedup();
    let mut kept = Vec::new();
    for (done, index) in ordered.iter().enumerate() {
        let name = format!("stash@{{{index}}}");
        let oid = ops::run(repo, &["rev-parse", &name])?.trim().to_owned();
        ops::run(repo, &["stash", "pop", "-q", &name]).map_err(|error| {
            if done == 0 {
                error
            } else {
                format!("Popped {} then stopped at {name}: {error}", stashes(done))
            }
        })?;
        kept.push((*index, oid));
    }
    let undo = kept
        .iter()
        .rev()
        .map(|(_, oid)| format!("`git stash store {oid}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(format!(
        "Popped {}. To put {} back in the list, run {undo}.",
        stashes(kept.len()),
        if kept.len() == 1 { "it" } else { "them" }
    )))
}

/// Deletes stashes. The newest go first, so the numbers of the rest don't move under it.
/// The notice gives the command that brings each one back.
pub fn drop_many(repo: &Path, indexes: &[usize]) -> Result<Option<String>, String> {
    let mut ordered = indexes.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    // Read every hash before the first drop, since afterwards the numbers shift.
    let mut kept = Vec::new();
    for index in &ordered {
        let name = format!("stash@{{{index}}}");
        let oid = ops::run(repo, &["rev-parse", &name])?.trim().to_owned();
        kept.push((*index, oid));
    }
    for (index, _) in kept.iter().rev() {
        ops::run(
            repo,
            &["stash", "drop", "-q", &format!("stash@{{{index}}}")],
        )?;
    }
    Ok(Some(restore_hint(&kept)))
}

fn restore_hint(kept: &[(usize, String)]) -> String {
    let undo = kept
        .iter()
        .rev()
        .map(|(_, oid)| format!("`git stash store {oid}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Deleted {}. To bring {} back, run {undo}.",
        stashes(kept.len()),
        if kept.len() == 1 { "it" } else { "them" }
    )
}

/// What deleting these stashes does, and where its undo will be written.
pub fn drop_warning(subjects: &[String]) -> String {
    match subjects {
        [one] => format!(
            "This deletes the stash \"{one}\". The notice that follows gives the command that \
             brings it back."
        ),
        many => format!(
            "This deletes {}. The notice that follows gives the command that brings each back.",
            stashes(many.len())
        ),
    }
}

/// The result a menu line names for applying `n` stashes.
pub fn apply_label(n: usize) -> String {
    if n == 1 {
        "Apply stash".to_owned()
    } else {
        format!("Apply {n} stashes")
    }
}

/// The result a menu line names for popping `n` stashes.
pub fn pop_label(n: usize) -> String {
    if n == 1 {
        "Pop stash".to_owned()
    } else {
        format!("Pop {n} stashes")
    }
}

/// The result a menu line names for deleting `n` stashes.
pub fn drop_label(n: usize) -> String {
    if n == 1 {
        "Delete stash".to_owned()
    } else {
        format!("Delete {n} stashes")
    }
}

/// A short form of a stash's subject for lists: git writes "On main: note" or
/// "WIP on main: 1a2b3c4 subject", and the part after the branch is what tells them apart.
pub fn title(subject: &str) -> &str {
    subject
        .split_once(": ")
        .map_or(subject, |(_, rest)| rest)
        .trim()
}

/// The word on a stash's row after its number: the branch it was made on.
pub fn branch(subject: &str) -> Option<&str> {
    let head = subject.split_once(": ")?.0;
    head.strip_prefix("WIP on ")
        .or_else(|| head.strip_prefix("On "))
}

/// The label for the button that stashes what is staged, naming its result.
pub fn push_label(staged: usize) -> String {
    format!("Stash {}", count(staged, "file"))
}

/// Which stashes are picked in the sidebar. Plain picks one, `Ctrl` adds or removes one,
/// and `Shift` picks the run from the last click.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picks {
    picked: Vec<usize>,
    anchor: Option<usize>,
}

pub use crate::history::picks::Click;

impl Picks {
    pub fn click(&mut self, index: usize, click: Click) {
        match click {
            Click::Plain => {
                if self.picked == [index] {
                    self.picked.clear();
                    self.anchor = None;
                } else {
                    self.picked = vec![index];
                    self.anchor = Some(index);
                }
            }
            Click::Add => {
                match self.picked.iter().position(|&picked| picked == index) {
                    Some(at) => {
                        self.picked.remove(at);
                    }
                    None => self.picked.push(index),
                }
                self.anchor = Some(index);
            }
            Click::Range => {
                let from = self.anchor.unwrap_or(index);
                let (low, high) = (from.min(index), from.max(index));
                self.picked = (low..=high).collect();
            }
        }
    }

    pub fn contains(&self, index: usize) -> bool {
        self.picked.contains(&index)
    }

    /// Picked numbers, ascending.
    pub fn indexes(&self) -> Vec<usize> {
        let mut indexes = self.picked.clone();
        indexes.sort_unstable();
        indexes
    }

    /// Stashes change when one is made or deleted, so keep only numbers that still exist.
    pub fn retain_below(&mut self, len: usize) {
        self.picked.retain(|&index| index < len);
        if self.anchor.is_some_and(|anchor| anchor >= len) {
            self.anchor = None;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.picked.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    fn repo_with_stashes() -> Repo {
        let repo = Repo::new();
        repo.write("file", "base");
        repo.git(&["add", "file"]);
        repo.commit("base");
        for (n, text) in ["one", "two", "three"].iter().enumerate() {
            repo.write("file", text);
            repo.git(&["stash", "push", "-q", "-m", &format!("note {n}")]);
        }
        repo
    }

    #[test]
    fn popping_applies_a_stash_and_removes_only_it() {
        let repo = repo_with_stashes();
        let notice = pop_many(repo.path(), &[0]).unwrap().unwrap();
        assert!(notice.contains("Popped 1 stash"), "{notice}");
        assert!(notice.contains("stash store"), "{notice}");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("file")).unwrap(),
            "three"
        );
        let left = ops::run(repo.path(), &["stash", "list"]).unwrap();
        assert_eq!(left.lines().count(), 2);
        assert!(!left.contains("note 2"));
    }

    #[test]
    fn staged_changes_are_stashed_and_unstaged_ones_stay() {
        let repo = Repo::new();
        repo.write("a", "base");
        repo.write("b", "base");
        repo.git(&["add", "."]);
        repo.commit("base");
        repo.write("a", "staged");
        repo.write("b", "loose");
        repo.git(&["add", "a"]);

        let notice = push_staged(repo.path(), 1, Some("just a"))
            .unwrap()
            .unwrap();
        assert!(notice.contains("1 staged file"), "{notice}");
        assert_eq!(repo.read("a"), "base");
        assert_eq!(repo.read("b"), "loose");
        assert!(repo.git(&["stash", "list"]).contains("just a"));
    }

    #[test]
    fn nothing_staged_is_git_s_to_refuse() {
        let repo = Repo::new();
        repo.commit("base");
        assert!(
            push_staged(repo.path(), 0, None).is_err() || repo.git(&["stash", "list"]).is_empty()
        );
    }

    #[test]
    fn applying_keeps_the_stash_and_oldest_goes_first() {
        let repo = repo_with_stashes();
        // stash@{0} is "three", the newest.
        apply_many(repo.path(), &[0, 2]).ok();
        assert_eq!(repo.git(&["stash", "list"]).lines().count(), 3);
    }

    #[test]
    fn deleting_several_removes_exactly_those_and_says_how_to_restore() {
        let repo = repo_with_stashes();
        let notice = drop_many(repo.path(), &[0, 2]).unwrap().unwrap();
        let left = repo.git(&["stash", "list"]);
        assert_eq!(left.lines().count(), 1);
        assert!(left.contains("note 1"), "{left}");
        assert!(notice.contains("Deleted 2 stashes"), "{notice}");
        assert_eq!(notice.matches("git stash store").count(), 2);
    }

    #[test]
    fn titles_and_branches_read_git_s_subjects() {
        assert_eq!(title("On main: half done"), "half done");
        assert_eq!(branch("On main: half done"), Some("main"));
        assert_eq!(title("WIP on feat/x: 1a2b3c4 add y"), "1a2b3c4 add y");
        assert_eq!(branch("WIP on feat/x: 1a2b3c4 add y"), Some("feat/x"));
        assert_eq!(title("plain"), "plain");
    }

    #[test]
    fn picks_follow_plain_ctrl_and_shift_clicks() {
        let mut picks = Picks::default();
        picks.click(1, Click::Plain);
        picks.click(3, Click::Add);
        assert_eq!(picks.indexes(), [1, 3]);
        picks.click(4, Click::Range);
        assert_eq!(picks.indexes(), [3, 4]);
        picks.click(4, Click::Add);
        assert_eq!(picks.indexes(), [3]);
        picks.click(3, Click::Plain);
        assert!(picks.is_empty());
        picks.click(5, Click::Plain);
        picks.retain_below(2);
        assert!(picks.is_empty());
    }

    #[test]
    fn labels_name_their_result() {
        assert_eq!(apply_label(1), "Apply stash");
        assert_eq!(drop_label(3), "Delete 3 stashes");
        assert_eq!(push_label(2), "Stash 2 files");
    }
}
