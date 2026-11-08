//! The small forms that ask for a few words before something is made: a new branch's
//! name, or a remote's name and address. The interface draws every form the same way, as
//! a popup with one input per field, and asks here what it says, what its button is
//! called, what is wrong with what was typed, and what to run.

use super::command::Weight;
use crate::history::refs::Refs;
use crate::repo::{branch, remote};
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
        }
    }

    /// The inputs, in order. Values are passed back in the same order.
    pub fn fields(&self) -> &'static [Field] {
        match self {
            Self::NewBranch { .. } => BRANCH_FIELDS,
            Self::Remote { .. } => REMOTE_FIELDS,
        }
    }

    /// What the inputs hold when the form opens: an edited remote starts from itself.
    pub fn initial_values(&self, refs: &Refs) -> Vec<String> {
        if let Self::Remote {
            original: Some(name),
        } = self
            && let Some(remote) = refs.remote_configs.iter().find(|r| &r.name == name)
        {
            return vec![remote.name.clone(), remote.url.clone()];
        }
        vec![String::new(); self.fields().len()]
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
        };
        problem.map(str::to_owned)
    }

    /// What the app is doing while the form's command runs.
    pub fn progress(&self) -> &'static str {
        match self {
            Self::NewBranch { .. } => "Creating a branch",
            Self::Remote { original: None } => "Adding the remote",
            Self::Remote { original: Some(_) } => "Saving the remote",
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
