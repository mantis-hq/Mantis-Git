//! What the context menus offer. Right-clicking a branch, a tag or commits, or dropping one
//! branch on another, asks here for the lines of a menu. Each line carries the command it
//! runs, so the interface only draws lines and hands the chosen command back to be run.

pub mod command;
pub mod form;

use crate::history::Commit;
use crate::history::refs::{RefKind, Refs};
use crate::repo::branch::{self, Doomed, Dragged, Relation};
use crate::repo::conflict;
use crate::repo::forge::{self, Forge, Hosting, Site};
use crate::repo::remote;
use crate::repo::stash;
use crate::repo::submodule;
use crate::repo::worktree;
use crate::text::{count, short_oid};
use crate::working::{self, Side};
use command::Command;
use form::Form;
use std::path::Path;

/// A small picture that starts a line, saying what the line is about. The interface draws
/// it, since this crate knows nothing of icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    GitHub,
    GitLab,
    Gitea,
    /// A question: what the line does depends on something that isn't known.
    Unknown,
}

impl From<Forge> for Glyph {
    fn from(forge: Forge) -> Self {
        match forge {
            Forge::GitHub => Self::GitHub,
            Forge::GitLab => Self::GitLab,
            Forge::Gitea => Self::Gitea,
        }
    }
}

/// One line of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// Does nothing without a command: it only explains why there isn't one.
    Item {
        label: String,
        command: Option<Command>,
        /// Drawn in red, for what removes something.
        danger: bool,
        /// Drawn before the label.
        glyph: Option<Glyph>,
    },
    /// A quiet line that says what the lines below it apply to.
    Heading(String),
    Separator,
}

impl Line {
    fn item(label: impl Into<String>, command: Command) -> Self {
        Self::Item {
            label: label.into(),
            command: Some(command),
            danger: false,
            glyph: None,
        }
    }

    fn unavailable(label: impl Into<String>) -> Self {
        Self::Item {
            label: label.into(),
            command: None,
            danger: false,
            glyph: None,
        }
    }

    fn danger(mut self) -> Self {
        if let Self::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    fn with_glyph(mut self, picture: impl Into<Glyph>) -> Self {
        if let Self::Item { glyph, .. } = &mut self {
            *glyph = Some(picture.into());
        }
        self
    }

    pub fn glyph(&self) -> Option<Glyph> {
        match self {
            Self::Item { glyph, .. } => *glyph,
            _ => None,
        }
    }

    pub fn command(&self) -> Option<&Command> {
        match self {
            Self::Item { command, .. } => command.as_ref(),
            _ => None,
        }
    }
}

fn show_in_folder(path: &str) -> Line {
    Line::item(
        "Show in folder",
        Command::RevealPath {
            path: path.trim_end_matches('/').to_owned(),
        },
    )
}

fn copy(text: &str, what: &'static str) -> Command {
    Command::Copy {
        text: text.to_owned(),
        what,
    }
}

/// The line that checks out a branch, remote branch or tag. The checked-out branch gets
/// none, since the menu's own heading already shows it.
fn check_out_ref_line(current: Option<&str>, kind: RefKind, name: &str) -> Option<Line> {
    if kind == RefKind::Branch && current == Some(name) {
        return None;
    }
    let label = match kind {
        RefKind::Remote => format!("Check out {name} as a local branch"),
        RefKind::Tag => format!("Check out {name} without a branch"),
        _ => format!("Check out {name}"),
    };
    Some(Line::item(
        label,
        Command::CheckOutRef {
            kind,
            name: name.to_owned(),
        },
    ))
}

/// Pull and push for a local branch, checked out or not, against the remote branch it
/// follows. A branch that follows nothing gets neither: "Set upstream" is how it starts.
fn sync_lines(refs: &Refs, name: &str) -> Vec<Line> {
    let Some(branch) = refs.branches.iter().find(|b| b.name == name) else {
        return Vec::new();
    };
    let Some(remote) = branch.upstream_name.as_deref() else {
        return Vec::new();
    };
    // The counts come from the last fetch, so a pull is always offered: it asks the remote.
    let pull = Line::item(
        format!("Pull {remote} into {name}"),
        Command::PullBranch {
            branch: name.to_owned(),
        },
    );
    let push = match branch.upstream {
        None => Line::unavailable(format!("{remote} is gone, so there is nothing to push to")),
        // Nothing to push: say nothing rather than list a line that can't be used.
        Some((0, _)) => return vec![pull],
        Some((ahead, _)) => Line::item(
            format!("Push {} to {remote}", crate::text::count(ahead, "commit")),
            Command::PushBranch {
                branch: name.to_owned(),
            },
        ),
    };
    vec![pull, push]
}

/// The menu for a branch, remote branch or tag.
pub fn ref_menu(refs: &Refs, target: &Dragged) -> Vec<Line> {
    let current = refs.head_branch.as_deref();
    let Dragged { kind, name } = target.clone();
    let is_current = kind == RefKind::Branch && current == Some(name.as_str());
    let mut lines = vec![Line::Heading(name.clone())];

    lines.extend(check_out_ref_line(current, kind, &name));

    if let Some(current) = current.filter(|_| !is_current) {
        lines.push(Line::item(
            format!("Merge {name} into {current}"),
            Command::MergeIntoCurrent {
                source: name.clone(),
            },
        ));
        if kind != RefKind::Tag {
            lines.push(Line::item(
                format!("Rebase {current} onto {name}"),
                Command::Rebase {
                    branch: current.to_owned(),
                    onto: name.clone(),
                },
            ));
        }
    }

    if kind == RefKind::Branch {
        lines.extend(sync_lines(refs, &name));
    }

    lines.push(Line::item(
        format!("New branch from {name}"),
        Command::Form(Form::NewBranch {
            start: Some(name.clone()),
            label: name.clone(),
        }),
    ));
    if kind == RefKind::Branch {
        lines.push(Line::item(
            format!("Set upstream of {name}"),
            Command::Form(Form::Upstream {
                branch: name.clone(),
                existing: refs.remotes.iter().map(|r| r.name.clone()).collect(),
            }),
        ));
    }

    let tip = match kind {
        RefKind::Branch => refs
            .branches
            .iter()
            .find(|b| b.name == name)
            .map(|b| &b.tip),
        RefKind::Tag => refs.tags.iter().find(|t| t.name == name).map(|t| &t.tip),
        _ => None,
    };
    let deletable = match kind {
        RefKind::Branch => !is_current,
        RefKind::Tag => true,
        _ => false,
    };
    if deletable {
        let label = match kind {
            RefKind::Tag => format!("Delete tag {name}"),
            _ => format!("Delete branch {name}"),
        };
        lines.push(Line::Separator);
        lines.push(
            Line::item(
                label,
                Command::Delete {
                    kind,
                    at_risk: branch::loses_commits(refs, kind, &name),
                    name: name.clone(),
                    tip: tip.map(|tip| tip.to_string()),
                },
            )
            .danger(),
        );
    }
    if kind == RefKind::Remote {
        if let Some((remote, branch_name)) = remote::split_branch(&refs.remote_configs, &name) {
            lines.push(Line::Separator);
            lines.push(
                Line::item(
                    format!("Delete {name} on {}", remote.name),
                    Command::DeleteRemote {
                        remote: remote.name.clone(),
                        name: branch_name.to_owned(),
                        tip: refs
                            .remotes
                            .iter()
                            .find(|r| r.name == name)
                            .map(|r| r.tip.to_string()),
                    },
                )
                .danger(),
            );
        }
    }
    lines
}

/// Whether a ref can be deleted from the menu: a local branch that isn't checked out, or
/// a tag. A remote branch is deleted one at a time, from its own menu.
fn why_not_deletable(refs: &Refs, target: &Dragged) -> Option<String> {
    let name = &target.name;
    match target.kind {
        RefKind::Tag => None,
        RefKind::Branch if refs.head_branch.as_deref() == Some(name.as_str()) => {
            Some(format!("{name} is checked out, so it stays"))
        }
        RefKind::Branch | RefKind::Remote => None,
        _ => Some(format!("{name} can't be deleted")),
    }
}

/// The menu for a working tree in the sidebar.
pub fn worktree_menu(refs: &Refs, path: &Path) -> Vec<Line> {
    let Some(tree) = refs.worktrees.iter().find(|tree| tree.path == path) else {
        return Vec::new();
    };
    let name = tree.label();
    let mut lines = vec![Line::Heading(name.clone())];
    let (switch, new_tab) = if tree.is_current {
        (
            Line::unavailable(format!("{name} is the one on show")),
            Line::unavailable("Already open here"),
        )
    } else if let Some(reason) = tree.unavailable() {
        (
            Line::unavailable(format!("Can't switch to {name}: {}", reason.to_lowercase())),
            Line::unavailable("Can't open it in a new tab"),
        )
    } else {
        let path = tree.path.clone();
        (
            Line::item(
                format!("Switch to {name}"),
                Command::SwitchWorktree { path: path.clone() },
            ),
            Line::item(
                format!("Open {name} in a new tab"),
                Command::OpenWorktreeInTab { path },
            ),
        )
    };
    lines.push(switch);
    lines.push(new_tab);
    lines.push(Line::Separator);
    lines.push(Line::item(
        "Copy path",
        copy(&tree.path.to_string_lossy(), "the path"),
    ));
    lines.push(Line::Separator);
    lines.push(if tree.is_main {
        Line::unavailable(format!("{name} holds the repository, so it stays"))
    } else if tree.is_current {
        Line::unavailable(format!("{name} is on show, so it stays"))
    } else if tree.locked {
        Line::unavailable(format!("{name} is locked, so it stays"))
    } else {
        Line::item(
            format!("Remove worktree {name}"),
            Command::RemoveWorktree {
                path: tree.path.clone(),
                warning: worktree::remove_warning(tree),
            },
        )
        .danger()
    });
    lines
}

/// The menu for the picked stashes in the sidebar, by their numbers. A stash is applied and
/// kept, popped (applied and deleted), or deleted after asking.
pub fn stash_menu(refs: &Refs, indexes: &[usize]) -> Vec<Line> {
    let indexes: Vec<usize> = indexes
        .iter()
        .copied()
        .filter(|&index| index < refs.stashes.len())
        .collect();
    let Some(&first) = indexes.first() else {
        return Vec::new();
    };
    let heading = match indexes.as_slice() {
        [_] => stash::title(&refs.stashes[first]).to_owned(),
        many => format!("{} selected", many.len()),
    };
    let subjects: Vec<String> = indexes
        .iter()
        .map(|&index| stash::title(&refs.stashes[index]).to_owned())
        .collect();
    vec![
        Line::Heading(heading),
        Line::item(
            stash::apply_label(indexes.len()),
            Command::ApplyStashes {
                indexes: indexes.clone(),
            },
        ),
        Line::item(
            stash::pop_label(indexes.len()),
            Command::PopStashes {
                indexes: indexes.clone(),
            },
        ),
        Line::Separator,
        Line::item(
            stash::drop_label(indexes.len()),
            Command::DeleteStashes {
                warning: stash::drop_warning(&subjects),
                indexes,
            },
        )
        .danger(),
    ]
}

/// Where a file in the inspector comes from: one of the two lists of working changes, or a
/// commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSource<'a> {
    Working(Side),
    Commit {
        oid: &'a str,
        /// Where a renamed file came from.
        old_path: Option<&'a str>,
    },
}

/// The menu for a file in the inspector. A working file can be staged or unstaged, and a
/// commit's file can be applied to the working changes. `None` offers neither.
pub fn file_menu(path: &str, index: usize, source: Option<FileSource>) -> Vec<Line> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut lines = vec![
        Line::Heading(path.to_owned()),
        Line::item(
            format!("Show changes in {name}"),
            Command::ShowFile { index },
        ),
    ];
    let action = match source {
        Some(FileSource::Working(side)) => Some(Line::item(
            side.action(name),
            Command::StageFiles {
                paths: vec![path.to_owned()],
                unstage: side.unstages(),
            },
        )),
        Some(FileSource::Commit { oid, old_path }) => Some(Line::item(
            format!("Apply changes in {name} to working changes"),
            Command::ApplyFile {
                oid: oid.to_owned(),
                path: path.to_owned(),
                old_path: old_path.map(str::to_owned),
            },
        )),
        None => None,
    };
    if let Some(action) = action {
        lines.push(Line::Separator);
        lines.push(action);
    }
    if let Some(FileSource::Working(Side::Unstaged)) = source {
        lines.push(discard_line(vec![path.to_owned()], name));
    }
    lines.push(Line::Separator);
    lines.push(show_in_folder(path));
    lines.push(Line::item("Copy path", copy(path, "the path")));
    lines
}

/// The red line that throws away unstaged changes in `paths`, which `what` names. It asks
/// first.
fn discard_line(paths: Vec<String>, what: &str) -> Line {
    Line::item(working::discard_label(what), discard_command(paths, what)).danger()
}

/// Discards the unstaged changes in `paths`, which `what` names, once confirmed. The
/// "Discard all" button runs this too.
pub fn discard_command(paths: Vec<String>, what: &str) -> Command {
    Command::DiscardChanges {
        paths,
        warning: working::discard_warning(what),
    }
}

/// The menu for a conflicted file in the inspector. The sides are named by branch, since
/// "ours" and "theirs" swap meaning during a rebase.
pub fn conflict_menu(progress: &conflict::Progress, path: &str) -> Vec<Line> {
    let use_side = |side, branch: &str| {
        Line::item(
            format!("Use {branch}"),
            Command::UseSide {
                path: path.to_owned(),
                side,
            },
        )
    };
    vec![
        Line::Heading(path.to_owned()),
        Line::item(
            "Show both sides",
            Command::ShowConflict {
                path: path.to_owned(),
            },
        ),
        Line::Separator,
        use_side(conflict::Side::Mine, &progress.mine),
        use_side(conflict::Side::Other, &progress.other),
        Line::Separator,
        show_in_folder(path),
        Line::item("Copy path", copy(path, "the path")),
    ]
}

/// The menu for a folder in the inspector's tree, which acts on every file under it.
pub fn folder_menu(path: &str, files: Vec<String>, side: Option<Side>) -> Vec<Line> {
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let mut lines = vec![Line::Heading(format!("{path}/"))];
    if let Some(side) = side {
        let what = format!("{} in {name}/", count(files.len(), "file"));
        lines.push(Line::item(
            side.action(&what),
            Command::StageFiles {
                paths: files.clone(),
                unstage: side.unstages(),
            },
        ));
        if side == Side::Unstaged {
            lines.push(discard_line(files, &what));
        }
        lines.push(Line::Separator);
    }
    lines.push(show_in_folder(path));
    lines.push(Line::item("Copy path", copy(path, "the path")));
    lines
}

/// The menu for a submodule in the sidebar, found by its path.
pub fn submodule_menu(refs: &Refs, path: &str) -> Vec<Line> {
    let Some(module) = refs.submodules.iter().find(|module| module.path == path) else {
        return Vec::new();
    };
    let name = module.label();
    let mut lines = vec![Line::Heading(name.to_owned())];
    lines.push(if module.is_downloaded() {
        Line::item(
            format!("Open {name} in a new tab"),
            Command::OpenSubmoduleInTab {
                path: module.folder.clone(),
            },
        )
    } else {
        Line::unavailable(format!("Can't open {name}: it isn't downloaded yet"))
    });
    lines.push(match module.update_unavailable() {
        Some(reason) => {
            Line::unavailable(format!("Can't update {name}: {}", reason.to_lowercase()))
        }
        None => Line::item(
            module.update_label(),
            Command::UpdateSubmodule {
                path: module.path.clone(),
                warning: module.update_warning(),
            },
        ),
    });
    if let Some(label) = submodule::update_all_label(&refs.submodules) {
        lines.push(Line::item(label, Command::UpdateSubmodules));
    }
    lines.push(Line::Separator);
    lines.push(Line::item(
        format!("Edit {name}"),
        Command::Form(Form::Submodule {
            original: Some(module.path.clone()),
        }),
    ));
    lines.push(Line::item(
        "Copy path",
        copy(&module.folder.to_string_lossy(), "the path"),
    ));
    if let Some(url) = &module.url {
        lines.push(Line::item("Copy URL", copy(url, "the URL")));
    }
    lines
}

/// The menu for a remote in the sidebar.
pub fn remote_menu(refs: &Refs, name: &str) -> Vec<Line> {
    let Some(remote) = refs
        .remote_configs
        .iter()
        .find(|remote| remote.name == name)
    else {
        return Vec::new();
    };
    let mut lines = vec![
        Line::Heading(name.to_owned()),
        Line::item(
            format!("Edit {name}"),
            Command::Form(Form::Remote {
                original: Some(name.to_owned()),
            }),
        ),
        Line::item("Copy address", copy(&remote.url, "the address")),
        Line::Separator,
    ];
    lines.push(
        Line::item(
            format!("Remove remote {name}"),
            Command::RemoveRemote {
                name: name.to_owned(),
                warning: remote::remove_warning(remote),
            },
        )
        .danger(),
    );
    lines
}

/// The menu for several picked branches and tags. The one thing it can do to many at
/// once is delete them.
pub fn refs_menu(refs: &Refs, targets: &[Dragged]) -> Vec<Line> {
    let mut lines = vec![Line::Heading(format!("{} selected", targets.len()))];

    let (deletable, kept): (Vec<&Dragged>, Vec<&Dragged>) = targets
        .iter()
        .partition(|target| why_not_deletable(refs, target).is_none());
    for target in &kept {
        if let Some(reason) = why_not_deletable(refs, target) {
            lines.push(Line::unavailable(reason));
        }
    }
    if !deletable.is_empty() {
        let doomed: Vec<Doomed> = deletable
            .iter()
            .map(|target| Doomed {
                kind: target.kind,
                name: target.name.clone(),
                tip: crate::history::picks::tip_of(refs, target).map(|tip| tip.to_string()),
                at_risk: branch::loses_commits(refs, target.kind, &target.name),
                on_remote: (target.kind == RefKind::Remote)
                    .then(|| remote::split_branch(&refs.remote_configs, &target.name))
                    .flatten()
                    .map(|(remote, name)| (remote.name.clone(), name.to_owned())),
            })
            .collect();
        let tags = doomed.iter().filter(|d| d.kind == RefKind::Tag).count();
        let label = match (tags, doomed.len()) {
            (0, 1) => "Delete 1 branch".to_owned(),
            (0, n) => format!("Delete {n} branches"),
            (t, n) if t == n && n == 1 => "Delete 1 tag".to_owned(),
            (t, n) if t == n => format!("Delete {n} tags"),
            (_, n) => format!("Delete {n} branches and tags"),
        };
        lines.push(Line::item(label, Command::DeleteMany { refs: doomed }).danger());
    }
    lines
}

/// The menu for the selected commits, newest first.
pub fn commit_menu(refs: &Refs, commits: &[Commit]) -> Vec<Line> {
    match commits {
        [] => Vec::new(),
        [commit] => one_commit_menu(refs, commit),
        many => many_commits_menu(refs, many),
    }
}

fn one_commit_menu(refs: &Refs, commit: &Commit) -> Vec<Line> {
    let actions = refs.actions_for(commit);
    let oid = commit.oid.to_string();
    let head = refs.head_name();
    let mut lines = vec![Line::Heading(format!(
        "{}  {}",
        short_oid(&oid),
        commit.subject
    ))];

    // Each ref at the commit, as its chip shows it, then the commit itself.
    let current = refs.head_branch.as_deref();
    for chip in refs.chips_at(&oid) {
        if chip.label.kind != RefKind::DetachedHead {
            lines.extend(check_out_ref_line(
                current,
                chip.label.kind,
                &chip.label.name,
            ));
        }
    }
    lines.push(if !actions.can_check_out && current.is_none() {
        Line::unavailable("This commit is already checked out")
    } else {
        Line::item(
            format!("Check out {} without a branch", short_oid(&oid)),
            Command::CheckOutCommit {
                branch: None,
                oid: oid.clone(),
            },
        )
    });
    if let Some(current) = refs
        .head_branch
        .as_deref()
        .filter(|_| actions.can_check_out)
    {
        lines.push(Line::item(
            format!("Merge into {current}"),
            Command::MergeIntoCurrent {
                source: oid.clone(),
            },
        ));
    }
    if actions.can_cherry_pick {
        lines.push(Line::item(
            format!("Cherry-pick onto {head}"),
            Command::CherryPick {
                oids: vec![oid.clone()],
            },
        ));
    }
    lines.push(Line::item(
        "Revert",
        Command::Revert {
            oids: vec![oid.clone()],
            is_merge: actions.is_merge,
        },
    ));
    lines.push(Line::item(
        format!("New branch from {}", short_oid(&oid)),
        Command::Form(Form::NewBranch {
            start: Some(oid.clone()),
            label: short_oid(&oid).to_owned(),
        }),
    ));

    if actions.can_check_out {
        lines.push(Line::Separator);
        lines.push(Line::item(
            format!("Reset {head} to here, keeping changes"),
            Command::Reset {
                oid: oid.clone(),
                hard: false,
            },
        ));
        lines.push(
            Line::item(
                format!("Reset {head} to here, discarding changes"),
                Command::Reset {
                    oid: oid.clone(),
                    hard: true,
                },
            )
            .danger(),
        );
    }

    lines.push(Line::Separator);
    lines.push(Line::item("Copy hash", copy(&oid, "the hash")));
    lines.push(Line::item(
        "Copy subject",
        copy(&commit.subject, "the subject"),
    ));
    lines
}

fn many_commits_menu(refs: &Refs, commits: &[Commit]) -> Vec<Line> {
    let total = count(commits.len(), "commit");
    let newest_first: Vec<String> = commits.iter().map(|c| c.oid.to_string()).collect();
    let oldest_first: Vec<String> = newest_first.iter().rev().cloned().collect();
    let any_merge = commits.iter().any(Commit::is_merge);
    let mut lines = vec![Line::Heading(format!("{total} selected"))];

    lines.push(if any_merge {
        Line::unavailable("A merge can't be cherry-picked")
    } else {
        Line::item(
            format!(
                "Cherry-pick {total} onto {}, oldest first",
                refs.head_name()
            ),
            Command::CherryPick { oids: oldest_first },
        )
    });
    lines.push(if any_merge {
        Line::unavailable("Revert a merge on its own")
    } else {
        Line::item(
            format!("Revert {total}, newest first"),
            Command::Revert {
                oids: newest_first.clone(),
                is_merge: false,
            },
        )
    });
    lines.push(Line::Separator);
    lines.push(Line::item(
        "Copy hashes",
        copy(&newest_first.join("\n"), "the hashes"),
    ));
    lines
}

/// The menu a drop opens: what dragging `source` onto the branch `target` can mean.
pub fn drop_menu(source: &Dragged, target: &str, relation: Result<Relation, String>) -> Vec<Line> {
    let relation = match relation {
        Ok(relation) => relation,
        Err(message) => return vec![Line::unavailable(message)],
    };
    let mut lines = vec![Line::Heading(format!("{} onto {target}", source.name))];
    for choice in branch::drop_choices(relation, source, target) {
        lines.push(match choice.action {
            Some(action) => Line::item(
                choice.label,
                Command::Drop {
                    action,
                    source: source.name.clone(),
                    target: target.to_owned(),
                },
            ),
            None => Line::unavailable(choice.label),
        });
    }
    lines
}

/// The menu a drop opens when both ends are remote branches: dragging `source` onto
/// `target` proposes bringing the one into the other, which is a pull or merge request on
/// the forge. Git can't make one, so the line opens the page that does. `relation` is how
/// the two histories stand.
pub fn request_drop_menu(
    refs: &Refs,
    source: &Dragged,
    target: &Dragged,
    relation: Result<Relation, String>,
) -> Vec<Line> {
    let remotes = &refs.remote_configs;
    let (Some((from, head)), Some((onto, base))) = (
        remote::split_branch(remotes, &source.name),
        remote::split_branch(remotes, &target.name),
    ) else {
        return vec![
            Line::Heading(format!("{} onto {}", source.name, target.name)),
            Line::unavailable("These branches aren't on a known remote."),
        ];
    };
    let heading = Line::Heading(format!("{head} onto {base}"));
    let said = |message: String| vec![heading.clone(), Line::unavailable(message)];
    if from.name != onto.name {
        return said("Branches on different remotes can't be proposed yet.".to_owned());
    }
    match relation {
        Err(message) => return said(message),
        Ok(Relation::Same | Relation::Contained) => {
            return said(format!("Nothing to propose: {base} has all of {head}."));
        }
        Ok(Relation::FastForward | Relation::Diverged) => {}
    }

    let line = match forge::hosting(onto, refs.forge) {
        Some(Hosting::Known(site)) => {
            let noun = site.forge.noun();
            let url = site.new_request_url(base, head);
            return vec![
                heading,
                Line::item(
                    format!("Open a {noun} for {head} into {base}"),
                    Command::OpenUrl {
                        url: url.clone(),
                        what: format!("the page that starts a {noun} on {}", site.forge.name()),
                    },
                )
                .with_glyph(site.forge),
                Line::item("Copy link", copy(&url, "the link")),
            ];
        }
        Some(Hosting::Unknown(_)) => Line::item(
            format!("Open a request for {head} into {base}"),
            Command::AskForge {
                message: forge::choice::unknown_question(&onto.name),
            },
        )
        .with_glyph(Glyph::Unknown),
        None => Line::unavailable(format!("{} isn't on the web.", onto.name)),
    };
    vec![heading, line]
}

/// The menu a drop opens when a local branch lands on a remote branch: it can be pushed
/// there, but only when that is a fast-forward of the remote branch. `relation` is how the
/// two histories stand, the local branch being the source.
pub fn push_drop_menu(
    refs: &Refs,
    source: &Dragged,
    target: &Dragged,
    relation: Result<Relation, String>,
) -> Vec<Line> {
    let heading = Line::Heading(format!("{} onto {}", source.name, target.name));
    let said = |message: String| vec![heading.clone(), Line::unavailable(message)];
    let Some((remote, name)) = remote::split_branch(&refs.remote_configs, &target.name) else {
        return said("That branch isn't on a known remote.".to_owned());
    };
    let (branch, to) = (&source.name, &target.name);
    match relation {
        Err(message) => said(message),
        Ok(Relation::Same | Relation::Contained) => {
            said(format!("Nothing to push: {to} has all of {branch}."))
        }
        Ok(Relation::Diverged) => said(format!(
            "{to} has commits {branch} lacks, so pushing would need a force. Pull or rebase first."
        )),
        Ok(Relation::FastForward) => vec![
            heading,
            Line::item(
                format!("Push {branch} to {to}"),
                Command::PushTo {
                    branch: branch.clone(),
                    remote: remote.name.clone(),
                    name: name.to_owned(),
                },
            ),
        ],
    }
}

/// The menu of one pull or merge request in the sidebar.
pub fn request_menu(site: &Site, number: u64) -> Vec<Line> {
    let label = site.label(number);
    let url = site.request_url(number);
    vec![
        Line::Heading(format!("{} {label}", site.forge.noun())),
        Line::item(
            format!("Open {label} on {}", site.forge.name()),
            Command::OpenUrl {
                url: url.clone(),
                what: format!("{} {label}", site.forge.noun()),
            },
        )
        .with_glyph(site.forge),
        Line::item("Copy link", copy(&url, "the link")),
    ]
}

#[cfg(test)]
mod tests {
    use super::command::Weight;
    use super::*;
    use std::path::PathBuf;

    fn shown_lines(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|line| match line {
                Line::Item { label, .. } | Line::Heading(label) => Some(label.clone()),
                Line::Separator => None,
            })
            .collect()
    }

    #[test]
    fn a_working_file_can_be_shown_staged_or_unstaged_and_copied() {
        let lines = file_menu("src/a.rs", 3, Some(FileSource::Working(Side::Unstaged)));
        assert_eq!(
            shown_lines(&lines),
            [
                "src/a.rs",
                "Show changes in a.rs",
                "Stage a.rs",
                "Discard changes in a.rs",
                "Show in folder",
                "Copy path"
            ]
        );
        assert_eq!(
            lines[3].command(),
            Some(&Command::StageFiles {
                paths: vec!["src/a.rs".to_owned()],
                unstage: false
            })
        );
        let lines = file_menu("src/a.rs", 3, Some(FileSource::Working(Side::Staged)));
        assert!(matches!(
            lines[3].command(),
            Some(Command::StageFiles { unstage: true, .. })
        ));
        assert_eq!(shown_lines(&lines)[2], "Unstage a.rs");
    }

    #[test]
    fn a_commits_file_can_be_applied_to_the_working_changes() {
        let source = FileSource::Commit {
            oid: "1a2b3c4d",
            old_path: Some("src/old.rs"),
        };
        let lines = file_menu("src/a.rs", 0, Some(source));
        assert_eq!(
            shown_lines(&lines),
            [
                "src/a.rs",
                "Show changes in a.rs",
                "Apply changes in a.rs to working changes",
                "Show in folder",
                "Copy path"
            ]
        );
        assert_eq!(
            lines[3].command(),
            Some(&Command::ApplyFile {
                oid: "1a2b3c4d".to_owned(),
                path: "src/a.rs".to_owned(),
                old_path: Some("src/old.rs".to_owned()),
            })
        );
    }

    #[test]
    fn discarding_is_red_and_asks_first() {
        let lines = file_menu("src/a.rs", 0, Some(FileSource::Working(Side::Unstaged)));
        let Some(Line::Item {
            danger: true,
            command: Some(command),
            ..
        }) = lines
            .iter()
            .find(|line| matches!(line, Line::Item { label, .. } if label.starts_with("Discard")))
        else {
            panic!("no red discard line");
        };
        let confirmation = command.confirmation().unwrap();
        assert_eq!(confirmation.action, "Discard");
        assert_eq!(confirmation.weight, Weight::DoublePress);
        let staged = file_menu("src/a.rs", 0, Some(FileSource::Working(Side::Staged)));
        assert!(
            !shown_lines(&staged)
                .iter()
                .any(|l| l.starts_with("Discard"))
        );
    }

    #[test]
    fn a_file_with_no_source_can_only_be_shown_and_copied() {
        let lines = file_menu("a.rs", 0, None);
        assert_eq!(
            shown_lines(&lines),
            [
                "a.rs",
                "Show changes in a.rs",
                "Show in folder",
                "Copy path"
            ]
        );
        assert_eq!(
            lines[3].command().and_then(Command::reveal_path),
            Some("a.rs")
        );
    }

    #[test]
    fn a_folder_stages_everything_under_it() {
        let files = vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()];
        let lines = folder_menu("crates/src", files, Some(Side::Unstaged));
        assert_eq!(
            shown_lines(&lines),
            [
                "crates/src/",
                "Stage 2 files in src/",
                "Discard changes in 2 files in src/",
                "Show in folder",
                "Copy path"
            ]
        );
        let lines = folder_menu("src", Vec::new(), Some(Side::Staged));
        assert_eq!(
            shown_lines(&lines),
            [
                "src/",
                "Unstage 0 files in src/",
                "Show in folder",
                "Copy path"
            ],
            "staged changes are not discarded from here"
        );
        let lines = folder_menu("src", Vec::new(), None);
        assert_eq!(shown_lines(&lines), ["src/", "Show in folder", "Copy path"]);
        assert_eq!(
            lines[1].command().and_then(Command::reveal_path),
            Some("src"),
            "a folder is shown without its trailing slash"
        );
    }
    use crate::history::lanes::LaneColor;
    use crate::history::refs::{Branch, NamedRef};

    fn commit(oid: &str, parents: &[&str]) -> Commit {
        Commit {
            oid: oid.into(),
            parents: parents.iter().map(|&p| p.into()).collect(),
            author: "Ada".into(),
            email: "".into(),
            time: 0,
            subject: format!("subject of {oid}").into(),
        }
    }

    fn refs() -> Refs {
        let branch = |name: &str, tip: &str, is_head| Branch {
            name: name.into(),
            tip: tip.into(),
            color: LaneColor::default(),
            is_head,
            ahead_behind: None,
            upstream: None,
            upstream_name: None,
        };
        let mut refs = Refs::default();
        refs.head = Some("aaaaaaaa".into());
        refs.head_branch = Some("main".into());
        refs.branches = vec![
            branch("main", "aaaaaaaa", true),
            branch("feat/x", "bbbbbbbb", false),
        ];
        refs.tags = vec![NamedRef {
            name: "v1.0.0".into(),
            tip: "bbbbbbbb".into(),
            color: LaneColor::default(),
        }];
        refs
    }

    #[test]
    fn a_tracking_branch_offers_pull_and_push() {
        let mut refs = refs();
        let target = Dragged {
            kind: RefKind::Branch,
            name: "feat/x".into(),
        };
        assert!(
            !labels(&ref_menu(&refs, &target))
                .iter()
                .any(|l| l.starts_with("Pull"))
        );

        refs.branches[1].upstream_name = Some("origin/feat/x".into());
        refs.branches[1].upstream = Some((2, 1));
        let menu = ref_menu(&refs, &target);
        let found = labels(&menu);
        assert!(
            found.contains(&"Pull origin/feat/x into feat/x"),
            "{found:?}"
        );
        assert!(
            found.contains(&"Push 2 commits to origin/feat/x"),
            "{found:?}"
        );

        // Level: nothing to push.
        refs.branches[1].upstream = Some((0, 0));
        assert!(
            ref_menu(&refs, &target)
                .iter()
                .all(|line| !matches!(line.command(), Some(Command::PushBranch { .. })))
        );
        refs.branches[0].upstream_name = Some("origin/main".into());
        let main = Dragged {
            kind: RefKind::Branch,
            name: "main".into(),
        };
        // The checked-out branch gets the same lines.
        refs.branches[0].upstream = Some((1, 0));
        let found = ref_menu(&refs, &main);
        let found = labels(&found);
        assert!(found.contains(&"Pull origin/main into main"), "{found:?}");
        assert!(found.contains(&"Push 1 commit to origin/main"), "{found:?}");
    }

    #[test]
    fn a_remote_menu_edits_copies_and_removes_with_a_way_back() {
        let mut refs = refs();
        refs.remote_configs = vec![crate::repo::remote::Remote {
            name: "origin".into(),
            url: "git@host:a.git".into(),
            push_url: None,
        }];
        let lines = remote_menu(&refs, "origin");
        let commands: Vec<_> = lines.iter().filter_map(Line::command).collect();
        assert_eq!(
            commands[0],
            &Command::Form(Form::Remote {
                original: Some("origin".into())
            })
        );
        assert_eq!(commands[1], &copy("git@host:a.git", "the address"));
        let remove = commands[2];
        assert!(
            remove
                .confirmation()
                .unwrap()
                .message
                .contains("git remote add origin git@host:a.git")
        );
        assert!(remote_menu(&refs, "nowhere").is_empty());
    }

    #[test]
    fn a_stash_menu_applies_copies_and_deletes_what_is_picked() {
        let mut refs = Refs::default();
        refs.stashes = vec!["On main: a".into(), "WIP on main: 1a2b3c4 b".into()];
        let one = stash_menu(&refs, &[1]);
        assert_eq!(one[0], Line::Heading("1a2b3c4 b".into()));
        let labels: Vec<_> = one
            .iter()
            .filter_map(|line| match line {
                Line::Item { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["Apply stash", "Pop stash", "Delete stash"]);
        let both = stash_menu(&refs, &[0, 1, 9]);
        assert_eq!(both[0], Line::Heading("2 selected".into()));
        let Some(Command::DeleteStashes { indexes, .. }) = both.last().unwrap().command().cloned()
        else {
            panic!("the last line deletes");
        };
        assert_eq!(indexes, [0, 1], "numbers that don't exist are dropped");
        assert!(stash_menu(&refs, &[7]).is_empty());
    }

    #[test]
    fn a_worktree_menu_switches_copies_and_removes_only_what_may_go() {
        let mut refs = refs();
        refs.worktrees = crate::repo::worktree::parse(
            "worktree /code/app\nHEAD aaaaaaaa\nbranch refs/heads/main\n\n\
             worktree /code/app-x\nHEAD bbbbbbbb\nbranch refs/heads/feat/x\n\n\
             worktree /code/app-l\nHEAD bbbbbbbb\ndetached\nlocked\n",
        );
        refs.worktrees[0].is_current = true;

        let linked = worktree_menu(&refs, Path::new("/code/app-x"));
        assert_eq!(
            labels(&linked),
            [
                "Switch to app-x",
                "Open app-x in a new tab",
                "Copy path",
                "Remove worktree app-x",
            ]
        );
        let remove = linked.last().and_then(Line::command).unwrap();
        assert!(
            remove
                .confirmation()
                .unwrap()
                .message
                .contains("git worktree add")
        );

        let current = worktree_menu(&refs, Path::new("/code/app"));
        assert!(current.iter().all(|line| {
            line.command()
                .is_none_or(|c| matches!(c, Command::Copy { .. }))
        }));
        assert!(labels(&current).contains(&"app holds the repository, so it stays"));

        let locked = worktree_menu(&refs, Path::new("/code/app-l"));
        assert!(labels(&locked).contains(&"app-l is locked, so it stays"));
        assert!(worktree_menu(&refs, Path::new("/nowhere")).is_empty());
    }

    #[test]
    fn a_submodule_menu_opens_what_is_there_and_asks_before_moving_one() {
        use crate::repo::submodule::{State, Submodule};
        let module = |path: &str, state, head: Option<&str>| Submodule {
            name: path.into(),
            path: path.into(),
            folder: PathBuf::from("/code/app").join(path),
            url: Some(format!("https://example.com/{path}.git")),
            recorded: "a".repeat(40),
            head: head.map(|oid| oid.repeat(40)),
            state,
        };
        let mut refs = refs();
        refs.submodules = vec![
            module("current", State::Current, Some("a")),
            module("moved", State::Moved, Some("b")),
            module("absent", State::NotDownloaded, None),
        ];

        let current = submodule_menu(&refs, "current");
        assert_eq!(
            labels(&current),
            [
                "Open current in a new tab",
                "Update current",
                "Update 2 submodules",
                "Edit current",
                "Copy path",
                "Copy URL",
            ]
        );
        assert!(current[2].command().unwrap().confirmation().is_none());
        assert_eq!(
            current[5].command(),
            Some(&Command::Form(Form::Submodule {
                original: Some("current".into())
            }))
        );

        let moved = submodule_menu(&refs, "moved");
        let update = moved[2].command().unwrap();
        assert_eq!(labels(&moved)[1], "Check out aaaaaaa in moved");
        let warning = update.confirmation().unwrap().message;
        assert!(warning.contains("git -C moved checkout bbbb"), "{warning}");

        let absent = submodule_menu(&refs, "absent");
        assert_eq!(
            labels(&absent)[0],
            "Can't open absent: it isn't downloaded yet"
        );
        assert_eq!(labels(&absent)[1], "Download absent at aaaaaaa");
        assert!(absent[2].command().unwrap().confirmation().is_none());
        assert!(submodule_menu(&refs, "nowhere").is_empty());
    }

    fn labels(lines: &[Line]) -> Vec<&str> {
        lines
            .iter()
            .filter_map(|line| match line {
                Line::Item { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect()
    }

    fn target(kind: RefKind, name: &str) -> Dragged {
        Dragged {
            kind,
            name: name.into(),
        }
    }

    #[test]
    fn several_picks_can_be_deleted_but_not_the_checked_out_one() {
        let picks = [
            target(RefKind::Branch, "main"),
            target(RefKind::Branch, "feat/x"),
            target(RefKind::Tag, "v1.0.0"),
        ];
        let lines = refs_menu(&refs(), &picks);
        assert_eq!(
            labels(&lines),
            [
                "main is checked out, so it stays",
                "Delete 2 branches and tags"
            ]
        );
        let command = lines.last().unwrap().command().unwrap();
        let Command::DeleteMany { refs } = command else {
            panic!("expected a bulk delete");
        };
        assert_eq!(refs.len(), 2);
        assert!(
            command
                .confirmation()
                .unwrap()
                .message
                .contains("feat/x, v1.0.0")
        );
    }

    #[test]
    fn the_checked_out_branch_alone_leaves_nothing_to_delete() {
        let picks = [target(RefKind::Branch, "main")];
        let lines = refs_menu(&refs(), &picks);
        assert!(
            lines
                .iter()
                .all(|l| !matches!(l.command(), Some(Command::DeleteMany { .. })))
        );
    }

    #[test]
    fn another_branch_can_be_merged_rebased_and_deleted() {
        let lines = ref_menu(&refs(), &target(RefKind::Branch, "feat/x"));
        assert_eq!(
            labels(&lines),
            [
                "Check out feat/x",
                "Merge feat/x into main",
                "Rebase main onto feat/x",
                "New branch from feat/x",
                "Set upstream of feat/x",
                "Delete branch feat/x"
            ]
        );
        let delete = lines.last().unwrap();
        let confirmation = delete.command().unwrap().confirmation().unwrap();
        assert!(confirmation.message.contains("git branch feat/x bbbbbbb"));
    }

    #[test]
    fn the_checked_out_branch_can_be_neither_checked_out_nor_deleted() {
        let lines = ref_menu(&refs(), &target(RefKind::Branch, "main"));
        assert_eq!(
            labels(&lines),
            ["New branch from main", "Set upstream of main"]
        );
        assert!(!labels(&lines).iter().any(|l| l.starts_with("Check out")));
    }

    #[test]
    fn a_tag_is_merged_but_never_rebased_onto() {
        let lines = ref_menu(&refs(), &target(RefKind::Tag, "v1.0.0"));
        assert_eq!(
            labels(&lines),
            [
                "Check out v1.0.0 without a branch",
                "Merge v1.0.0 into main",
                "New branch from v1.0.0",
                "Delete tag v1.0.0"
            ]
        );
    }

    #[test]
    fn a_remote_branch_is_checked_out_as_a_local_one_and_not_deleted() {
        let lines = ref_menu(&refs(), &target(RefKind::Remote, "origin/feat/x"));
        assert!(labels(&lines)[0].contains("as a local branch"));
        assert!(!labels(&lines).iter().any(|l| l.starts_with("Delete")));
    }

    #[test]
    fn a_commit_names_the_branches_it_acts_on() {
        let head = commit("aaaaaaaa", &["cccccccc"]);
        let other = commit("bbbbbbbb", &["cccccccc"]);
        let mut refs = refs();
        refs.by_commit
            .insert("aaaaaaaa".into(), vec![named(RefKind::Branch, "main")]);
        let on_head = commit_menu(&refs, &[head]);
        assert_eq!(
            labels(&on_head),
            [
                "Check out aaaaaaa without a branch",
                "Revert",
                "New branch from aaaaaaa",
                "Copy hash",
                "Copy subject"
            ]
        );
        let elsewhere = commit_menu(&refs, &[other]);
        assert_eq!(
            labels(&elsewhere),
            [
                "Check out bbbbbbb without a branch",
                "Merge into main",
                "Cherry-pick onto main",
                "Revert",
                "New branch from bbbbbbb",
                "Reset main to here, keeping changes",
                "Reset main to here, discarding changes",
                "Copy hash",
                "Copy subject"
            ]
        );
    }

    fn named(kind: RefKind, name: &str) -> crate::history::refs::Ref {
        crate::history::refs::Ref {
            kind,
            name: name.into(),
            is_head: false,
            color: LaneColor::default(),
        }
    }

    #[test]
    fn a_commit_with_several_refs_checks_out_any_of_them_or_itself() {
        let mut refs = refs();
        refs.by_commit.insert(
            "bbbbbbbb".into(),
            vec![
                named(RefKind::Branch, "feat/x"),
                named(RefKind::Remote, "origin/feat/x"),
                named(RefKind::Remote, "origin/feat/y"),
                named(RefKind::Tag, "v1.0.0"),
            ],
        );
        let lines = commit_menu(&refs, &[commit("bbbbbbbb", &["cccccccc"])]);
        assert_eq!(
            labels(&lines)[..4],
            [
                "Check out feat/x",
                "Check out origin/feat/y as a local branch",
                "Check out v1.0.0 without a branch",
                "Check out bbbbbbb without a branch",
            ],
            "a remote branch whose local copy is here is the same chip, so one line"
        );
        assert_eq!(
            lines[1].command(),
            Some(&Command::CheckOutRef {
                kind: RefKind::Branch,
                name: "feat/x".into()
            })
        );
        assert_eq!(
            lines[4].command(),
            Some(&Command::CheckOutCommit {
                branch: None,
                oid: "bbbbbbbb".into()
            })
        );
    }

    #[test]
    fn a_detached_head_is_already_checked_out_but_its_branches_are_not() {
        let mut refs = refs();
        refs.head_branch = None;
        refs.by_commit.insert(
            "aaaaaaaa".into(),
            vec![
                named(RefKind::DetachedHead, "HEAD"),
                named(RefKind::Branch, "main"),
            ],
        );
        let lines = commit_menu(&refs, &[commit("aaaaaaaa", &["cccccccc"])]);
        assert_eq!(
            labels(&lines)[..2],
            ["Check out main", "This commit is already checked out"]
        );
    }

    #[test]
    fn only_a_hard_reset_asks_first() {
        let reset = |hard| Command::Reset {
            oid: "bbbbbbbb".into(),
            hard,
        };
        assert!(reset(false).confirmation().is_none());
        assert!(
            reset(true)
                .confirmation()
                .unwrap()
                .message
                .contains("ORIG_HEAD")
        );
    }

    #[test]
    fn several_commits_are_applied_oldest_first_and_reverted_newest_first() {
        let commits = [
            commit("c3", &["c2"]),
            commit("c2", &["c1"]),
            commit("c1", &[]),
        ];
        let lines = commit_menu(&refs(), &commits);
        assert_eq!(
            labels(&lines),
            [
                "Cherry-pick 3 commits onto main, oldest first",
                "Revert 3 commits, newest first",
                "Copy hashes"
            ]
        );
        assert_eq!(
            lines[1].command(),
            Some(&Command::CherryPick {
                oids: vec!["c1".into(), "c2".into(), "c3".into()]
            })
        );
        assert_eq!(
            lines[2].command(),
            Some(&Command::Revert {
                oids: vec!["c3".into(), "c2".into(), "c1".into()],
                is_merge: false
            })
        );
    }

    #[test]
    fn a_merge_among_several_commits_blocks_applying_them() {
        let commits = [commit("m", &["a", "b"]), commit("a", &[])];
        let lines = commit_menu(&refs(), &commits);
        assert_eq!(lines[1].command(), None);
        assert_eq!(lines[2].command(), None);
    }

    #[test]
    fn a_drop_lists_what_fits_and_a_rebase_asks_first() {
        let source = target(RefKind::Branch, "feat/x");
        let lines = drop_menu(&source, "main", Ok(Relation::Diverged));
        assert_eq!(
            labels(&lines),
            ["Merge feat/x into main", "Rebase feat/x onto main"]
        );
        let rebase = lines[2].command().unwrap();
        assert_eq!(rebase.progress(), "Rebasing");
        assert!(
            rebase
                .confirmation()
                .unwrap()
                .message
                .contains("new hashes")
        );
        assert!(lines[1].command().unwrap().confirmation().is_none());

        let nothing = drop_menu(&source, "main", Ok(Relation::Contained));
        assert_eq!(nothing[1].command(), None);
        assert_eq!(
            labels(&drop_menu(&source, "main", Err("gone".into()))),
            ["gone"]
        );
    }

    fn refs_on(remotes: &[(&str, &str)]) -> Refs {
        let mut refs = Refs::default();
        refs.remote_configs = remotes
            .iter()
            .map(|(name, url)| remote::Remote {
                name: (*name).to_owned(),
                url: (*url).to_owned(),
                push_url: None,
            })
            .collect();
        refs
    }

    #[test]
    fn several_picked_branches_can_include_remote_ones() {
        let refs = refs_on(&[("origin", "git@github.com:octo/cat.git")]);
        let lines = refs_menu(
            &refs,
            &[
                target(RefKind::Remote, "origin/feat/x"),
                target(RefKind::Remote, "origin/feat/y"),
            ],
        );
        let Some(Command::DeleteMany { refs }) = lines.last().and_then(|l| l.command()) else {
            panic!("no delete line: {:?}", labels(&lines));
        };
        assert_eq!(
            refs[0].on_remote,
            Some(("origin".to_owned(), "feat/x".to_owned()))
        );
    }

    #[test]
    fn a_remote_branch_can_be_deleted_from_its_menu_after_asking() {
        let refs = refs_on(&[("origin", "git@github.com:octo/cat.git")]);
        let lines = ref_menu(&refs, &target(RefKind::Remote, "origin/feat/x"));
        let delete = lines.last().unwrap();
        assert!(matches!(
            delete.command(),
            Some(Command::DeleteRemote { remote, name, .. })
                if remote == "origin" && name == "feat/x"
        ));
        let ask = delete.command().unwrap().confirmation().unwrap();
        assert!(ask.message.contains("on origin"));
    }

    #[test]
    fn dropping_a_local_branch_on_a_remote_one_offers_a_push_only_when_it_fast_forwards() {
        let refs = refs_on(&[("origin", "git@github.com:octo/cat.git")]);
        let (from, onto) = (
            target(RefKind::Branch, "feat/x"),
            target(RefKind::Remote, "origin/main"),
        );
        let push = push_drop_menu(&refs, &from, &onto, Ok(Relation::FastForward));
        assert_eq!(labels(&push), ["Push feat/x to origin/main"]);
        assert_eq!(
            push[1].command(),
            Some(&Command::PushTo {
                branch: "feat/x".into(),
                remote: "origin".into(),
                name: "main".into()
            })
        );
        for relation in [Relation::Same, Relation::Contained, Relation::Diverged] {
            let lines = push_drop_menu(&refs, &from, &onto, Ok(relation));
            assert!(
                lines.iter().all(|line| line.command().is_none()),
                "{relation:?}"
            );
        }
    }

    #[test]
    fn dropping_a_remote_branch_on_another_opens_the_page_that_starts_a_request() {
        let refs = refs_on(&[("origin", "git@github.com:octo/cat.git")]);
        let (from, onto) = (
            target(RefKind::Remote, "origin/feat/x"),
            target(RefKind::Remote, "origin/main"),
        );
        let lines = request_drop_menu(&refs, &from, &onto, Ok(Relation::Diverged));
        assert_eq!(
            labels(&lines),
            ["Open a pull request for feat/x into main", "Copy link"]
        );
        assert_eq!(shown_lines(&lines)[0], "feat/x onto main");
        assert_eq!(lines[1].glyph(), Some(Glyph::GitHub));
        assert_eq!(lines[2].glyph(), None);
        let open = lines[1].command().unwrap();
        assert_eq!(
            open.url(),
            Some("https://github.com/octo/cat/compare/main...feat/x?expand=1")
        );
        assert_eq!(
            open.opened_notice().as_deref(),
            Some("Opened the page that starts a pull request on GitHub in your browser.")
        );
        assert_eq!(open.run(Path::new(".")), Ok(None));
        assert!(open.confirmation().is_none());

        let gitlab = refs_on(&[("origin", "git@gitlab.com:team/cat.git")]);
        let lines = request_drop_menu(&gitlab, &from, &onto, Ok(Relation::FastForward));
        assert_eq!(
            labels(&lines)[0],
            "Open a merge request for feat/x into main"
        );
        assert_eq!(lines[1].glyph(), Some(Glyph::GitLab));
    }

    #[test]
    fn a_forge_that_is_not_known_still_gets_the_line_with_a_question_on_it() {
        // Nothing at this address answers, so only a choice can settle which forge it is.
        let mut refs = refs_on(&[("origin", "http://127.0.0.1:1/me/app.git")]);
        let (from, onto) = (
            target(RefKind::Remote, "origin/feat/x"),
            target(RefKind::Remote, "origin/main"),
        );
        let lines = request_drop_menu(&refs, &from, &onto, Ok(Relation::Diverged));
        assert_eq!(
            labels(&lines),
            ["Open a request for feat/x into main"],
            "the line is there, and it is short"
        );
        assert_eq!(lines[1].glyph(), Some(Glyph::Unknown));
        let ask = lines[1].command().unwrap();
        assert_eq!(ask.url(), None);
        assert!(
            ask.forge_question()
                .unwrap()
                .starts_with("gitclient can't tell whether origin is on GitHub, GitLab or Gitea")
        );
        assert_eq!(ask.run(Path::new(".")), Ok(None));
        assert!(ask.confirmation().is_none());

        // Choosing a forge for the repository settles it.
        refs.forge = Some(Forge::Gitea);
        let lines = request_drop_menu(&refs, &from, &onto, Ok(Relation::Diverged));
        assert_eq!(
            labels(&lines),
            ["Open a pull request for feat/x into main", "Copy link"]
        );
        assert_eq!(lines[1].glyph(), Some(Glyph::Gitea));
        assert_eq!(
            lines[1].command().unwrap().url(),
            Some("http://127.0.0.1:1/me/app/compare/main...feat/x")
        );
    }

    #[test]
    fn a_drop_that_cannot_make_a_request_says_why() {
        let (from, onto) = (
            target(RefKind::Remote, "origin/feat/x"),
            target(RefKind::Remote, "origin/main"),
        );
        let said = |refs: &Refs, relation| {
            let lines = request_drop_menu(refs, &from, &onto, relation);
            assert_eq!(lines.len(), 2, "a heading and one line");
            assert_eq!(lines[1].command(), None);
            labels(&lines)[0].to_owned()
        };
        let github = refs_on(&[("origin", "git@github.com:o/r.git")]);

        assert_eq!(
            said(&github, Ok(Relation::Contained)),
            "Nothing to propose: main has all of feat/x."
        );
        assert_eq!(said(&github, Err("gone".into())), "gone");
        assert_eq!(
            said(&refs_on(&[]), Ok(Relation::Diverged)),
            "These branches aren't on a known remote."
        );
        for message in [
            "Nothing to propose: main has all of feat/x.",
            "Branches on different remotes can't be proposed yet.",
        ] {
            assert!(message.len() < 60, "short enough to read in a menu line");
        }

        let across = request_drop_menu(
            &refs_on(&[
                ("origin", "git@github.com:o/r.git"),
                ("fork", "git@github.com:me/r.git"),
            ]),
            &target(RefKind::Remote, "fork/feat/x"),
            &onto,
            Ok(Relation::Diverged),
        );
        assert!(labels(&across)[0].contains("different remotes"));
    }

    #[test]
    fn a_request_opens_on_its_forge_or_is_copied() {
        let Some(Hosting::Known(site)) = forge::hosting(
            &remote::Remote {
                name: "origin".into(),
                url: "https://gitlab.com/team/cat.git".into(),
                push_url: None,
            },
            None,
        ) else {
            panic!("gitlab.com is a known forge");
        };
        let lines = request_menu(&site, 42);
        assert_eq!(lines[1].glyph(), Some(Glyph::GitLab));
        assert_eq!(
            shown_lines(&lines),
            ["merge request !42", "Open !42 on GitLab", "Copy link"]
        );
        assert_eq!(
            lines[1].command().and_then(Command::url),
            Some("https://gitlab.com/team/cat/-/merge_requests/42")
        );
        assert_eq!(
            lines[2]
                .command()
                .and_then(|c| c.copied_notice())
                .as_deref(),
            Some("Copied the link.")
        );
    }

    #[test]
    fn copying_says_what_was_copied() {
        let command = copy("abc", "the hash");
        assert_eq!(command.copied_notice().as_deref(), Some("Copied the hash."));
        assert_eq!(command.run(Path::new(".")), Ok(None));
    }

    #[test]
    fn deleting_asks_in_a_popup_and_only_unmerged_unpushed_work_needs_two_presses() {
        let weight = |upstream, ahead_behind| {
            let mut refs = refs();
            let branch = refs
                .branches
                .iter_mut()
                .find(|b| b.name == "feat/x")
                .unwrap();
            branch.upstream = upstream;
            branch.ahead_behind = ahead_behind;
            let lines = ref_menu(&refs, &target(RefKind::Branch, "feat/x"));
            let delete = lines.last().unwrap().command().unwrap();
            assert!(matches!(delete, Command::Delete { .. }));
            delete.confirmation().unwrap().weight
        };
        // Nothing known: not shown to be merged, and no remote branch has it.
        assert_eq!(weight(None, None), Weight::DoublePress);
        assert_eq!(weight(Some((2, 0)), Some((2, 0))), Weight::DoublePress);
        // Pushed, merged, or both are recoverable.
        assert_eq!(weight(Some((0, 0)), Some((2, 0))), Weight::Popup);
        assert_eq!(weight(None, Some((0, 3))), Weight::Popup);
    }
}
