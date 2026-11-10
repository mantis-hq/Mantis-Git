//! A repository's refs: its branches, tags and remote branches, where HEAD is, and the
//! colour each branch takes. Read in one go with [`read_refs`], before history streams in.

use super::lanes::{LaneColor, Plan};
use super::{Commit, CommitActions};
use crate::ops;
use crate::repo::forge::{self, Forge};
use crate::repo::remote::{self, Remote};
use crate::repo::status::{self, RepoStatus};
use crate::repo::submodule::{self, Submodule};
use crate::repo::worktree::{self, Worktree};
use crate::text::count;
use crate::working::version::{self, Version};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a default branch usually goes by, in the order they're tried.
const USUAL_DEFAULT_BRANCHES: [&str; 3] = ["main", "master", "trunk"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    /// HEAD pointing straight at a commit.
    DetachedHead,
    Branch,
    Tag,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub kind: RefKind,
    /// The name without its `refs/...` prefix.
    pub name: String,
    /// Whether this is the branch HEAD is on.
    pub is_head: bool,
    /// The branch's colour, the same in the lanes and the chips. Unused for tags.
    pub color: LaneColor,
}

impl Ref {
    /// The branch this is, or follows: a remote's `origin/main` is `main`.
    pub fn branch_name(&self) -> &str {
        branch_key(self.kind, &self.name)
    }
}

/// A ref as drawn: a branch and its remote at the same commit are one chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chip<'a> {
    pub label: &'a Ref,
    /// A remote has this branch at the same commit.
    pub synced: bool,
}

/// The chips for the refs at one commit.
pub fn chips(refs: &[Ref]) -> Vec<Chip<'_>> {
    let local = |remote: &Ref| {
        refs.iter()
            .any(|r| r.kind == RefKind::Branch && r.name == remote.branch_name())
    };
    let tracked = |branch: &Ref| {
        refs.iter()
            .any(|r| r.kind == RefKind::Remote && r.branch_name() == branch.name)
    };
    refs.iter()
        .filter(|r| r.kind != RefKind::Remote || !local(r))
        .map(|label| Chip {
            label,
            synced: label.kind == RefKind::Branch && tracked(label),
        })
        .collect()
}

/// A local branch, for the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub tip: Arc<str>,
    pub color: LaneColor,
    pub is_head: bool,
    /// Commits this has that the default branch doesn't, and the other way around.
    pub ahead_behind: Option<(u32, u32)>,
    /// Commits this has that its remote branch doesn't, and the other way around. `None`
    /// when it tracks nothing, or the remote branch is gone.
    pub upstream: Option<(u32, u32)>,
    /// The remote branch it tracks (`origin/feat/x`), even when that branch is gone.
    pub upstream_name: Option<String>,
}

impl Branch {
    /// How the branch stands against the remote it tracks: `↑2 ↓1`, or nothing when level
    /// or untracked.
    pub fn remote_delta(&self) -> String {
        match self.upstream {
            Some((ahead, behind)) => crate::text::ahead_behind(ahead, behind),
            None => String::new(),
        }
    }
}

/// A remote branch or tag, for the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedRef {
    pub name: String,
    pub tip: Arc<str>,
    pub color: LaneColor,
}

/// The state of a repository's refs at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub by_commit: HashMap<Arc<str>, Vec<Ref>>,
    pub head: Option<Arc<str>>,
    /// The short name of the branch HEAD is on.
    pub head_branch: Option<String>,
    /// The branch history is shared on, usually `main`.
    pub default_branch: Option<String>,
    /// HEAD's branch, then the default branch, then the rest by name.
    pub branches: Vec<Branch>,
    pub remotes: Vec<NamedRef>,
    pub tags: Vec<NamedRef>,
    /// Stash subjects, newest first.
    pub stashes: Vec<String>,
    /// The commit of each stash, in the order of `stashes`.
    pub stash_commits: Vec<Arc<str>>,
    /// What keeps a stash's helper commits out of the graph.
    pub(super) stash_graph: StashGraph,
    /// The repository's working trees, main one first. Empty when git can't say.
    pub worktrees: Vec<Worktree>,
    /// The repositories this one pins, by path. Empty when it has none.
    pub submodules: Vec<Submodule>,
    /// The remotes the repository is set up with, which can have no branches yet.
    pub remote_configs: Vec<Remote>,
    /// The forge chosen for the repository when it can't be told, `gitclient.forge`.
    pub forge: Option<Forge>,
    /// How to lay out the graph for this repository.
    pub plan: Plan,
    /// The top of the working tree, or the repository itself when it is bare.
    pub workdir: Option<PathBuf>,
    pub status: RepoStatus,
    /// The commits to walk history from.
    pub(super) tips: Vec<Arc<str>>,
    /// Compares equal exactly when no ref moved.
    pub(super) fingerprint: String,
    /// The default branch's full ref name and the commit it is at, which the counts in
    /// `Branch::ahead_behind` are measured against.
    default_base: Option<(String, Arc<str>)>,
}

impl Refs {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Whether some branch still lacks its count against the default branch. `read_refs`
    /// leaves those out because git has to walk history to count them, which takes seconds
    /// in a big repository without a commit-graph; `read_counts` fills them in.
    pub fn needs_counts(&self) -> bool {
        self.default_base.is_some() && self.branches.iter().any(|b| b.ahead_behind.is_none())
    }

    /// Takes counts from `old` for branches that haven't moved since, when the default
    /// branch hasn't either, so a reload doesn't blank the sidebar while it counts again.
    pub fn keep_counts_from(&mut self, old: &Refs) {
        let same_base = match (&self.default_base, &old.default_base) {
            (Some((name, tip)), Some((old_name, old_tip))) => name == old_name && tip == old_tip,
            _ => false,
        };
        if !same_base {
            return;
        }
        for branch in self
            .branches
            .iter_mut()
            .filter(|b| b.ahead_behind.is_none())
        {
            branch.ahead_behind = old
                .branches
                .iter()
                .find(|o| o.name == branch.name && o.tip == branch.tip)
                .and_then(|o| o.ahead_behind);
        }
    }

    /// Fills in the counts `read_counts` found.
    pub fn set_counts(&mut self, counts: &HashMap<String, (u32, u32)>) {
        for branch in &mut self.branches {
            if let Some(&found) = counts.get(&branch.name) {
                branch.ahead_behind = Some(found);
            }
        }
    }

    pub fn is_head(&self, oid: &Arc<str>) -> bool {
        self.head.as_ref() == Some(oid)
    }

    /// What to call where HEAD is: its branch, when it's on one.
    pub fn head_name(&self) -> &str {
        self.head_branch.as_deref().unwrap_or("HEAD")
    }

    /// The chips for the refs at a commit.
    pub fn chips_at(&self, oid: &str) -> Vec<Chip<'_>> {
        chips(self.by_commit.get(oid).map_or(&[], Vec::as_slice))
    }

    /// The first branch at a commit.
    fn branch_at(&self, oid: &str) -> Option<String> {
        let refs = self.by_commit.get(oid)?;
        refs.iter()
            .find(|r| r.kind == RefKind::Branch)
            .map(|r| r.name.clone())
    }

    pub fn actions_for(&self, commit: &Commit) -> CommitActions {
        let is_head = self.is_head(&commit.oid);
        CommitActions {
            branch: self.branch_at(&commit.oid),
            can_check_out: !is_head,
            can_cherry_pick: !is_head && !commit.is_merge(),
            is_merge: commit.is_merge(),
        }
    }

    /// A branch's colour, or the default branch's for a name that isn't a branch here.
    pub fn branch_color(&self, name: &str) -> LaneColor {
        self.branches
            .iter()
            .find(|branch| branch.name == name)
            .map_or(LaneColor::default(), |branch| branch.color)
    }

    /// The highest `v1.2.3` style tag.
    pub fn latest_version(&self) -> Option<Version> {
        version::latest_version(self.tags.iter().map(|tag| tag.name.as_str()))
    }

    /// Says what the working changes are, in plain words.
    pub fn working_sentence(&self) -> String {
        format!(
            "{} changed, not committed yet. These sit on top of {}.",
            count(self.status.changed, "file"),
            self.head_name()
        )
    }
}

/// How stashes sit in the graph: as a commit on the one they were made on, without the
/// index and untracked commits stored beside them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct StashGraph {
    hidden: HashSet<Arc<str>>,
    /// Each stash's commit, and the commit it was made on.
    bases: HashMap<Arc<str>, Arc<str>>,
}

impl StashGraph {
    /// Drops the helper commits and gives each stash only its base as a parent.
    pub(super) fn tidy(&self, commits: Vec<Commit>) -> Vec<Commit> {
        if self.bases.is_empty() {
            return commits;
        }
        commits
            .into_iter()
            .filter(|commit| !self.hidden.contains(&commit.oid))
            .map(|mut commit| {
                if self.bases.contains_key(&commit.oid) {
                    commit.parents = commit.parents.iter().take(1).cloned().collect();
                }
                commit
            })
            .collect()
    }
}

impl Refs {
    /// The number of the stash whose commit this is.
    pub fn stash_at(&self, oid: &str) -> Option<usize> {
        self.stash_commits
            .iter()
            .position(|commit| &**commit == oid)
    }
}

pub fn read_refs(repo: &Path) -> Result<Refs, String> {
    const FORMAT: &str =
        "%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(refname)%00%(symref)";
    // These read different things and none needs another's answer, so they run together.
    // Each starts its own process, which is what costs time, most of all under WSL. Only
    // `worktrees` and `submodules` wait, for the folder `toplevel` finds.
    fn gone<T>() -> T {
        panic!("a read stopped")
    }
    let (listing, head_ref, head, status, remote_configs, forge, workdir, worktrees, submodules) =
        std::thread::scope(|scope| {
            let listing = scope.spawn(|| {
                ops::run(
                    repo,
                    &[
                        "for-each-ref",
                        &format!("--format={FORMAT}"),
                        // Narrower than --all: stashes and notes are not history anyone wants drawn.
                        "refs/heads",
                        "refs/remotes",
                        "refs/tags",
                    ],
                )
            });
            // Both of these fail, as expected, in some states: HEAD can be detached or unborn.
            let head_ref = scope.spawn(|| ops::run(repo, &["symbolic-ref", "-q", "HEAD"]).ok());
            let head = scope
                .spawn(|| ops::run(repo, &["rev-parse", "-q", "--verify", "HEAD^{commit}"]).ok());
            let toplevel = scope.spawn(|| ops::run(repo, &["rev-parse", "--show-toplevel"]).ok());
            let status = scope.spawn(|| status::read(repo));
            let remote_configs = scope.spawn(|| remote::list(repo));
            let forge = scope.spawn(|| forge::choice::chosen(repo));

            let workdir = toplevel
                .join()
                .unwrap_or_else(|_| gone())
                .map(|path| crate::repo::wsl::from_git(repo, path.trim_end()))
                .unwrap_or_else(|| repo.to_owned());
            let (for_worktrees, for_submodules) = (workdir.clone(), workdir.clone());
            let worktrees = scope.spawn(move || worktree::list(repo, &for_worktrees));
            let submodules = scope.spawn(move || submodule::list(&for_submodules));
            (
                listing.join().unwrap_or_else(|_| gone()),
                head_ref.join().unwrap_or_else(|_| gone()),
                head.join().unwrap_or_else(|_| gone()),
                status.join().unwrap_or_else(|_| gone()),
                remote_configs.join().unwrap_or_else(|_| gone()),
                forge.join().unwrap_or_else(|_| gone()),
                workdir,
                worktrees.join().unwrap_or_else(|_| gone()),
                submodules.join().unwrap_or_else(|_| gone()),
            )
        });
    let listing = listing?;
    let head_ref = head_ref.map(|name| name.trim().to_owned());
    let head: Option<Arc<str>> = head.map(|oid| oid.trim().into());

    let mut refs = Refs {
        fingerprint: format!(
            "{listing}\0{head_ref:?}\0{head:?}\0{status:?}\0{worktrees:?}\0{submodules:?}\0{remote_configs:?}\0{forge:?}"
        ),
        workdir: Some(workdir),
        worktrees,
        submodules,
        remote_configs,
        forge,
        status,
        ..Refs::default()
    };

    let mut found = Vec::new();
    let mut origin_head = None;
    for line in listing.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [object, object_type, peeled, peeled_type, full_name, symref] = fields[..] else {
            return Err(format!("unexpected output from git for-each-ref: {line:?}"));
        };
        if full_name == "refs/remotes/origin/HEAD" {
            origin_head = symref.strip_prefix("refs/remotes/origin/");
        }
        // Annotated tags point at a tag object; history starts at the commit behind it.
        // Tags can also name trees and blobs, which have no place in the graph.
        let commit = match (object_type, peeled_type) {
            ("commit", _) => object,
            (_, "commit") => peeled,
            _ => continue,
        };
        if let Some((kind, name)) = classify_ref(full_name) {
            found.push((kind, name, commit, full_name));
        }
    }

    let head_branch = head_ref
        .as_deref()
        .and_then(|full_name| full_name.strip_prefix("refs/heads/"));
    let default_branch = default_branch(&found, origin_head);
    let colors = branch_colors(&found, default_branch.as_deref(), head_branch);
    let color_of = |kind: RefKind, name: &str| match kind {
        RefKind::DetachedHead => LaneColor::Head,
        _ => colors
            .get(branch_key(kind, name))
            .copied()
            .unwrap_or_default(),
    };

    for &(kind, name, commit, full_name) in &found {
        let color = color_of(kind, name);
        let oid = intern(&mut refs.tips, commit);
        if kind != RefKind::Tag && color != LaneColor::Head {
            let shared = default_branch.as_deref() == Some(branch_key(kind, name));
            let tip = refs.plan.tips.entry(oid.clone()).or_insert(color);
            if shared {
                *tip = color;
            }
        }
        refs.by_commit.entry(oid).or_default().push(Ref {
            kind,
            name: name.to_owned(),
            is_head: head_ref.as_deref() == Some(full_name),
            color,
        });
    }

    if let Some(head) = &head {
        let oid = intern(&mut refs.tips, head);
        if head_ref.is_none() {
            refs.by_commit.entry(oid).or_default().push(Ref {
                kind: RefKind::DetachedHead,
                name: "HEAD".into(),
                is_head: true,
                color: LaneColor::Head,
            });
        }
    }

    // A branch at the default branch's own commit is level with it, which needs no git. The
    // others are counted later, by `read_counts`.
    let base = default_branch
        .as_deref()
        .and_then(|default| shared_ref(&found, default))
        .map(|base| (base.3.to_owned(), Arc::<str>::from(base.2)));
    refs.default_base = base.clone();
    let upstream = read_upstream(repo);
    for &(kind, name, commit, _) in &found {
        let tip: Arc<str> = commit.into();
        let color = color_of(kind, name);
        match kind {
            RefKind::Branch => refs.branches.push(Branch {
                name: name.to_owned(),
                tip: tip.clone(),
                color,
                is_head: head_branch == Some(name),
                ahead_behind: base
                    .as_ref()
                    .filter(|(_, base_tip)| **base_tip == *tip)
                    .map(|_| (0, 0)),
                upstream: upstream.get(name).and_then(|(_, counts)| *counts),
                upstream_name: upstream.get(name).map(|(found, _)| found.clone()),
            }),
            RefKind::Remote => refs.remotes.push(NamedRef {
                name: name.to_owned(),
                tip,
                color,
            }),
            RefKind::Tag => refs.tags.push(NamedRef {
                name: name.to_owned(),
                tip,
                color,
            }),
            RefKind::DetachedHead => {}
        }
    }
    refs.branches.sort_by_key(|branch| {
        let is_default = default_branch.as_deref() == Some(branch.name.as_str());
        (!branch.is_head, !is_default, branch.name.clone())
    });
    refs.tags.reverse();
    let stashes = ops::run(repo, &["stash", "list", "--format=%H%x00%P%x00%s"]).unwrap_or_default();
    for line in stashes.lines() {
        let mut fields = line.splitn(3, '\0');
        let (Some(oid), Some(parents), Some(subject)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let oid = intern(&mut refs.tips, oid);
        let mut parents = parents.split_ascii_whitespace();
        if let Some(base) = parents.next() {
            refs.stash_graph.bases.insert(oid.clone(), base.into());
        }
        // The index and untracked commits kept under a stash aren't history.
        refs.stash_graph.hidden.extend(parents.map(Arc::from));
        refs.stash_commits.push(oid);
        refs.stashes.push(subject.to_owned());
    }

    refs.plan.fork = default_branch
        .as_deref()
        .filter(|&default| head.is_some() && head_branch != Some(default))
        .and_then(|default| fork_point(repo, shared_ref(&found, default)?.3));
    refs.plan.head = head.clone();
    refs.plan.shared_color = default_branch
        .as_deref()
        .and_then(|default| colors.get(default).copied())
        .filter(|color| *color != LaneColor::Head)
        .unwrap_or_default();
    refs.plan.first_free_color = colors.len() as u32 + 1;
    refs.head = head;
    refs.head_branch = head_branch.map(str::to_owned);
    refs.default_branch = default_branch;

    for commit_refs in refs.by_commit.values_mut() {
        commit_refs.sort_by_key(|r| (!r.is_head, r.kind));
    }
    Ok(refs)
}

/// A remote branch and the local branch of the same name are one branch to the eye.
fn branch_key(kind: RefKind, name: &str) -> &str {
    match kind {
        RefKind::Remote => name.split_once('/').map_or(name, |(_, branch)| branch),
        _ => name,
    }
}

type FoundRef<'a> = (RefKind, &'a str, &'a str, &'a str);

/// The branch history is shared on: what origin calls its default, or a usual name.
pub(crate) fn pick_default_branch(
    origin_head: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Option<&str> {
    origin_head
        .filter(|name| exists(name))
        .or_else(|| USUAL_DEFAULT_BRANCHES.into_iter().find(|n| exists(n)))
}

fn default_branch(found: &[FoundRef], origin_head: Option<&str>) -> Option<String> {
    let exists = |wanted: &str| {
        found
            .iter()
            .any(|r| r.0 != RefKind::Tag && branch_key(r.0, r.1) == wanted)
    };
    pick_default_branch(origin_head, exists).map(str::to_owned)
}

/// The ref to compare with the default branch: the local one, or failing that a remote's.
fn shared_ref<'a, 'b>(found: &'a [FoundRef<'b>], default: &str) -> Option<&'a FoundRef<'b>> {
    found
        .iter()
        .find(|r| r.0 == RefKind::Branch && r.1 == default)
        .or_else(|| {
            found
                .iter()
                .find(|r| r.0 == RefKind::Remote && branch_key(r.0, r.1) == default)
        })
}

/// Gives every branch its colour. HEAD's is Blue, the default branch takes the first of
/// the palette, and the rest follow in name order, so colours don't depend on commits.
fn branch_colors(
    found: &[FoundRef],
    default_branch: Option<&str>,
    head_branch: Option<&str>,
) -> HashMap<String, LaneColor> {
    let mut names: Vec<&str> = found
        .iter()
        .filter(|r| matches!(r.0, RefKind::Branch | RefKind::Remote))
        .map(|r| branch_key(r.0, r.1))
        .filter(|name| Some(*name) != default_branch && Some(*name) != head_branch)
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut colors: HashMap<String, LaneColor> = names
        .into_iter()
        .zip(1..)
        .map(|(name, index)| (name.to_owned(), LaneColor::Branch(index)))
        .collect();
    if let Some(default) = default_branch {
        colors.insert(default.to_owned(), LaneColor::Branch(0));
    }
    if let Some(head) = head_branch {
        colors.insert(head.to_owned(), LaneColor::Head);
    }
    colors
}

/// How each local branch that `read_refs` left without a count stands against the default
/// branch, in one git call. This is the slow part of reading refs, so it is a call of its
/// own, to run once the refs are on screen. Old versions of git don't know this format,
/// which only costs the sidebar its counts.
pub fn read_counts(repo: &Path, refs: &Refs) -> HashMap<String, (u32, u32)> {
    let Some((base, _)) = &refs.default_base else {
        return HashMap::new();
    };
    let format = format!("--format=%(refname:short)%00%(ahead-behind:{base})");
    let mut args = vec!["for-each-ref".to_owned(), format];
    let first_pattern = args.len();
    args.extend(
        refs.branches
            .iter()
            .filter(|branch| branch.ahead_behind.is_none())
            .map(|branch| format!("refs/heads/{}", branch.name)),
    );
    if args.len() == first_pattern {
        return HashMap::new();
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let listing = ops::run(repo, &args).unwrap_or_default();
    listing
        .lines()
        .filter_map(|line| {
            let (name, counts) = line.split_once('\0')?;
            let (ahead, behind) = counts.split_once(' ')?;
            Some((name.to_owned(), (ahead.parse().ok()?, behind.parse().ok()?)))
        })
        .collect()
}

/// How each local branch stands against the remote branch it tracks, in one git call.
/// Branches that track nothing, or whose remote branch is gone, are left out.
fn read_upstream(repo: &Path) -> HashMap<String, (String, Option<(u32, u32)>)> {
    let format = "--format=%(refname:short)%00%(upstream)%00%(upstream:track)%00%(upstream:short)";
    let listing = ops::run(repo, &["for-each-ref", format, "refs/heads"]).unwrap_or_default();
    listing
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let (name, upstream, track, short) =
                (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            (!upstream.is_empty()).then_some(())?;
            Some((name.to_owned(), (short.to_owned(), parse_track(track))))
        })
        .collect()
}

/// Reads `[ahead 2, behind 1]`, which is empty when level and `[gone]` when the remote
/// branch is.
fn parse_track(track: &str) -> Option<(u32, u32)> {
    let track = track.trim().trim_start_matches('[').trim_end_matches(']');
    if track == "gone" {
        return None;
    }
    let (mut ahead, mut behind) = (0, 0);
    for part in track.split(',') {
        match part.trim().split_once(' ') {
            Some(("ahead", n)) => ahead = n.parse().ok()?,
            Some(("behind", n)) => behind = n.parse().ok()?,
            _ => {}
        }
    }
    Some((ahead, behind))
}

/// Where HEAD's first-parent line joins `base`. This can't be `git merge-base`: after
/// merging `base` back into a branch, that commit isn't on HEAD's line at all.
fn fork_point(repo: &Path, base: &str) -> Option<Arc<str>> {
    let exclude = format!("^{base}");
    let args = ["rev-list", "--first-parent", "--parents", "HEAD", &exclude];
    let listing = ops::run(repo, &args).ok()?;
    let Some(oldest) = listing.lines().last() else {
        // Nothing of HEAD's is outside base, so all of its line is shared.
        return ops::run(repo, &["rev-parse", "HEAD"])
            .ok()
            .map(|head| head.trim().into());
    };
    oldest.split_whitespace().nth(1).map(Arc::from)
}

fn intern(tips: &mut Vec<Arc<str>>, oid: &str) -> Arc<str> {
    let oid: Arc<str> = oid.into();
    tips.push(oid.clone());
    oid
}

fn classify_ref(full_name: &str) -> Option<(RefKind, &str)> {
    if let Some(name) = full_name.strip_prefix("refs/heads/") {
        Some((RefKind::Branch, name))
    } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
        Some((RefKind::Tag, name))
    } else {
        // refs/remotes/origin/HEAD only repeats what origin's default branch already shows.
        let name = full_name.strip_prefix("refs/remotes/")?;
        (!name.ends_with("/HEAD")).then_some((RefKind::Remote, name))
    }
}

/// Streams up to `limit` commits reachable from `tips`, children before parents, until
/// `sink` returns false.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Commit;

    #[test]
    fn track_text_reads_as_counts() {
        assert_eq!(parse_track(""), Some((0, 0)));
        assert_eq!(parse_track("[ahead 2]"), Some((2, 0)));
        assert_eq!(parse_track("[behind 1]"), Some((0, 1)));
        assert_eq!(parse_track("[ahead 2, behind 1]"), Some((2, 1)));
        assert_eq!(parse_track("[gone]"), None);
    }

    fn named(kind: RefKind, name: &str) -> Ref {
        Ref {
            kind,
            name: name.to_owned(),
            is_head: false,
            color: Default::default(),
        }
    }

    #[test]
    fn a_branch_and_its_remote_at_one_commit_are_one_synced_chip() {
        let refs = vec![
            named(RefKind::Branch, "main"),
            named(RefKind::Remote, "origin/main"),
            named(RefKind::Remote, "origin/other"),
            named(RefKind::Tag, "v1"),
        ];
        let found = chips(&refs);
        let names: Vec<_> = found.iter().map(|c| c.label.name.as_str()).collect();
        assert_eq!(names, ["main", "origin/other", "v1"]);
        assert!(found[0].synced);
        assert!(!found[1].synced && !found[2].synced);
    }

    fn commit_with(oid: &str, parents: &[&str]) -> Commit {
        Commit {
            oid: oid.into(),
            parents: parents.iter().map(|&parent| parent.into()).collect(),
            author: "".into(),
            time: 0,
            subject: "".into(),
        }
    }

    #[test]
    fn head_can_only_be_reverted() {
        let refs = Refs {
            head: Some("a".into()),
            ..Refs::default()
        };
        let actions = refs.actions_for(&commit_with("a", &["b"]));
        assert!(!actions.can_check_out && !actions.can_cherry_pick);
    }

    #[test]
    fn a_merge_cannot_be_cherry_picked() {
        let actions = Refs::default().actions_for(&commit_with("a", &["b", "c"]));
        assert!(actions.is_merge && actions.can_check_out && !actions.can_cherry_pick);
    }

    #[test]
    fn check_out_names_the_branch_at_the_commit() {
        let mut refs = Refs::default();
        let branch = named(RefKind::Branch, "feat/x");
        refs.by_commit.insert("a".into(), vec![branch]);
        let at_branch = refs.actions_for(&commit_with("a", &[]));
        assert_eq!(at_branch.check_out_label(), "Check out feat/x");
        let bare = refs.actions_for(&commit_with("b", &[]));
        assert_eq!(bare.check_out_label(), "Check out");
    }

    /// `main` with one commit, `level` at the same commit and `side` one commit ahead.
    fn repo_with_side() -> crate::testing::Repo {
        let repo = crate::testing::Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        repo.git(&["branch", "level"]);
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.write("b", "2\n");
        repo.commit("add b");
        repo.git(&["checkout", "-q", "main"]);
        repo
    }

    fn counts_of(refs: &Refs, name: &str) -> Option<(u32, u32)> {
        refs.branches.iter().find(|b| b.name == name)?.ahead_behind
    }

    #[test]
    fn a_branch_level_with_the_default_is_counted_without_asking_git() {
        let repo = repo_with_side();
        let refs = read_refs(repo.path()).unwrap();
        assert_eq!(counts_of(&refs, "main"), Some((0, 0)));
        assert_eq!(counts_of(&refs, "level"), Some((0, 0)));
        // The branch that differs waits for `read_counts`.
        assert_eq!(counts_of(&refs, "side"), None);
        assert!(refs.needs_counts());
    }

    #[test]
    fn counts_read_later_fill_in_the_branches_left_out() {
        let repo = repo_with_side();
        let mut refs = read_refs(repo.path()).unwrap();
        let counts = read_counts(repo.path(), &refs);
        assert_eq!(counts.get("side"), Some(&(1, 0)));
        assert!(!counts.contains_key("level"), "level was already known");
        refs.set_counts(&counts);
        assert_eq!(counts_of(&refs, "side"), Some((1, 0)));
        assert!(!refs.needs_counts());
    }

    #[test]
    fn nothing_is_asked_when_every_branch_is_level() {
        let repo = crate::testing::Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");
        let refs = read_refs(repo.path()).unwrap();
        assert!(!refs.needs_counts());
        assert!(read_counts(repo.path(), &refs).is_empty());
    }

    #[test]
    fn a_reload_keeps_the_counts_of_branches_that_did_not_move() {
        let repo = repo_with_side();
        let mut old = read_refs(repo.path()).unwrap();
        old.set_counts(&read_counts(repo.path(), &old));
        let mut fresh = read_refs(repo.path()).unwrap();
        fresh.keep_counts_from(&old);
        assert_eq!(counts_of(&fresh, "side"), Some((1, 0)));
        assert!(!fresh.needs_counts());

        // Once the default branch moves, every old count is out of date.
        repo.write("c", "3\n");
        repo.commit("main moves");
        let mut moved = read_refs(repo.path()).unwrap();
        moved.keep_counts_from(&old);
        assert_eq!(counts_of(&moved, "side"), None);
    }
}
