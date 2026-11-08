//! What the context menus offer. Right-clicking a branch, a tag or commits, or dropping one
//! branch on another, asks here for the lines of a menu. Each line carries the command it
//! runs, so the interface only draws lines and hands the chosen command back to be run.

pub mod command;
pub mod form;

use crate::history::Commit;
use crate::history::refs::{RefKind, Refs};
use crate::repo::branch::{self, Doomed, Dragged, Relation};
use crate::repo::conflict;
use crate::repo::remote;
use crate::repo::submodule;
use crate::repo::worktree;
use crate::text::{count, short_oid};
use crate::working::Side;
use command::Command;
use form::Form;
use std::path::Path;

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

/// The line that checks out a branch, remote branch or tag, or says it already is.
fn check_out_ref_line(current: Option<&str>, kind: RefKind, name: &str) -> Line {
    if kind == RefKind::Branch && current == Some(name) {
        return Line::unavailable(format!("{name} is already checked out"));
    }
    let label = match kind {
        RefKind::Remote => format!("Check out {name} as a local branch"),
        RefKind::Tag => format!("Check out {name} without a branch"),
        _ => format!("Check out {name}"),
    };
    Line::item(
        label,
        Command::CheckOutRef {
            kind,
            name: name.to_owned(),
        },
    )
}

/// The menu for a branch, remote branch or tag.
pub fn ref_menu(refs: &Refs, target: &Dragged) -> Vec<Line> {
    let current = refs.head_branch.as_deref();
    let Dragged { kind, name } = target.clone();
    let is_current = kind == RefKind::Branch && current == Some(name.as_str());
    let mut lines = vec![Line::Heading(name.clone())];

    lines.push(check_out_ref_line(current, kind, &name));

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

    lines.push(Line::item(
        format!("New branch from {name}"),
        Command::Form(Form::NewBranch {
            start: Some(name.clone()),
            label: name.clone(),
        }),
    ));
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
                    at_risk: branch::loses_commits(refs, kind, &name),
                    name,
                    tip: tip.map(|tip| tip.to_string()),
                },
            )
            .danger(),
        );
    }
    lines
}

/// Whether a ref can be deleted from the menu: a local branch that isn't checked out, or
/// a tag. A remote branch is somebody else's to remove.
fn why_not_deletable(refs: &Refs, target: &Dragged) -> Option<String> {
    let name = &target.name;
    match target.kind {
        RefKind::Tag => None,
        RefKind::Branch if refs.head_branch.as_deref() == Some(name.as_str()) => {
            Some(format!("{name} is checked out, so it stays"))
        }
        RefKind::Branch => None,
        _ => Some(format!("{name} is a remote branch, so it stays")),
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
    lines.push(Line::Separator);
    lines.push(Line::item("Copy path", copy(path, "the path")));
    lines
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
                paths: files,
                unstage: side.unstages(),
            },
        ));
        lines.push(Line::Separator);
    }
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

/// The menu for several picked branches and tags. What it can do to many at once is
/// copy their names and delete them.
pub fn refs_menu(refs: &Refs, targets: &[Dragged]) -> Vec<Line> {
    let mut lines = vec![Line::Heading(format!("{} selected", targets.len()))];
    let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    lines.push(Line::item(
        "Copy names",
        copy(&names.join("\n"), "the names"),
    ));

    let (deletable, kept): (Vec<&Dragged>, Vec<&Dragged>) = targets
        .iter()
        .partition(|target| why_not_deletable(refs, target).is_none());
    lines.push(Line::Separator);
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
            lines.push(check_out_ref_line(
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
    fn a_file_with_no_source_can_only_be_shown_and_copied() {
        let lines = file_menu("a.rs", 0, None);
        assert_eq!(
            shown_lines(&lines),
            ["a.rs", "Show changes in a.rs", "Copy path"]
        );
    }

    #[test]
    fn a_folder_stages_everything_under_it() {
        let files = vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()];
        let lines = folder_menu("crates/src", files, Some(Side::Unstaged));
        assert_eq!(
            shown_lines(&lines),
            ["crates/src/", "Stage 2 files in src/", "Copy path"]
        );
        let lines = folder_menu("src", Vec::new(), None);
        assert_eq!(shown_lines(&lines), ["src/", "Copy path"]);
    }
    use crate::history::lanes::LaneColor;
    use crate::history::refs::{Branch, NamedRef};

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
            upstream: None,
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
                "Copy names",
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
    fn only_a_checked_out_branch_leaves_nothing_to_delete() {
        let picks = [
            target(RefKind::Branch, "main"),
            target(RefKind::Remote, "origin/x"),
        ];
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
        assert_eq!(
            labels(&lines),
            [
                "main is already checked out",
                "New branch from main",
                "Copy name"
            ]
        );
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
                "New branch from v1.0.0",
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
        let mut refs = refs();
        refs.by_commit
            .insert("aaaaaaaa".into(), vec![named(RefKind::Branch, "main")]);
        let on_head = commit_menu(&refs, &[head]);
        assert_eq!(
            labels(&on_head),
            [
                "main is already checked out",
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
