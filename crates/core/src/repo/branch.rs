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

/// Where a local branch is tracked: its remote and the branch's name there.
fn tracked(repo: &Path, branch: &str) -> Result<(String, String), String> {
    let config = |key: &str| {
        ops::run(
            repo,
            &["config", "--get", &format!("branch.{branch}.{key}")],
        )
        .map(|out| out.trim().to_owned())
        .ok()
        .filter(|value| !value.is_empty())
    };
    let none = || format!("{branch} follows no remote branch.");
    let remote = config("remote").ok_or_else(none)?;
    let merge = config("merge").ok_or_else(none)?;
    let name = merge
        .strip_prefix("refs/heads/")
        .unwrap_or(&merge)
        .to_owned();
    Ok((remote, name))
}

/// Brings a local branch up to the remote branch it follows, without checking it out. Only
/// a fast-forward is done, so nothing is merged and the working tree stays as it is.
pub fn pull_branch(repo: &Path, branch: &str) -> Result<Option<String>, String> {
    let (remote, name) = tracked(repo, branch)?;
    let old = rev_parse(repo, &format!("refs/heads/{branch}"))?;
    if current_branch(repo).as_deref() == Some(branch) {
        ops::run(repo, &["pull", "--ff-only"])?;
    } else {
        let refspec = format!("refs/heads/{name}:refs/heads/{branch}");
        ops::run(repo, &["fetch", "-q", &remote, &refspec])?;
    }
    let new = rev_parse(repo, &format!("refs/heads/{branch}"))?;
    if new == old {
        return Ok(Some(format!(
            "{branch} is already up to date with {remote}/{name}."
        )));
    }
    Ok(Some(format!(
        "Pulled {remote}/{name} into {branch}. To undo it, run `git branch -f {branch} {}`.",
        short_oid(&old)
    )))
}

/// Sends a local branch to the remote branch it follows, without checking it out.
pub fn push_branch(repo: &Path, branch: &str) -> Result<Option<String>, String> {
    let (remote, name) = tracked(repo, branch)?;
    push_to(repo, branch, &remote, &name)
}

/// Sends a local branch to the branch `name` on `remote`, as a fast-forward only: git
/// refuses anything else, and the app never forces.
pub fn push_to(
    repo: &Path,
    branch: &str,
    remote: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let before = rev_parse(repo, &format!("refs/remotes/{remote}/{name}")).ok();
    let refspec = format!("refs/heads/{branch}:refs/heads/{name}");
    ops::run(repo, &["push", remote, &refspec])?;
    Ok(Some(match before {
        Some(before) => format!(
            "Pushed {branch} to {remote}/{name}. To undo it, run \
             `git push --force-with-lease {remote} {}:refs/heads/{name}`.",
            short_oid(&before)
        ),
        None => format!("Pushed {branch} to {remote}/{name}."),
    }))
}

/// Whether git turned a push away because the remote branch has commits the pushed one
/// lacks, which only a force can get past. `error` is what git said.
pub fn push_was_rejected(error: &str) -> bool {
    error.contains("[rejected]")
        && [
            "non-fast-forward",
            "fetch first",
            "stale info",
            "tip is behind",
        ]
        .iter()
        .any(|reason| error.contains(reason))
}

/// Sends a local branch to the branch `name` on `remote`, replacing what is there. The
/// push only goes through if the remote branch is still where the last fetch saw it, so
/// someone else's newer commits are never overwritten without the app knowing.
pub fn force_push_to(
    repo: &Path,
    branch: &str,
    remote: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let before = rev_parse(repo, &format!("refs/remotes/{remote}/{name}")).ok();
    // No `+` here: it would make the push unconditional and the lease would not be checked.
    let refspec = format!("refs/heads/{branch}:refs/heads/{name}");
    let lease = match &before {
        Some(before) => format!("--force-with-lease=refs/heads/{name}:{before}"),
        None => format!("--force-with-lease=refs/heads/{name}"),
    };
    ops::run(repo, &["push", &lease, remote, &refspec])?;
    Ok(Some(match before {
        Some(before) => format!(
            "Force pushed {branch} to {remote}/{name}. To undo it, run \
             `git push --force-with-lease {remote} {}:refs/heads/{name}`.",
            short_oid(&before)
        ),
        None => format!("Force pushed {branch} to {remote}/{name}."),
    }))
}

/// What a force push does, for the confirmation. It says how to undo it when the remote
/// branch is known locally.
pub fn force_push_warning(repo: &Path, branch: &str, remote: &str, name: &str) -> String {
    let undo = rev_parse(repo, &format!("refs/remotes/{remote}/{name}"))
        .map(|before| {
            format!(
                " To undo it, run `git push --force-with-lease {remote} {}:refs/heads/{name}`.",
                short_oid(&before)
            )
        })
        .unwrap_or_else(|_| {
            " The commits it replaces aren't in this repository, so fetch first if you want a \
             way back."
                .into()
        });
    format!(
        "Git refused the push because {remote}/{name} has commits {branch} lacks. Forcing it \
         replaces them for everyone who uses the remote.{undo}"
    )
}

/// The remote and branch a plain push of `branch` goes to, which is where it follows.
pub fn push_target(repo: &Path, branch: &str) -> Result<(String, String), String> {
    tracked(repo, branch)
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
    /// For a remote branch: its remote and its name there (`origin`, `feat/x`).
    pub on_remote: Option<(String, String)>,
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
            Some(match (&r.kind, &r.on_remote) {
                (_, Some((remote, name))) => {
                    format!("`git push {remote} {tip}:refs/heads/{name}`")
                }
                (RefKind::Tag, _) => format!("`git tag {} {tip}`", r.name),
                _ => format!("`git branch {} {tip}`", r.name),
            })
        })
        .collect();
    let on_remote = refs.iter().any(|r| r.on_remote.is_some());
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
    let shared = if on_remote {
        " Remote branches go for everyone who uses the remote."
    } else {
        ""
    };
    format!(
        "This deletes {}: {}.{shared}{stay}{undo}",
        summary(&branches, &tags),
        names.join(", ")
    )
}

/// Deletes branches and tags in one go. Branches go first, in one git call.
pub fn delete_many(repo: &Path, refs: &[Doomed]) -> Result<Option<String>, String> {
    let (branches, tags) = split(refs);
    let (remote, local): (Vec<&Doomed>, Vec<&Doomed>) =
        branches.iter().partition(|r| r.on_remote.is_some());
    for (group, base) in [(&local, ["branch", "-D"]), (&tags, ["tag", "-d"])] {
        if !group.is_empty() {
            let names = group.iter().map(|r| r.name.as_str());
            let args: Vec<&str> = base.into_iter().chain(names).collect();
            ops::run(repo, &args)?;
        }
    }
    // One push per remote, which also drops the local copies.
    let mut remotes: Vec<&str> = remote
        .iter()
        .filter_map(|r| r.on_remote.as_ref().map(|(remote, _)| remote.as_str()))
        .collect();
    remotes.dedup();
    for name in remotes {
        let names = remote.iter().filter_map(|r| match &r.on_remote {
            Some((on, branch)) if on == name => Some(branch.as_str()),
            _ => None,
        });
        let args: Vec<&str> = ["push", name, "--delete"]
            .into_iter()
            .chain(names)
            .collect();
        ops::run(repo, &args)?;
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

/// Makes the local branch `branch` follow `name` on `remote`. A branch the remote doesn't
/// have yet is pushed first, which is what makes it exist there.
pub fn set_upstream(
    repo: &Path,
    branch: &str,
    remote: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let (remote, name) = (remote.trim(), name.trim());
    let upstream = format!("{remote}/{name}");
    let before = ops::run(
        repo,
        &[
            "rev-parse",
            "--abbrev-ref",
            &format!("{branch}@{{upstream}}"),
        ],
    )
    .ok()
    .map(|found| found.trim().to_owned())
    .filter(|found| !found.is_empty());
    let exists = ops::resolves(repo, &format!("refs/remotes/{upstream}"));
    if exists {
        ops::run(
            repo,
            &["branch", &format!("--set-upstream-to={upstream}"), branch],
        )?;
    } else {
        ops::run(
            repo,
            &[
                "push",
                "--set-upstream",
                remote,
                &format!("{branch}:{name}"),
            ],
        )?;
        // A repository that fetches only some branches (a single-branch clone) keeps no
        // local copy of what a push made, and a fetch would never bring it in, so the
        // branch would be missing from the app. Fetching it is what a push does here.
        if !ops::resolves(repo, &format!("refs/remotes/{upstream}")) {
            ops::run(repo, &["remote", "set-branches", "--add", remote, name])?;
            ops::run(repo, &["fetch", remote, name])?;
        }
    }
    let back = match before {
        Some(before) => format!("git branch --set-upstream-to={before} {branch}"),
        None => format!("git branch --unset-upstream {branch}"),
    };
    Ok(Some(if exists {
        format!("{branch} now follows {upstream}. To undo it, run `{back}`.")
    } else {
        format!(
            "Pushed {branch} to {remote} as {name}, and it now follows {upstream}. To undo it, \
             run `git push {remote} --delete {name} && {back}`."
        )
    }))
}

/// What deleting a branch on a remote does, and how to put it back. It is the one
/// deletion that reaches other people, so it says so.
pub fn delete_remote_warning(remote: &str, name: &str, tip: Option<&str>) -> String {
    let undo = tip.map_or(String::new(), |tip| {
        format!(
            " To undo it, run `git push {remote} {}:refs/heads/{name}`.",
            short_oid(tip)
        )
    });
    format!(
        "This deletes the branch {name} on {remote}, for everyone who uses it. Your local \
         branches stay.{undo}"
    )
}

/// Deletes a branch on its remote, which also drops the local copy of it.
pub fn delete_remote_branch(
    repo: &Path,
    remote: &str,
    name: &str,
) -> Result<Option<String>, String> {
    ops::run(repo, &["push", remote, "--delete", name])
        .map(|_| Some(format!("Deleted {name} on {remote}.")))
}

/// Says what deleting a tag on its remote does and how to bring it back.
pub fn delete_remote_tag_warning(
    remote: &str,
    name: &str,
    tip: Option<&str>,
    also_local: bool,
) -> String {
    let undo = tip.map_or(String::new(), |tip| {
        let tip = short_oid(tip);
        if also_local {
            format!(" To undo it, run `git tag {name} {tip}` and `git push {remote} {name}`.")
        } else {
            format!(" To undo it, run `git push {remote} {name}`.")
        }
    });
    let here = if also_local {
        " The local tag goes too."
    } else {
        " The local tag stays."
    };
    format!("This deletes the tag {name} on {remote}, for everyone who uses it.{here}{undo}")
}

/// Sends a tag to a remote. Git refuses to move a tag the remote already has elsewhere.
pub fn push_tag(repo: &Path, remote: &str, name: &str) -> Result<Option<String>, String> {
    let refspec = format!("refs/tags/{name}:refs/tags/{name}");
    ops::run(repo, &["push", remote, &refspec]).map(|_| {
        Some(format!(
            "Pushed the tag {name} to {remote}. To undo it, run \
             `git push {remote} --delete {name}`."
        ))
    })
}

/// Deletes a tag on a remote, and the local one too when `also_local`.
pub fn delete_remote_tag(
    repo: &Path,
    remote: &str,
    name: &str,
    also_local: bool,
) -> Result<Option<String>, String> {
    let refname = format!("refs/tags/{name}");
    ops::run(repo, &["push", remote, "--delete", &refname])?;
    if also_local {
        ops::run(repo, &["tag", "-d", name])?;
        return Ok(Some(format!(
            "Deleted the tag {name} here and on {remote}."
        )));
    }
    Ok(Some(format!("Deleted the tag {name} on {remote}.")))
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
    fn a_branch_can_follow_a_remote_branch() {
        let repo = ahead();
        repo.git(&["remote", "add", "origin", "/nowhere"]);
        repo.git(&["update-ref", "refs/remotes/origin/side", "side"]);
        let said = set_upstream(repo.path(), "side", "origin", " side ")
            .unwrap()
            .unwrap();
        assert_eq!(
            repo.git(&["rev-parse", "--abbrev-ref", "side@{upstream}"])
                .trim(),
            "origin/side"
        );
        assert!(said.contains("--unset-upstream side"), "{said}");
    }

    #[test]
    fn a_branch_the_remote_lacks_is_pushed_and_followed() {
        let repo = ahead();
        let remote = Repo::new();
        repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        remote.git(&["config", "receive.denyCurrentBranch", "ignore"]);
        let said = set_upstream(repo.path(), "side", "origin", "topic")
            .unwrap()
            .unwrap();
        assert_eq!(
            repo.git(&["rev-parse", "--abbrev-ref", "side@{upstream}"])
                .trim(),
            "origin/topic"
        );
        assert_eq!(
            remote.git(&["rev-parse", "topic"]),
            repo.git(&["rev-parse", "side"])
        );
        assert!(said.contains("push origin --delete topic"), "{said}");
    }

    #[test]
    fn a_pushed_branch_shows_up_even_when_the_repository_fetches_only_some_branches() {
        let repo = ahead();
        let remote = Repo::new();
        repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        repo.git(&[
            "config",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ]);
        remote.git(&["config", "receive.denyCurrentBranch", "ignore"]);
        set_upstream(repo.path(), "side", "origin", "feat/x/12345").unwrap();
        let read = crate::history::refs::read_refs(repo.path()).unwrap();
        let names: Vec<_> = read.remotes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["origin/feat/x/12345"]);
        assert_eq!(
            repo.git(&["rev-parse", "--abbrev-ref", "side@{upstream}"])
                .trim(),
            "origin/feat/x/12345"
        );
        // So a later fetch keeps it up to date.
        let wanted = repo.git(&["config", "--get-all", "remote.origin.fetch"]);
        assert!(wanted.contains("refs/heads/feat/x/12345"), "{wanted}");
    }

    #[test]
    fn a_branch_that_is_not_checked_out_can_be_pulled_and_pushed() {
        let repo = ahead();
        let remote = Repo::new();
        repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        remote.git(&["config", "receive.denyCurrentBranch", "ignore"]);
        set_upstream(repo.path(), "side", "origin", "side").unwrap();

        // Another clone moves the remote's `side` forward.
        let other = Repo::new();
        other.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        other.git(&["fetch", "-q", "origin", "side"]);
        other.git(&["checkout", "-q", "-b", "side", "FETCH_HEAD"]);
        other.write("d", "4\n");
        other.commit("add d");
        other.git(&["push", "-q", "origin", "side"]);

        let old = repo.git(&["rev-parse", "side"]);
        let said = pull_branch(repo.path(), "side").unwrap().unwrap();
        assert_eq!(
            repo.git(&["rev-parse", "side"]),
            other.git(&["rev-parse", "side"])
        );
        assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "main");
        assert!(said.contains(&format!("git branch -f side {}", short_oid(old.trim()))));

        // Commits made on `side` go to the remote without leaving `main`.
        repo.git(&["checkout", "-q", "side"]);
        repo.write("e", "5\n");
        repo.commit("add e");
        repo.git(&["checkout", "-q", "main"]);
        push_branch(repo.path(), "side").unwrap();
        assert_eq!(
            remote.git(&["rev-parse", "side"]),
            repo.git(&["rev-parse", "side"])
        );
    }

    #[test]
    fn a_rejected_push_can_be_forced_and_says_how_to_undo_it() {
        let repo = ahead();
        let remote = Repo::new();
        repo.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        remote.git(&["config", "receive.denyCurrentBranch", "ignore"]);
        set_upstream(repo.path(), "side", "origin", "side").unwrap();

        // Another clone puts a commit on the remote's `side` that this one lacks.
        let other = Repo::new();
        other.git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        other.git(&["fetch", "-q", "origin", "side"]);
        other.git(&["checkout", "-q", "-b", "side", "FETCH_HEAD"]);
        other.write("d", "4\n");
        other.commit("add d");
        other.git(&["push", "-q", "origin", "side"]);

        // This clone commits on `side` too, so the two have diverged.
        repo.git(&["checkout", "-q", "side"]);
        repo.write("e", "5\n");
        repo.commit("add e");
        repo.git(&["checkout", "-q", "main"]);

        let error = push_branch(repo.path(), "side").unwrap_err();
        assert!(push_was_rejected(&error), "{error}");
        assert!(!push_was_rejected("fatal: could not read from remote"));

        let warning = force_push_warning(repo.path(), "side", "origin", "side");
        assert!(
            warning.contains("origin/side has commits side lacks"),
            "{warning}"
        );

        // The lease is what the last fetch saw, so a remote that moved since is not forced.
        repo.git(&["fetch", "-q", "origin"]);
        other.write("f", "6\n");
        other.commit("add f");
        other.git(&["push", "-q", "origin", "side"]);
        assert!(force_push_to(repo.path(), "side", "origin", "side").is_err());
        repo.git(&["fetch", "-q", "origin"]);
        let said = force_push_to(repo.path(), "side", "origin", "side")
            .unwrap()
            .unwrap();
        assert!(
            said.contains("git push --force-with-lease origin"),
            "{said}"
        );
        assert_eq!(
            remote.git(&["rev-parse", "side"]),
            repo.git(&["rev-parse", "side"])
        );
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
    fn a_tag_is_pushed_then_deleted_on_the_remote_and_optionally_here() {
        let repo = ahead();
        let remote = Repo::new();
        remote.git(&["init", "-q", "--bare", "-b", "main"]);
        let url = remote.path().to_str().unwrap();
        repo.git(&["remote", "add", "origin", url]);
        repo.git(&["tag", "v1"]);
        repo.git(&["tag", "v2"]);

        push_tag(repo.path(), "origin", "v1").unwrap();
        push_tag(repo.path(), "origin", "v2").unwrap();
        assert!(remote.git(&["tag"]).contains("v1"));

        delete_remote_tag(repo.path(), "origin", "v1", false).unwrap();
        assert!(!remote.git(&["tag"]).contains("v1"));
        assert!(ops::resolves(repo.path(), "refs/tags/v1"));

        delete_remote_tag(repo.path(), "origin", "v2", true).unwrap();
        assert!(!remote.git(&["tag"]).contains("v2"));
        assert!(!ops::resolves(repo.path(), "refs/tags/v2"));
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
            on_remote: None,
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
