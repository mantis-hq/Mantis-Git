//! What can be done to branches and tags: bring one into another, check one out, delete
//! one. The interface asks [`drop_choices`] what dropping one ref on another can mean, so
//! that the menu only offers what makes sense for how the two histories relate.

use crate::history::refs::RefKind;
use crate::ops::{self, git};
use crate::repo::conflict;
use crate::text::short_oid;
use std::path::Path;

/// How a ref being brought in (the source) stands against the branch receiving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// Both are the same commit.
    Same,
    /// The target already has everything in the source.
    Contained,
    /// The target is behind the source, so it can simply move forward to it.
    FastForward,
    /// Each has commits the other lacks.
    Diverged,
}

/// Compares two revisions. `source` is brought into `target`.
pub fn relation(repo: &Path, source: &str, target: &str) -> Result<Relation, String> {
    let oid = |revision: &str| -> Result<String, String> {
        let spec = format!("{revision}^{{commit}}");
        ops::run(repo, &["rev-parse", "--verify", "-q", &spec])
            .map(|out| out.trim().to_owned())
            .map_err(|_| format!("{revision} is not a commit"))
    };
    let (source, target) = (oid(source)?, oid(target)?);
    if source == target {
        return Ok(Relation::Same);
    }
    let base = ops::run(repo, &["merge-base", &source, &target])
        .map(|out| out.trim().to_owned())
        .unwrap_or_default();
    Ok(if base == source {
        Relation::Contained
    } else if base == target {
        Relation::FastForward
    } else {
        Relation::Diverged
    })
}

/// What dropping can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAction {
    FastForward,
    Merge,
    /// A merge that always makes a merge commit.
    MergeCommit,
    Rebase,
}

/// One entry of the menu a drop opens. An entry with no action only explains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropChoice {
    pub action: Option<DropAction>,
    pub label: String,
}

/// The ref being dragged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dragged {
    pub kind: RefKind,
    pub name: String,
}

/// What dropping `source` on the local branch `target` offers, given how they relate.
pub fn drop_choices(relation: Relation, source: &Dragged, target: &str) -> Vec<DropChoice> {
    let (from, to) = (&source.name, target);
    let choice = |action, label: String| DropChoice {
        action: Some(action),
        label,
    };
    match relation {
        Relation::Same | Relation::Contained => vec![DropChoice {
            action: None,
            label: format!("{to} already has everything in {from}"),
        }],
        Relation::FastForward => vec![
            choice(
                DropAction::FastForward,
                format!("Fast-forward {to} to {from}"),
            ),
            choice(
                DropAction::MergeCommit,
                format!("Merge {from} into {to} with a merge commit"),
            ),
        ],
        Relation::Diverged => {
            let mut choices = vec![choice(DropAction::Merge, format!("Merge {from} into {to}"))];
            // Replaying a tag or a remote branch would leave nothing to move.
            if source.kind == RefKind::Branch {
                choices.push(choice(
                    DropAction::Rebase,
                    format!("Rebase {from} onto {to}"),
                ));
            }
            choices
        }
    }
}

/// Says how to undo a step that moved a branch.
fn undo_hint(old: &str) -> String {
    format!("To undo it, run `git reset --hard {}`.", short_oid(old))
}

fn current_branch(repo: &Path) -> Option<String> {
    let name = ops::run(repo, &["branch", "--show-current"]).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn rev_parse(repo: &Path, revision: &str) -> Result<String, String> {
    ops::run(repo, &["rev-parse", "--verify", revision]).map(|out| out.trim().to_owned())
}

/// Runs a drop's action.
pub fn apply_drop(
    repo: &Path,
    action: DropAction,
    source: &str,
    target: &str,
) -> Result<Option<String>, String> {
    match action {
        DropAction::FastForward => fast_forward(repo, source, target),
        DropAction::Merge => merge(repo, source, target, false),
        DropAction::MergeCommit => merge(repo, source, target, true),
        DropAction::Rebase => conflict::rebase_branch(repo, source, target),
    }
}

/// Moves `target` forward to `source`, which must contain it.
pub fn fast_forward(repo: &Path, source: &str, target: &str) -> Result<Option<String>, String> {
    let old = rev_parse(repo, &format!("refs/heads/{target}"))?;
    if current_branch(repo).as_deref() == Some(target) {
        ops::run(repo, &["merge", "--ff-only", source])?;
    } else {
        // Updates the branch without touching the working tree, which is on another branch.
        let new = rev_parse(repo, &format!("{source}^{{commit}}"))?;
        let refspec = format!("{new}:refs/heads/{target}");
        ops::run(repo, &["fetch", "-q", ".", &refspec])?;
    }
    Ok(Some(format!(
        "Moved {target} forward to {source}. {}",
        undo_hint(&old)
    )))
}

/// Merges `source` into `target`, switching to `target` first when it isn't checked out.
pub fn merge(
    repo: &Path,
    source: &str,
    target: &str,
    always_commit: bool,
) -> Result<Option<String>, String> {
    let old = rev_parse(repo, &format!("refs/heads/{target}"))?;
    if current_branch(repo).as_deref() != Some(target) {
        ops::run(repo, &["switch", target])?;
    }
    let mut command = git(repo);
    command.arg("merge").env("GIT_EDITOR", "true");
    if always_commit {
        command.arg("--no-ff");
    }
    command.args(["--no-edit", source]);
    match ops::run_command(&mut command, None) {
        // The conflict screen takes over from here.
        Err(_) if conflict::detect(repo).is_some() => Ok(None),
        Err(message) => Err(message),
        Ok(_) => Ok(Some(format!(
            "Merged {source} into {target}. {}",
            undo_hint(&old)
        ))),
    }
}

/// Merges whatever the user points at into the branch they're on.
pub fn merge_into_current(repo: &Path, source: &str) -> Result<Option<String>, String> {
    let current = current_branch(repo).ok_or("Check out a branch to merge into first.")?;
    merge(repo, source, &current, false)
}

/// Switches to a local branch, or makes a local copy of a remote one, or detaches onto
/// a tag.
pub fn check_out_ref(repo: &Path, kind: RefKind, name: &str) -> Result<Option<String>, String> {
    match kind {
        RefKind::Branch | RefKind::DetachedHead => ops::run(repo, &["switch", name]).map(|_| None),
        RefKind::Remote => {
            let local = name.split_once('/').map_or(name, |(_, branch)| branch);
            if ops::resolves(repo, &format!("refs/heads/{local}")) {
                ops::run(repo, &["switch", local]).map(|_| None)
            } else {
                ops::run(repo, &["switch", "--track", name])
                    .map(|_| Some(format!("Made a local branch {local} that follows {name}.")))
            }
        }
        RefKind::Tag => ops::run(repo, &["switch", "--detach", name]).map(|_| {
            Some(format!(
                "Checked out {name} without a branch. Switch to a branch to get back."
            ))
        }),
    }
}

/// What deleting a ref loses, so the confirmation can say it. `tip` is where the ref points.
pub fn delete_warning(kind: RefKind, name: &str, tip: Option<&str>) -> String {
    let undo = tip.map_or(String::new(), |tip| {
        let restore = match kind {
            RefKind::Tag => format!("git tag {name} {}", short_oid(tip)),
            _ => format!("git branch {name} {}", short_oid(tip)),
        };
        format!(" To undo it, run `{restore}`.")
    });
    match kind {
        RefKind::Tag => format!("This deletes the tag {name}.{undo}"),
        _ => format!("This deletes the branch {name}. Its commits stay until git cleans up.{undo}"),
    }
}

/// Whether deleting a ref loses commits that nothing else keeps: a local branch the default
/// branch doesn't contain, with commits its remote branch doesn't have either (or none).
/// A tag, a merged branch and a pushed branch can all be brought back from elsewhere.
pub fn loses_commits(refs: &crate::history::refs::Refs, kind: RefKind, name: &str) -> bool {
    if kind != RefKind::Branch {
        return false;
    }
    let Some(branch) = refs.branches.iter().find(|branch| branch.name == name) else {
        return false;
    };
    let unmerged = !matches!(branch.ahead_behind, Some((0, _)));
    let unpushed = branch.upstream.is_none_or(|(ahead, _)| ahead > 0);
    unmerged && unpushed
}

/// A branch or tag that is about to be deleted, with where it pointed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doomed {
    pub kind: RefKind,
    pub name: String,
    pub tip: Option<String>,
    /// Whether deleting it loses commits that nothing else keeps, see [`loses_commits`].
    pub at_risk: bool,
}

fn counted(number: usize, one: &str, many: &str) -> String {
    format!("{number} {}", if number == 1 { one } else { many })
}

fn split(refs: &[Doomed]) -> (Vec<&Doomed>, Vec<&Doomed>) {
    refs.iter().partition(|r| r.kind != RefKind::Tag)
}

/// "2 branches and 1 tag".
fn summary(branches: &[&Doomed], tags: &[&Doomed]) -> String {
    let mut parts = Vec::new();
    if !branches.is_empty() {
        parts.push(counted(branches.len(), "branch", "branches"));
    }
    if !tags.is_empty() {
        parts.push(counted(tags.len(), "tag", "tags"));
    }
    parts.join(" and ")
}

/// What deleting several refs loses, and the commands that bring each back.
pub fn delete_many_warning(refs: &[Doomed]) -> String {
    let (branches, tags) = split(refs);
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    let undo: Vec<String> = refs
        .iter()
        .filter_map(|r| {
            let tip = short_oid(r.tip.as_deref()?);
            Some(match r.kind {
                RefKind::Tag => format!("`git tag {} {tip}`", r.name),
                _ => format!("`git branch {} {tip}`", r.name),
            })
        })
        .collect();
    let undo = if undo.is_empty() {
        String::new()
    } else {
        format!(" To undo it, run {}.", undo.join(", "))
    };
    let stay = if branches.is_empty() {
        ""
    } else {
        " Branch commits stay until git cleans up."
    };
    format!(
        "This deletes {}: {}.{stay}{undo}",
        summary(&branches, &tags),
        names.join(", ")
    )
}

/// Deletes branches and tags in one go. Branches go first, in one git call.
pub fn delete_many(repo: &Path, refs: &[Doomed]) -> Result<Option<String>, String> {
    let (branches, tags) = split(refs);
    for (group, base) in [(&branches, ["branch", "-D"]), (&tags, ["tag", "-d"])] {
        if !group.is_empty() {
            let names = group.iter().map(|r| r.name.as_str());
            let args: Vec<&str> = base.into_iter().chain(names).collect();
            ops::run(repo, &args)?;
        }
    }
    Ok(Some(format!("Deleted {}.", summary(&branches, &tags))))
}

/// Says what a rebase does and how to undo it.
pub fn rebase_warning(branch: &str, onto: &str) -> String {
    format!(
        "This replays the commits of {branch} on top of {onto} and gives them new hashes. \
         To undo it afterwards, run `git reset --hard ORIG_HEAD`."
    )
}

/// Deletes a local branch, which git refuses to do for unmerged work unless forced.
pub fn delete_branch(repo: &Path, name: &str) -> Result<Option<String>, String> {
    ops::run(repo, &["branch", "-D", name]).map(|_| Some(format!("Deleted the branch {name}.")))
}

/// Makes a branch at `start` (the checked-out commit when `None`) and switches to it. The
/// name is checked first, so that git's own refusal isn't the first word on a typo.
pub fn create_branch(
    repo: &Path,
    name: &str,
    start: Option<&str>,
) -> Result<Option<String>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name the new branch first.".to_owned());
    }
    if ops::run(repo, &["check-ref-format", "--branch", name]).is_err() {
        return Err(format!(
            "{name} can't be a branch name: it can't have spaces, `~`, `^`, `:`, `..` \
             or end in `.lock`."
        ));
    }
    if ops::resolves(repo, &format!("refs/heads/{name}")) {
        return Err(format!("A branch called {name} already exists."));
    }
    let before = current_branch(repo);
    let mut args = vec!["switch", "-c", name];
    args.extend(start);
    ops::run(repo, &args)?;
    let undo = match before {
        Some(before) => format!("git switch {before} && git branch -D {name}"),
        None => format!("git switch --detach - && git branch -D {name}"),
    };
    Ok(Some(format!(
        "Created {name} and switched to it. To undo it, run `{undo}`."
    )))
}

pub fn delete_tag(repo: &Path, name: &str) -> Result<Option<String>, String> {
    ops::run(repo, &["tag", "-d", name]).map(|_| Some(format!("Deleted the tag {name}.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    /// `main` with one commit, and `side` one commit ahead of it.
    fn ahead() -> Repo {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b", "2\n");
        repo.commit("add b");
        repo.git(&["checkout", "-q", "main"]);
        repo
    }

    fn diverge(repo: &Repo) {
        repo.write("c", "3\n");
        repo.commit("add c");
    }

    fn branch(name: &str) -> Dragged {
        Dragged {
            kind: RefKind::Branch,
            name: name.into(),
        }
    }

    #[test]
    fn a_new_branch_starts_where_asked_and_is_switched_to() {
        let repo = ahead();
        let side = repo.git(&["rev-parse", "side"]);
        let said = create_branch(repo.path(), " topic ", Some("side")).unwrap();
        assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "topic");
        assert_eq!(repo.git(&["rev-parse", "topic"]), side);
        assert!(
            said.unwrap()
                .contains("git switch main && git branch -D topic")
        );
        // Without a start it grows from where you are.
        create_branch(repo.path(), "next", None).unwrap();
        assert_eq!(repo.git(&["rev-parse", "next"]), side);
    }

    #[test]
    fn bad_and_taken_names_are_refused_in_words() {
        let repo = ahead();
        for bad in ["", "  ", "two words", "a..b", "x.lock"] {
            assert!(create_branch(repo.path(), bad, None).is_err(), "{bad:?}");
        }
        let taken = create_branch(repo.path(), "side", None).unwrap_err();
        assert!(taken.contains("already exists"), "{taken}");
        assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "main");
    }

    #[test]
    fn histories_are_told_apart_by_what_each_has() {
        let repo = ahead();
        let path = repo.path();
        assert_eq!(relation(path, "side", "main"), Ok(Relation::FastForward));
        assert_eq!(relation(path, "main", "side"), Ok(Relation::Contained));
        assert_eq!(relation(path, "main", "main"), Ok(Relation::Same));
        diverge(&repo);
        assert_eq!(relation(path, "side", "main"), Ok(Relation::Diverged));
        assert!(relation(path, "nope", "main").is_err());
    }

    #[test]
    fn dropping_offers_only_what_fits_the_histories() {
        let actions = |relation, source: &Dragged| -> Vec<Option<DropAction>> {
            drop_choices(relation, source, "main")
                .iter()
                .map(|c| c.action)
                .collect()
        };
        assert_eq!(
            actions(Relation::FastForward, &branch("side")),
            [Some(DropAction::FastForward), Some(DropAction::MergeCommit)]
        );
        assert_eq!(
            actions(Relation::Diverged, &branch("side")),
            [Some(DropAction::Merge), Some(DropAction::Rebase)]
        );
        let tag = Dragged {
            kind: RefKind::Tag,
            name: "v1".into(),
        };
        assert_eq!(actions(Relation::Diverged, &tag), [Some(DropAction::Merge)]);
        assert_eq!(actions(Relation::Contained, &tag), [None]);
        let labels = drop_choices(Relation::Diverged, &branch("side"), "main");
        assert_eq!(labels[0].label, "Merge side into main");
        assert_eq!(labels[1].label, "Rebase side onto main");
    }

    #[test]
    fn fast_forwarding_the_checked_out_branch_moves_the_working_tree() {
        let repo = ahead();
        let message = fast_forward(repo.path(), "side", "main").unwrap().unwrap();
        assert!(message.contains("git reset --hard"));
        assert!(repo.path().join("b").exists());
        assert_eq!(
            repo.git(&["rev-parse", "main"]),
            repo.git(&["rev-parse", "side"])
        );
    }

    #[test]
    fn fast_forwarding_another_branch_leaves_the_working_tree_alone() {
        let repo = ahead();
        repo.git(&["checkout", "-q", "-b", "other"]);
        fast_forward(repo.path(), "side", "main").unwrap();
        assert_eq!(
            repo.git(&["rev-parse", "main"]),
            repo.git(&["rev-parse", "side"])
        );
        assert_eq!(repo.git(&["branch", "--show-current"]), "other");
        assert!(!repo.path().join("b").exists());
    }

    #[test]
    fn merging_into_another_branch_switches_to_it_first() {
        let repo = ahead();
        diverge(&repo);
        repo.git(&["checkout", "-q", "side"]);
        let message = merge(repo.path(), "main", "main", false).unwrap();
        assert!(message.unwrap().starts_with("Merged main into main"));
        assert_eq!(repo.git(&["branch", "--show-current"]), "main");
        merge(repo.path(), "side", "main", false).unwrap();
        assert!(repo.path().join("b").exists() && repo.path().join("c").exists());
        assert_eq!(
            repo.git(&["rev-list", "--parents", "-1", "main"])
                .split(' ')
                .count(),
            3
        );
    }

    #[test]
    fn a_merge_commit_is_made_even_when_fast_forward_is_possible() {
        let repo = ahead();
        merge(repo.path(), "side", "main", true).unwrap();
        assert_eq!(
            repo.git(&["rev-list", "--parents", "-1", "main"])
                .split(' ')
                .count(),
            3
        );
    }

    #[test]
    fn a_conflicting_merge_is_left_for_the_conflict_screen() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("a", "side\n");
        repo.commit("side");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("a", "main\n");
        repo.commit("main");

        assert_eq!(merge(repo.path(), "side", "main", false), Ok(None));
        assert!(conflict::detect(repo.path()).is_some());
    }

    #[test]
    fn rebasing_a_branch_onto_another_replays_it_from_wherever_you_are() {
        let repo = ahead();
        diverge(&repo);
        apply_drop(repo.path(), DropAction::Rebase, "side", "main").unwrap();
        assert_eq!(repo.git(&["branch", "--show-current"]), "side");
        assert_eq!(repo.git(&["rev-list", "--count", "main..side"]), "1");
        assert_eq!(repo.git(&["rev-list", "--count", "side..main"]), "0");
    }

    #[test]
    fn remote_branches_become_local_ones_and_tags_detach() {
        let repo = ahead();
        repo.git(&["remote", "add", "origin", "."]);
        repo.git(&["update-ref", "refs/remotes/origin/feat", "side"]);
        let message = check_out_ref(repo.path(), RefKind::Remote, "origin/feat").unwrap();
        assert!(message.unwrap().contains("follows origin/feat"));
        assert_eq!(repo.git(&["branch", "--show-current"]), "feat");

        repo.git(&["switch", "-q", "main"]);
        // A second time there is a local branch to switch to.
        assert_eq!(
            check_out_ref(repo.path(), RefKind::Remote, "origin/feat"),
            Ok(None)
        );
        assert_eq!(repo.git(&["branch", "--show-current"]), "feat");

        repo.git(&["tag", "v1", "main"]);
        check_out_ref(repo.path(), RefKind::Tag, "v1").unwrap();
        assert_eq!(repo.git(&["branch", "--show-current"]), "");
    }

    #[test]
    fn deleting_says_how_to_get_the_branch_back() {
        let repo = ahead();
        let tip = repo.git(&["rev-parse", "side"]);
        let warning = delete_warning(RefKind::Branch, "side", Some(&tip));
        assert!(warning.contains(&format!("git branch side {}", &tip[..7])));
        delete_branch(repo.path(), "side").unwrap();
        assert!(!ops::resolves(repo.path(), "refs/heads/side"));

        repo.git(&["tag", "v1"]);
        assert!(delete_warning(RefKind::Tag, "v1", None).starts_with("This deletes the tag v1."));
        delete_tag(repo.path(), "v1").unwrap();
        assert!(!ops::resolves(repo.path(), "refs/tags/v1"));
    }

    #[test]
    fn several_refs_are_deleted_together_and_the_warning_lists_each_undo() {
        let repo = ahead();
        repo.git(&["branch", "other", "main"]);
        repo.git(&["tag", "v1"]);
        let doomed = |kind, name: &str| Doomed {
            kind,
            name: name.into(),
            tip: Some(repo.git(&["rev-parse", name])),
            at_risk: false,
        };
        let refs = [
            doomed(RefKind::Branch, "side"),
            doomed(RefKind::Branch, "other"),
            doomed(RefKind::Tag, "v1"),
        ];
        let warning = delete_many_warning(&refs);
        assert!(warning.starts_with("This deletes 2 branches and 1 tag: side, other, v1."));
        assert!(warning.contains("`git branch other "));
        assert!(warning.contains("`git tag v1 "));

        assert_eq!(
            delete_many(repo.path(), &refs),
            Ok(Some("Deleted 2 branches and 1 tag.".into()))
        );
        for name in ["refs/heads/side", "refs/heads/other", "refs/tags/v1"] {
            assert!(!ops::resolves(repo.path(), name));
        }
    }
}
