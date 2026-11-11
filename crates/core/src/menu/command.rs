//! What a menu line does when chosen: the command itself, what it says while it runs, how
//! it asks first when it can lose something, and running it.

use super::Line;
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
    /// Opens a menu of its own beside this one. Nothing runs.
    Submenu {
        lines: Vec<Line>,
    },
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
    /// Fast-forwards a local branch to the remote branch it follows.
    PullBranch {
        branch: String,
    },
    /// Sends a local branch to the remote branch it follows.
    PushBranch {
        branch: String,
    },
    /// Sends a local branch to a named branch on a remote.
    PushTo {
        branch: String,
        remote: String,
        name: String,
    },
    /// Sends a local branch over a remote branch that has commits it lacks, after asking.
    ForcePushTo {
        branch: String,
        remote: String,
        name: String,
        /// What forcing does and how to undo it, worked out when git refused the push.
        warning: String,
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
    /// A branch on a remote, which is removed there with `git push --delete`.
    DeleteRemote {
        remote: String,
        name: String,
        tip: Option<String>,
    },
    /// Sends a tag to a remote, which a plain push does for one that isn't there yet.
    PushTag {
        remote: String,
        name: String,
    },
    /// Removes a tag from a remote with `git push --delete`, and from here too when
    /// `also_local` is set.
    DeleteRemoteTag {
        remote: String,
        name: String,
        tip: Option<String>,
        also_local: bool,
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
    /// Locks a worktree so git won't prune or remove it, or lifts that lock.
    LockWorktree {
        path: PathBuf,
        lock: bool,
    },
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
    /// Applies stashes by their number and deletes each that applied.
    PopStashes {
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
    /// Handled by the interface, which opens the browser. `what` finishes the sentence
    /// "Opened ... in your browser."
    OpenUrl {
        url: String,
        what: String,
    },
    /// Handled by the interface, which shows the file in the system's file manager. `path`
    /// is relative to the repository.
    RevealPath {
        path: String,
    },
    /// Handled by the interface, which asks in a popup whether to open the settings where
    /// the forge is chosen. `message` is the question.
    AskForge {
        message: String,
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
    /// A small popup: Enter confirms, Escape cancels. Also for steps that rewrite rather than
    /// remove.
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
            Self::PullBranch { .. } => "Pulling",
            Self::PushBranch { .. } | Self::PushTo { .. } | Self::PushTag { .. } => "Pushing",
            Self::ForcePushTo { .. } => "Force pushing",
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
            Self::Delete { .. }
            | Self::DeleteMany { .. }
            | Self::DeleteRemote { .. }
            | Self::DeleteRemoteTag { .. } => "Deleting",
            Self::CherryPick { .. } => "Cherry-picking",
            Self::Revert { .. } => "Reverting",
            Self::Reset { .. } => "Resetting",
            Self::ApplyFile { .. } => "Applying",
            Self::SwitchWorktree { .. } | Self::OpenWorktreeInTab { .. } => "Switching",
            Self::Form(form) => form.progress(),
            Self::LockWorktree { lock: true, .. } => "Locking the worktree",
            Self::LockWorktree { lock: false, .. } => "Unlocking the worktree",
            Self::RemoveWorktree { .. } => "Removing the worktree",
            Self::OpenSubmoduleInTab { .. } => "Switching",
            Self::UpdateSubmodule { .. } | Self::UpdateSubmodules => "Updating",
            Self::RemoveRemote { .. } => "Removing the remote",
            Self::Copy { .. } => "Copying",
            Self::RevealPath { .. } => "Opening",
            Self::OpenUrl { .. } | Self::AskForge { .. } => "Opening",
            Self::Submenu { .. } => "Opening",
            Self::StashStaged { .. } => "Stashing",
            Self::ApplyStashes { .. } => "Applying",
            Self::PopStashes { .. } => "Popping",
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
                weight: Weight::Popup,
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
            Self::ForcePushTo { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Force push",
                weight: Weight::DoublePress,
            }),
            Self::DeleteRemote { remote, name, tip } => Some(Confirmation {
                message: branch::delete_remote_warning(remote, name, tip.as_deref()),
                action: "Delete",
                weight: Weight::DoublePress,
            }),
            Self::DeleteRemoteTag {
                remote,
                name,
                tip,
                also_local,
            } => Some(Confirmation {
                message: branch::delete_remote_tag_warning(
                    remote,
                    name,
                    tip.as_deref(),
                    *also_local,
                ),
                action: "Delete",
                weight: Weight::DoublePress,
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
                weight: Weight::Popup,
            }),
            Self::DeleteStashes { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Delete",
                weight: Weight::DoublePress,
            }),
            Self::DiscardChanges { warning, .. } => Some(Confirmation {
                message: warning.clone(),
                action: "Discard",
                weight: Weight::DoublePress,
            }),
            Self::DeleteMany { refs } => Some(Confirmation {
                message: branch::delete_many_warning(refs),
                action: "Delete",
                weight: if refs.iter().any(|doomed| doomed.on_remote.is_some()) {
                    Weight::DoublePress
                } else {
                    deletion_weight(refs.iter().any(|doomed| doomed.at_risk))
                },
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
                weight: Weight::DoublePress,
            }),
            _ => None,
        }
    }

    /// The force push to offer when this command is a push that git refused with `error`
    /// because the remote branch has commits the pushed one lacks. Blocks, so call it off
    /// the UI thread.
    pub fn force_after_rejection(&self, repo: &Path, error: &str) -> Option<Command> {
        if !branch::push_was_rejected(error) {
            return None;
        }
        let (branch, remote, name) = match self {
            Self::PushBranch { branch } => {
                let (remote, name) = branch::push_target(repo, branch).ok()?;
                (branch.clone(), remote, name)
            }
            Self::PushTo {
                branch,
                remote,
                name,
            } => (branch.clone(), remote.clone(), name.clone()),
            _ => return None,
        };
        let warning = branch::force_push_warning(repo, &branch, &remote, &name);
        Some(Self::ForcePushTo {
            branch,
            remote,
            name,
            warning,
        })
    }

    /// What to say after copying.
    pub fn copied_notice(&self) -> Option<String> {
        match self {
            Self::Copy { what, .. } => Some(format!("Copied {what}.")),
            _ => None,
        }
    }

    /// The repository-relative path to show in the file manager, for a command that does that.
    pub fn reveal_path(&self) -> Option<&str> {
        match self {
            Self::RevealPath { path } => Some(path),
            _ => None,
        }
    }

    /// The address to open in the browser, for a command that does that.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::OpenUrl { url, .. } => Some(url),
            _ => None,
        }
    }

    /// The question to ask about the forge, for a command that does that.
    pub fn forge_question(&self) -> Option<&str> {
        match self {
            Self::AskForge { message } => Some(message),
            _ => None,
        }
    }

    /// What to say after opening the browser.
    pub fn opened_notice(&self) -> Option<String> {
        match self {
            Self::OpenUrl { what, .. } => Some(format!("Opened {what} in your browser.")),
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
            Self::PullBranch { branch } => branch::pull_branch(repo, branch),
            Self::PushBranch { branch } => branch::push_branch(repo, branch),
            Self::PushTo {
                branch,
                remote,
                name,
            } => branch::push_to(repo, branch, remote, name),
            Self::ForcePushTo {
                branch,
                remote,
                name,
                ..
            } => branch::force_push_to(repo, branch, remote, name),
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
            Self::DeleteRemote { remote, name, .. } => {
                branch::delete_remote_branch(repo, remote, name)
            }
            Self::PushTag { remote, name } => branch::push_tag(repo, remote, name),
            Self::DeleteRemoteTag {
                remote,
                name,
                also_local,
                ..
            } => branch::delete_remote_tag(repo, remote, name, *also_local),
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
            Self::LockWorktree { path, lock } => worktree::set_locked(repo, path, *lock),
            Self::RemoveWorktree { path, .. } => worktree::remove(repo, path),
            Self::UpdateSubmodule { path, .. } => submodule::update(repo, path),
            Self::UpdateSubmodules => submodule::update_all(repo),
            Self::RemoveRemote { name, .. } => remote::remove(repo, name),
            Self::StashStaged { staged, message } => {
                stash::push_staged(repo, *staged, message.as_deref())
            }
            Self::ApplyStashes { indexes } => stash::apply_many(repo, indexes),
            Self::PopStashes { indexes } => stash::pop_many(repo, indexes),
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
            | Self::OpenUrl { .. }
            | Self::RevealPath { .. }
            | Self::AskForge { .. }
            | Self::Submenu { .. }
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
