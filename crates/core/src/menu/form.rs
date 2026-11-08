//! The small forms that ask for a few words before something is made: a new branch's
//! name, a remote's name and address, or a submodule's address and folder. The interface draws every form the same way, as
//! a popup with one input per field, and asks here what it says, what its button is
//! called, what is wrong with what was typed, and what to run.

use super::command::Weight;
use crate::history::refs::Refs;
use crate::repo::{branch, remote, submodule};
use std::path::Path;

/// A form, and what it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    /// Names a branch that starts at `start`, or at the checked-out commit when `None`.
    /// `label` is what to call that place, such as `a1b2c3d` or `main`.
    NewBranch {
        start: Option<String>,
        label: String,
    },
    /// Adds a remote, or edits the one called `original`.
    Remote { original: Option<String> },
    /// Adds a submodule, or edits the one at the path `original`.
    Submodule { original: Option<String> },
    /// Picks the remote branch a local branch follows, which can be one the remote
    /// doesn't have yet. `existing` is every remote branch there is, as `origin/main`.
    Upstream {
        branch: String,
        existing: Vec<String>,
    },
}

/// One input of a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub placeholder: &'static str,
}

const BRANCH_FIELDS: &[Field] = &[Field {
    label: "Name",
    placeholder: "feat/my-change",
}];

const REMOTE_FIELDS: &[Field] = &[
    Field {
        label: "Name",
        placeholder: "origin",
    },
    Field {
        label: "Address",
        placeholder: "https://example.com/you/repo.git",
    },
];

const UPSTREAM_FIELDS: &[Field] = &[
    Field {
        label: "Remote",
        placeholder: "origin",
    },
    Field {
        label: "Branch",
        placeholder: "main",
    },
];

const SUBMODULE_ADD_FIELDS: &[Field] = &[
    Field {
        label: "Address",
        placeholder: "https://example.com/you/lib.git",
    },
    Field {
        label: "Folder",
        placeholder: "Named after the address when left empty",
    },
];

const SUBMODULE_EDIT_FIELDS: &[Field] = &[
    Field {
        label: "Address",
        placeholder: "https://example.com/you/lib.git",
    },
    Field {
        label: "Folder",
        placeholder: "vendor/lib",
    },
];

/// The trimmed value of field `index`, or nothing when there is no such field.
fn value(values: &[String], index: usize) -> &str {
    values.get(index).map_or("", |value| value.trim())
}

impl Form {
    /// The form for a new branch starting at the checked-out commit.
    pub fn new_branch_here(refs: &Refs) -> Self {
        Self::NewBranch {
            start: None,
            label: refs.head_name().to_owned(),
        }
    }

    /// The line at the top of the form, which says what it makes or changes.
    pub fn title(&self) -> String {
        match self {
            Self::NewBranch { label, .. } => format!("New branch, starting at {label}"),
            Self::Remote { original: None } => "Add a remote".to_owned(),
            Self::Remote {
                original: Some(name),
            } => format!("Edit {name}"),
            Self::Submodule { original: None } => "Add a submodule".to_owned(),
            Self::Submodule {
                original: Some(path),
            } => format!("Edit the submodule {path}"),
            Self::Upstream { branch, .. } => format!("Set the upstream of {branch}"),
        }
    }

    /// The inputs, in order. Values are passed back in the same order.
    pub fn fields(&self) -> &'static [Field] {
        match self {
            Self::NewBranch { .. } => BRANCH_FIELDS,
            Self::Remote { .. } => REMOTE_FIELDS,
            Self::Submodule { original: None } => SUBMODULE_ADD_FIELDS,
            Self::Submodule { original: Some(_) } => SUBMODULE_EDIT_FIELDS,
            Self::Upstream { .. } => UPSTREAM_FIELDS,
        }
    }

    /// What the inputs hold when the form opens: an edited remote or submodule starts
    /// from itself.
    pub fn initial_values(&self, refs: &Refs) -> Vec<String> {
        match self {
            Self::Remote {
                original: Some(name),
            } => {
                if let Some(remote) = refs.remote_configs.iter().find(|r| &r.name == name) {
                    return vec![remote.name.clone(), remote.url.clone()];
                }
            }
            Self::Submodule {
                original: Some(path),
            } => {
                if let Some(module) = refs.submodules.iter().find(|m| &m.path == path) {
                    let url = module.url.clone().unwrap_or_default();
                    return vec![url, module.path.clone()];
                }
            }
            Self::Upstream { branch, existing } => {
                // The remote that has a branch of the same name is the usual answer, else
                // the first one. The branch is the local one's name, which the remote may
                // not have yet.
                let remotes = self.options(0, refs);
                let same = remotes
                    .iter()
                    .find(|remote| existing.contains(&format!("{remote}/{branch}")));
                let remote = same.or(remotes.first()).cloned().unwrap_or_default();
                return vec![remote, branch.clone()];
            }
            _ => {}
        }
        vec![String::new(); self.fields().len()]
    }

    /// Whether field `index` is picked from `options` instead of typed.
    pub fn is_pick(&self, index: usize) -> bool {
        matches!(self, Self::Upstream { .. }) && index == 0
    }

    /// What goes between two fields drawn on one row, such as the `/` of `origin/main`.
    /// `None` draws each field on its own row.
    pub fn separator(&self) -> Option<&'static str> {
        matches!(self, Self::Upstream { .. }).then_some("/")
    }

    /// What a picked field offers.
    pub fn options(&self, index: usize, refs: &Refs) -> Vec<String> {
        match (self, index) {
            (Self::Upstream { .. }, 0) => refs
                .remote_configs
                .iter()
                .map(|remote| remote.name.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Existing things the typed field could be filled with instead, such as the remote's
    /// branches that contain what has been typed, a few at most.
    pub fn suggestions(&self, index: usize, values: &[String]) -> Vec<String> {
        const MOST: usize = 5;
        let Self::Upstream { existing, .. } = self else {
            return Vec::new();
        };
        if index != 1 {
            return Vec::new();
        }
        let prefix = format!("{}/", value(values, 0));
        let typed = value(values, 1).to_lowercase();
        existing
            .iter()
            .filter_map(|name| name.strip_prefix(&prefix))
            .filter(|name| *name != "HEAD" && name.to_lowercase().contains(&typed))
            .filter(|name| name.to_lowercase() != typed)
            .take(MOST)
            .map(str::to_owned)
            .collect()
    }

    /// Whether following `upstream` (`origin/x`) means making it on the remote first.
    fn is_new_upstream(&self, upstream: &str) -> bool {
        matches!(self, Self::Upstream { existing, .. } if !existing.iter().any(|name| name == upstream))
    }

    /// The button, named for its result: "Create feat/x", "Add remote", "Save origin".
    pub fn action(&self, values: &[String]) -> String {
        match self {
            Self::NewBranch { .. } => match value(values, 0) {
                "" => "Create branch".to_owned(),
                name => format!("Create {name}"),
            },
            Self::Remote { original: None } => "Add remote".to_owned(),
            Self::Remote {
                original: Some(name),
            } => format!("Save {name}"),
            Self::Submodule { original: None } => "Add submodule".to_owned(),
            Self::Submodule {
                original: Some(path),
            } => format!("Save {path}"),
            Self::Upstream { .. } => match (value(values, 0), value(values, 1)) {
                (remote, name) if !remote.is_empty() && !name.is_empty() => {
                    if self.is_new_upstream(&format!("{remote}/{name}")) {
                        // The whole remote branch, as it will be on the remote.
                        format!("Create {remote}/{name}")
                    } else {
                        format!("Follow {remote}/{name}")
                    }
                }
                _ => "Set upstream".to_owned(),
            },
        }
    }

    /// The line that says which keys answer the form.
    pub fn hint(&self, values: &[String]) -> String {
        Weight::Popup.hint(&self.action(values), false)
    }

    /// Why what was typed can't work, found without running git.
    pub fn problem(&self, values: &[String]) -> Option<String> {
        let problem = match self {
            Self::NewBranch { .. } => value(values, 0)
                .is_empty()
                .then_some("Name the new branch first."),
            Self::Remote { .. } => remote::why_bad_name(value(values, 0)).or(value(values, 1)
                .is_empty()
                .then_some("Give the remote an address to fetch from.")),
            Self::Submodule { original } => {
                let path = value(values, 1);
                if value(values, 0).is_empty() {
                    Some("Give the submodule an address to clone from.")
                } else if original.is_some() && path.is_empty() {
                    Some("Give the submodule a folder.")
                } else {
                    submodule::why_bad_path(path)
                }
            }
            Self::Upstream { .. } => {
                if value(values, 0).is_empty() {
                    Some("This repository has no remote to follow a branch of.")
                } else if value(values, 1).is_empty() {
                    Some("Type the name of the remote branch to follow.")
                } else {
                    None
                }
            }
        };
        problem.map(str::to_owned)
    }

    /// What the app is doing while the form's command runs.
    pub fn progress(&self) -> &'static str {
        match self {
            Self::NewBranch { .. } => "Creating a branch",
            Self::Remote { original: None } => "Adding the remote",
            Self::Remote { original: Some(_) } => "Saving the remote",
            Self::Submodule { original: None } => "Adding the submodule",
            Self::Submodule { original: Some(_) } => "Saving the submodule",
            Self::Upstream { .. } => "Setting the upstream",
        }
    }

    /// Does what the form asks for. It blocks on git, so call it off the UI thread. The
    /// error, if any, is said in the form, which stays open so it can be put right.
    pub fn submit(&self, repo: &Path, values: &[String]) -> Result<Option<String>, String> {
        if let Some(problem) = self.problem(values) {
            return Err(problem);
        }
        match self {
            Self::NewBranch { start, .. } => {
                branch::create_branch(repo, value(values, 0), start.as_deref())
            }
            Self::Remote { original } => remote::save(
                repo,
                original.as_deref(),
                value(values, 0),
                value(values, 1),
            ),
            Self::Submodule { original } => submodule::save(
                repo,
                original.as_deref(),
                value(values, 1),
                value(values, 0),
            ),
            Self::Upstream { branch, .. } => {
                branch::set_upstream(repo, branch, value(values, 0), value(values, 1))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::remote::Remote;
    use crate::testing::Repo;

    fn values(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn new_branch() -> Form {
        Form::NewBranch {
            start: None,
            label: "main".into(),
        }
    }

    #[test]
    fn a_new_branch_form_names_where_it_starts_and_what_it_makes() {
        let form = new_branch();
        assert_eq!(form.title(), "New branch, starting at main");
        assert_eq!(form.fields().len(), 1);
        assert_eq!(form.action(&values(&["  "])), "Create branch");
        assert_eq!(form.action(&values(&[" feat/X "])), "Create feat/X");
        assert_eq!(
            form.hint(&values(&["feat/X"])),
            "Press Enter to create feat/X, or Esc to cancel.",
            "names keep their case"
        );
        assert_eq!(
            form.problem(&values(&[""])).as_deref(),
            Some("Name the new branch first.")
        );
        assert_eq!(form.problem(&values(&["topic"])), None);
    }

    fn upstream_form(branch: &str, refs: &Refs) -> Form {
        Form::Upstream {
            branch: branch.into(),
            existing: refs.remotes.iter().map(|r| r.name.clone()).collect(),
        }
    }

    fn upstream_refs() -> Refs {
        let named = |name: &str| crate::history::refs::NamedRef {
            name: name.into(),
            tip: "aaaaaaaa".into(),
            color: Default::default(),
        };
        let remote = |name: &str| Remote {
            name: name.into(),
            ..Remote::default()
        };
        let mut refs = Refs::default();
        refs.remote_configs = vec![remote("origin"), remote("fork")];
        refs.remotes = vec![
            named("origin/main"),
            named("origin/HEAD"),
            named("fork/feat/x"),
        ];
        refs
    }

    #[test]
    fn an_upstream_form_picks_a_remote_and_types_a_branch() {
        let refs = upstream_refs();
        let form = upstream_form("feat/x", &refs);
        assert_eq!(form.separator(), Some("/"));
        assert!(form.is_pick(0) && !form.is_pick(1));
        assert_eq!(form.options(0, &refs), ["origin", "fork"]);
        // The remote with a branch of the same name starts picked, with that name typed.
        let start = values(&["fork", "feat/x"]);
        assert_eq!(form.initial_values(&refs), start);
        assert_eq!(form.action(&start), "Follow fork/feat/x");
        assert_eq!(form.problem(&start), None);
    }

    #[test]
    fn typing_a_branch_the_remote_lacks_names_the_push() {
        let refs = upstream_refs();
        let form = upstream_form("topic", &refs);
        // It starts on the first remote with the local name typed in.
        let start = values(&["origin", "topic"]);
        assert_eq!(form.initial_values(&refs), start);
        assert_eq!(form.action(&start), "Create origin/topic");
        assert_eq!(form.problem(&start), None);
        assert_eq!(
            form.action(&values(&["origin", "main"])),
            "Follow origin/main"
        );
        assert_eq!(form.action(&values(&["origin", ""])), "Set upstream");
        assert!(
            form.problem(&values(&["origin", ""]))
                .unwrap()
                .contains("Type")
        );
        let none = form.initial_values(&Refs::default());
        assert!(form.problem(&none).unwrap().contains("no remote"));
    }

    #[test]
    fn what_is_typed_narrows_the_branches_suggested_from_the_remote() {
        let refs = upstream_refs();
        let form = upstream_form("topic", &refs);
        assert_eq!(form.suggestions(1, &values(&["origin", ""])), ["main"]);
        assert_eq!(form.suggestions(1, &values(&["origin", "MA"])), ["main"]);
        assert!(form.suggestions(1, &values(&["origin", "zzz"])).is_empty());
        assert!(
            form.suggestions(1, &values(&["origin", "main"])).is_empty(),
            "an exact match needs no suggestion"
        );
        assert!(form.suggestions(0, &values(&["origin", ""])).is_empty());
    }

    #[test]
    fn a_remote_form_adds_or_edits_and_turns_down_names_that_cant_work() {
        let add = Form::Remote { original: None };
        assert_eq!(
            (add.title(), add.action(&[])),
            ("Add a remote".into(), "Add remote".into())
        );
        assert_eq!(add.initial_values(&Refs::default()), values(&["", ""]));
        assert!(
            add.problem(&values(&["-x", "url"]))
                .unwrap()
                .contains("dash")
        );
        assert!(
            add.problem(&values(&["up", " "]))
                .unwrap()
                .contains("address")
        );
        assert_eq!(add.problem(&values(&["up", "url"])), None);

        let edit = Form::Remote {
            original: Some("origin".into()),
        };
        assert_eq!(
            (edit.title(), edit.action(&[])),
            ("Edit origin".into(), "Save origin".into())
        );
        let mut refs = Refs::default();
        refs.remote_configs = vec![Remote {
            name: "origin".into(),
            url: "git@host:a.git".into(),
            push_url: None,
        }];
        assert_eq!(
            edit.initial_values(&refs),
            values(&["origin", "git@host:a.git"]),
            "an edit starts from the remote as it is"
        );
    }

    #[test]
    fn a_submodule_form_adds_or_edits_and_turns_down_folders_outside() {
        let add = Form::Submodule { original: None };
        assert_eq!(
            (add.title(), add.action(&[])),
            ("Add a submodule".into(), "Add submodule".into())
        );
        assert_eq!(add.fields()[0].label, "Address", "the cursor starts there");
        assert_eq!(add.initial_values(&Refs::default()), values(&["", ""]));
        assert!(
            add.problem(&values(&[" ", "lib"]))
                .unwrap()
                .contains("address")
        );
        assert!(
            add.problem(&values(&["u", "../lib"]))
                .unwrap()
                .contains("inside")
        );
        assert_eq!(
            add.problem(&values(&["u", ""])),
            None,
            "git names the folder"
        );

        let edit = Form::Submodule {
            original: Some("vendor/lib".into()),
        };
        assert_eq!(
            (edit.title(), edit.action(&[])),
            (
                "Edit the submodule vendor/lib".into(),
                "Save vendor/lib".into()
            )
        );
        assert!(
            edit.problem(&values(&["u", " "]))
                .unwrap()
                .contains("folder")
        );
        let mut refs = Refs::default();
        refs.submodules = vec![crate::repo::submodule::Submodule {
            name: "vendor/lib".into(),
            path: "vendor/lib".into(),
            folder: "/code/app/vendor/lib".into(),
            url: Some("https://example.com/lib.git".into()),
            recorded: "a".repeat(40),
            head: None,
            state: crate::repo::submodule::State::NotDownloaded,
        }];
        assert_eq!(
            edit.initial_values(&refs),
            values(&["https://example.com/lib.git", "vendor/lib"]),
            "an edit starts from the submodule as it is"
        );
    }

    #[test]
    fn submitting_runs_git_and_says_why_it_refused() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit("base");

        let said = new_branch()
            .submit(repo.path(), &values(&["topic"]))
            .unwrap();
        assert!(said.unwrap().contains("Created topic"));
        assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "topic");
        let taken = new_branch().submit(repo.path(), &values(&["topic"]));
        assert_eq!(taken, Err("A branch called topic already exists.".into()));
        assert_eq!(
            new_branch().submit(repo.path(), &values(&[""])),
            Err("Name the new branch first.".into()),
            "checks come before git"
        );

        let add = Form::Remote { original: None };
        add.submit(repo.path(), &values(&["up", "https://example.com/a.git"]))
            .unwrap();
        assert_eq!(remote::list(repo.path())[0].name, "up");
    }
}
