//! What the context menus offer. Right-clicking a branch, a tag or commits, or dropping one
//! branch on another, asks here for the lines of a menu. Each line carries the command it
//! runs, so the interface only draws lines and hands the chosen command back to be run.

use crate::branch::{self, Dragged, DropAction, Relation};
use crate::conflict;
use crate::details;
use crate::git::{Commit, RefKind, Refs};
use crate::text::{count, short_oid};
use std::path::Path;

/// What a menu line does when chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    CheckOutRef {
        kind: RefKind,
        name: String,
    },
    CheckOutCommit {
        branch: Option<String>,
        oid: String,
    },
    MergeIntoCurrent {
        source: String,
    },
    Drop {
        action: DropAction,
        source: String,
        target: String,
    },
    Rebase {
        branch: String,
        onto: String,
    },
    Delete {
        kind: RefKind,
        name: String,
        /// Where the ref pointed, so the warning can say how to get it back.
        tip: Option<String>,
    },
    /// Oldest first, which is the order they're applied in.
    CherryPick {
        oids: Vec<String>,
    },
    /// Newest first, so that each undoes what is still there.
    Revert {
        oids: Vec<String>,
        is_merge: bool,
    },
    Reset {
        oid: String,
        hard: bool,
    },
    /// Handled by the interface, which owns the clipboard.
    Copy {
        text: String,
        what: &'static str,
    },
}

/// What to ask before running a command that can lose something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub message: String,
    pub action: &'static str,
}

impl Command {
    /// What the app is doing while the command runs.
    pub fn progress(&self) -> &'static str {
        match self {
            Self::CheckOutRef { .. } | Self::CheckOutCommit { .. } => "Checking out",
            Self::MergeIntoCurrent { .. } => "Merging",
            Self::Drop {
                action: DropAction::FastForward,
                ..
            } => "Fast-forwarding",
            Self::Drop {
                action: DropAction::Rebase,
                ..
            }
            | Self::Rebase { .. } => "Rebasing",
            Self::Drop { .. } => "Merging",
            Self::Delete { .. } => "Deleting",
            Self::CherryPick { .. } => "Cherry-picking",
            Self::Revert { .. } => "Reverting",
            Self::Reset { .. } => "Resetting",
            Self::Copy { .. } => "Copying",
        }
    }

    /// The question to answer first, for a command that rewrites or removes things.
    pub fn confirmation(&self) -> Option<Confirmation> {
        match self {
            Self::Rebase { branch, onto }
            | Self::Drop {
                action: DropAction::Rebase,
                source: branch,
                target: onto,
            } => Some(Confirmation {
                message: branch::rebase_warning(branch, onto),
                action: "Rebase",
            }),
            Self::Delete { kind, name, tip } => Some(Confirmation {
                message: branch::delete_warning(*kind, name, tip.as_deref()),
                action: "Delete",
            }),
            Self::Reset {
                oid, hard: true, ..
            } => Some(Confirmation {
                message: format!(
                    "This moves the branch to {} and throws away all uncommitted changes, \
                     which can't be brought back. The commits you leave behind can be, with \
                     `git reset --hard ORIG_HEAD`.",
                    short_oid(oid)
                ),
                action: "Discard and reset",
            }),
            _ => None,
        }
    }

    /// What to say after copying.
    pub fn copied_notice(&self) -> Option<String> {
        match self {
            Self::Copy { what, .. } => Some(format!("Copied {what}.")),
            _ => None,
        }
    }

    /// Runs the command. This blocks, so call it off the UI thread.
    pub fn run(&self, repo: &Path) -> Result<Option<String>, String> {
        match self {
            Self::CheckOutRef { kind, name } => branch::check_out_ref(repo, *kind, name),
            Self::CheckOutCommit { branch, oid } => {
                details::check_out(repo, branch.as_deref(), oid)
            }
            Self::MergeIntoCurrent { source } => branch::merge_into_current(repo, source),
            Self::Drop {
                action,
                source,
                target,
            } => branch::apply_drop(repo, *action, source, target),
            Self::Rebase { branch, onto } => conflict::rebase_branch(repo, branch, onto),
            Self::Delete { kind, name, .. } => match kind {
                RefKind::Tag => branch::delete_tag(repo, name),
                _ => branch::delete_branch(repo, name),
            },
            Self::CherryPick { oids } => match oids.as_slice() {
                [oid] => details::cherry_pick(repo, oid),
                _ => details::cherry_pick_many(repo, oids),
            },
            Self::Revert { oids, is_merge } => match oids.as_slice() {
                [oid] => details::revert(repo, oid, *is_merge),
                _ => details::revert_many(repo, oids),
            },
            Self::Reset { oid, hard } => details::reset(repo, oid, *hard),
            Self::Copy { .. } => Ok(None),
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
        }
    }

    fn unavailable(label: impl Into<String>) -> Self {
        Self::Item {
            label: label.into(),
            command: None,
            danger: false,
        }
    }

    fn danger(mut self) -> Self {
        if let Self::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    pub fn command(&self) -> Option<&Command> {
        match self {
            Self::Item { command, .. } => command.as_ref(),
            _ => None,
        }
    }
}

fn copy(text: &str, what: &'static str) -> Command {
    Command::Copy {
        text: text.to_owned(),
        what,
    }
}

/// The menu for a branch, remote branch or tag.
pub fn ref_menu(refs: &Refs, target: &Dragged) -> Vec<Line> {
    let current = refs.head_branch.as_deref();
    let Dragged { kind, name } = target.clone();
    let is_current = kind == RefKind::Branch && current == Some(name.as_str());
    let mut lines = vec![Line::Heading(name.clone())];

    lines.push(if is_current {
        Line::unavailable(format!("{name} is already checked out"))
    } else {
        let label = match kind {
            RefKind::Remote => format!("Check out {name} as a local branch"),
            RefKind::Tag => format!("Check out {name} without a branch"),
            _ => format!("Check out {name}"),
        };
        Line::item(
            label,
            Command::CheckOutRef {
                kind,
                name: name.clone(),
            },
        )
    });

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

    lines.push(Line::Separator);
    lines.push(Line::item("Copy name", copy(&name, "the name")));

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
                    name,
                    tip: tip.map(|tip| tip.to_string()),
                },
            )
            .danger(),
        );
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

    lines.push(if actions.can_check_out {
        Line::item(
            actions.check_out_label(),
            Command::CheckOutCommit {
                branch: actions.branch.clone(),
                oid: oid.clone(),
            },
        )
    } else {
        Line::unavailable("This commit is already checked out")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Branch, NamedRef};
    use crate::graph::LaneColor;

    fn commit(oid: &str, parents: &[&str]) -> Commit {
        Commit {
            oid: oid.into(),
            parents: parents.iter().map(|&p| p.into()).collect(),
            author: "Ada".into(),
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
    fn another_branch_can_be_merged_rebased_and_deleted() {
        let lines = ref_menu(&refs(), &target(RefKind::Branch, "feat/x"));
        assert_eq!(
            labels(&lines),
            [
                "Check out feat/x",
                "Merge feat/x into main",
                "Rebase main onto feat/x",
                "Copy name",
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
        assert_eq!(labels(&lines), ["main is already checked out", "Copy name"]);
        assert!(lines[1].command().is_none());
    }

    #[test]
    fn a_tag_is_merged_but_never_rebased_onto() {
        let lines = ref_menu(&refs(), &target(RefKind::Tag, "v1.0.0"));
        assert_eq!(
            labels(&lines),
            [
                "Check out v1.0.0 without a branch",
                "Merge v1.0.0 into main",
                "Copy name",
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
        let on_head = commit_menu(&refs(), &[head]);
        assert_eq!(
            labels(&on_head),
            [
                "This commit is already checked out",
                "Revert",
                "Copy hash",
                "Copy subject"
            ]
        );
        let elsewhere = commit_menu(&refs(), &[other]);
        assert_eq!(
            labels(&elsewhere),
            [
                "Check out",
                "Merge into main",
                "Cherry-pick onto main",
                "Revert",
                "Reset main to here, keeping changes",
                "Reset main to here, discarding changes",
                "Copy hash",
                "Copy subject"
            ]
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

    #[test]
    fn copying_says_what_was_copied() {
        let command = copy("abc", "the hash");
        assert_eq!(command.copied_notice().as_deref(), Some("Copied the hash."));
        assert_eq!(command.run(Path::new(".")), Ok(None));
    }
}
