//! What a menu line does when chosen: the command itself, what it says while it runs, how
//! it asks first when it can lose something, and running it.

use super::form::Form;
use crate::commit::actions;
use crate::history::refs::RefKind;
use crate::repo::branch::{self, Doomed, DropAction};
use crate::repo::conflict;
use crate::repo::remote;
use crate::repo::stash;
use crate::repo::submodule;
use crate::repo::worktree;
use crate::text::short_oid;
use crate::working;
use std::path::{Path, PathBuf};

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
        /// Whether deleting it loses commits that nothing else keeps.
        at_risk: bool,
    },
    /// Several branches and tags at once.
    DeleteMany {
        refs: Vec<Doomed>,
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
    /// Applies what a commit changed in one file to the working tree.
    ApplyFile {
        oid: String,
        path: String,
        /// Where a renamed file came from.
        old_path: Option<String>,
    },
    /// Handled by the interface, which owns the tabs: shows the worktree in this tab.
    SwitchWorktree {
        path: PathBuf,
    },
    /// Handled by the interface, which owns the tabs.
    OpenWorktreeInTab {
        path: PathBuf,
    },
    /// Handled by the interface, which opens the form: asks for the words a command needs,
    /// such as a new branch's name, then runs what the form says.
    Form(Form),
    RemoveWorktree {
        path: PathBuf,
        /// What removing it does and how to bring it back, worked out when the menu opened.
        warning: String,
    },
    /// Handled by the interface, which owns the tabs: shows a submodule in a tab.
    OpenSubmoduleInTab {
        path: PathBuf,
    },
    /// Downloads a submodule if need be and checks out the commit this repository records.
    UpdateSubmodule {
        path: String,
        /// What it does and how to undo it, when it moves the submodule off a commit.
        warning: Option<String>,
    },
    UpdateSubmodules,
    RemoveRemote {
        name: String,
        /// What removing it does and how to bring it back, worked out when the menu opened.
        warning: String,
    },
    /// Handled by the interface, which owns the diff: opens a file's changes. `index` is the
    /// file's place in what the inspector lists.
    ShowFile {
        index: usize,
    },
    /// Handled by the interface, which owns the list of changes: puts files into the index
    /// or takes them out.
    StageFiles {
        paths: Vec<String>,
        unstage: bool,
    },
    /// Throws away the unstaged changes in these files, after asking.
    DiscardChanges {
        paths: Vec<String>,
        /// What discarding does, worked out when the menu opened.
        warning: String,
    },
    /// Handled by the interface, which owns the resolver: opens a conflicted file's two
    /// sides next to each other.
    ShowConflict {
        path: String,
    },
    /// Handled by the interface, which owns the answers: takes one side for every decision
    /// in a conflicted file.
    UseSide {
        path: String,
        side: conflict::Side,
    },
    /// Sets what is staged aside. `message` names the stash, when one was typed.
    StashStaged {
        staged: usize,
        message: Option<String>,
    },
    /// Applies stashes by their number and keeps them.
    ApplyStashes {
        indexes: Vec<usize>,
    },
    DeleteStashes {
        indexes: Vec<usize>,
        /// What deleting them does, worked out when the menu opened.
        warning: String,
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
    pub weight: Weight,
}

/// How a confirmation is asked, by how much a mistake costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    /// A band under the header, for steps that rewrite rather than remove.
    Banner,
    /// A small popup: Enter confirms, Escape cancels.
    Popup,
    /// A popup for losing work that can't be found again: Enter has to be pressed twice,
    /// quickly.
    DoublePress,
}

impl Weight {
    /// The line under the question that says which keys answer it. `armed` is true once the
    /// first of two presses has landed.
    pub fn hint(self, action: &str, armed: bool) -> String {
        // Only the verb: what follows it is a name, which keeps its case.
        let mut chars = action.chars();
        let action = chars
            .next()
            .map(|first| first.to_lowercase().chain(chars).collect::<String>())
            .unwrap_or_default();
        match (self, armed) {
            (Self::DoublePress, false) => {
                format!("Press Enter twice to {action}, or Esc to cancel.")
            }
            (Self::DoublePress, true) => format!("Press Enter again to {action}."),
            _ => format!("Press Enter to {action}, or Esc to cancel."),
        }
    }
}

fn deletion_weight(at_risk: bool) -> Weight {
    if at_risk {
        Weight::DoublePress
    } else {
        Weight::Popup
    }
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
            Self::Delete { .. } | Self::DeleteMany { .. } => "Deleting",
            Self::CherryPick { .. } => "Cherry-picking",
            Self::Revert { .. } => "Reverting",
            Self::Reset { .. } => "Resetting",
            Self::ApplyFile { .. } => "Applying",
            Self::SwitchWorktree { .. } | Self::OpenWorktreeInTab { .. } => "Switching",
            Self::Form(form) => form.progress(),
            Self::RemoveWorktree { .. } => "Removing the worktree",
            Self::OpenSubmoduleInTab { .. } => "Switching",
            Self::UpdateSubmodule { .. } | Self::UpdateSubmodules => "Updating",
            Self::RemoveRemote { .. } => "Removing the remote",
            Self::Copy { .. } => "Copying",
            Self::StashStaged { .. } => "Stashing",
            Self::ApplyStashes { .. } => "Applying",
            Self::DeleteStashes { .. } => "Deleting",
            Self::ShowFile { .. } | Self::ShowConflict { .. } => "Opening",
            Self::UseSide { .. } => "Resolving",
            Self::DiscardChanges { .. } => "Discarding",
            Self::StageFiles { unstage: false, .. } => "Staging",
            Self::StageFiles { unstage: true, .. } => "Unstaging",
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
                weight: Weight::Banner,
            }),
            Self::Delete {
                kind,
                name,
                tip,
                at_risk,
            } => Some(Confirmation {
                message: branch::delete_warning(*kind, name, tip.as_deref()),
                action: "Delete",
                weight: deletion_weight(*at_risk),
            }),
            Self::RemoveWorktree { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Remove",
                weight: Weight::Popup,
            }),
            Self::RemoveRemote { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Remove",
                weight: Weight::Popup,
            }),
            Self::UpdateSubmodule {
                warning: Some(warning),
                ..
            } => Some(Confirmation {
                message: warning.clone(),
                action: "Check out",
                weight: Weight::Banner,
            }),
            Self::DeleteStashes { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Delete",
                weight: Weight::Popup,
            }),
            Self::DiscardChanges { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Discard",
                weight: Weight::Popup,
            }),
            Self::DeleteMany { refs } => Some(Confirmation {
                message: branch::delete_many_warning(refs),
                action: "Delete",
                weight: deletion_weight(refs.iter().any(|doomed| doomed.at_risk)),
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
                weight: Weight::Banner,
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
                actions::check_out(repo, branch.as_deref(), oid)
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
            Self::DeleteMany { refs } => branch::delete_many(repo, refs),
            Self::CherryPick { oids } => match oids.as_slice() {
                [oid] => actions::cherry_pick(repo, oid),
                _ => actions::cherry_pick_many(repo, oids),
            },
            Self::Revert { oids, is_merge } => match oids.as_slice() {
                [oid] => actions::revert(repo, oid, *is_merge),
                _ => actions::revert_many(repo, oids),
            },
            Self::Reset { oid, hard } => actions::reset(repo, oid, *hard),
            Self::ApplyFile {
                oid,
                path,
                old_path,
            } => actions::apply_file(repo, oid, path, old_path.as_deref()),
            Self::RemoveWorktree { path, .. } => worktree::remove(repo, path),
            Self::UpdateSubmodule { path, .. } => submodule::update(repo, path),
            Self::UpdateSubmodules => submodule::update_all(repo),
            Self::RemoveRemote { name, .. } => remote::remove(repo, name),
            Self::StashStaged { staged, message } => {
                stash::push_staged(repo, *staged, message.as_deref())
            }
            Self::ApplyStashes { indexes } => stash::apply_many(repo, indexes),
            Self::DeleteStashes { indexes, .. } => stash::drop_many(repo, indexes),
            Self::DiscardChanges { paths, .. } => {
                working::discard_files(repo, paths).map(|()| None)
            }
            Self::SwitchWorktree { .. }
            | Self::OpenWorktreeInTab { .. }
            | Self::OpenSubmoduleInTab { .. }
            | Self::Form(_)
            | Self::ShowFile { .. }
            | Self::ShowConflict { .. }
            | Self::UseSide { .. }
            | Self::StageFiles { .. }
            | Self::Copy { .. } => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_names_the_keys() {
        assert_eq!(
            Weight::Popup.hint("Delete", false),
            "Press Enter to delete, or Esc to cancel."
        );
        assert_eq!(
            Weight::DoublePress.hint("Delete", false),
            "Press Enter twice to delete, or Esc to cancel."
        );
        assert_eq!(
            Weight::DoublePress.hint("Delete", true),
            "Press Enter again to delete."
        );
        assert_eq!(
            Weight::Popup.hint("Save Origin", false),
            "Press Enter to save Origin, or Esc to cancel.",
            "a name keeps its case"
        );
    }
}
