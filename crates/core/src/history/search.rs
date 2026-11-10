//! Finding commits: by what they say, who wrote them or their hash, or by a file they
//! changed. Git does the looking, over every branch and tag, so a result may be older
//! than what the graph has loaded.

use super::Commit;
use crate::ops::{self, Cancel};
use std::path::Path;

/// How many results are listed. A search that finds more says so.
pub const LIMIT: usize = 500;

/// What a query is matched against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// The commit's message, its author's name or email, or the start of its hash.
    #[default]
    Commit,
    /// The path of a file the commit changed.
    File,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Commit => "Message, author or hash",
            Mode::File => "File",
        }
    }
}

/// What the band says when a result is older than the history the graph has read.
pub fn not_loaded_message(loaded: usize, limit: u32) -> String {
    format!(
        "That commit is older than the {loaded} the graph has loaded. Scroll the graph \
         further back, or raise the number of commits loaded (now {limit}) in Settings → App."
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    pub commits: Vec<Commit>,
    /// There were more than [`LIMIT`], and only the newest are here.
    pub truncated: bool,
}

impl Results {
    /// The sentence over the list.
    pub fn summary(&self, query: &str) -> String {
        let query = query.trim();
        match (self.commits.len(), self.truncated) {
            (0, _) => format!("No commits match “{query}”."),
            (_, true) => format!("The newest {} commits match “{query}”.", self.commits.len()),
            (1, false) => format!("1 commit matches “{query}”."),
            (found, false) => format!("{found} commits match “{query}”."),
        }
    }
}

const FIELD: char = '\u{1f}';
const FORMAT: &str = "--format=%H%x1f%an%x1f%ae%x1f%at%x1f%s";

/// Lists the commits matching `query`, newest first. An empty query matches nothing.
pub fn search(repo: &Path, query: &str, mode: Mode, cancel: &Cancel) -> Result<Results, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Results::default());
    }
    let max = format!("--max-count={}", LIMIT + 1);
    let log = |filters: &[String]| -> Result<Vec<Commit>, String> {
        let mut args = vec!["log", "--exclude=refs/stash", "--all", FORMAT, max.as_str()];
        args.extend(filters.iter().map(String::as_str));
        let output = ops::run_cancellable(repo, &args, cancel)?;
        Ok(output.lines().filter_map(parse).collect())
    };
    // Options are built as `--name=value`, so a query starting with `-` is still a value.
    let mut commits = match mode {
        Mode::Commit => {
            // Given both, git wants a commit to match the message and the author, so
            // they are two searches, put together by date.
            let mut all = log(&["-i".into(), "-F".into(), format!("--grep={query}")])?;
            for commit in log(&["-i".into(), "-F".into(), format!("--author={query}")])? {
                if !all.iter().any(|known| known.oid == commit.oid) {
                    all.push(commit);
                }
            }
            all.sort_by_key(|commit| std::cmp::Reverse(commit.time));
            if let Some(found) = by_hash(repo, query, cancel)
                && !all.iter().any(|commit| commit.oid == found.oid)
            {
                all.insert(0, found);
            }
            all
        }
        // Wildcards in a plain pathspec cross folders, so this is "anywhere in the path".
        Mode::File => log(&["--".into(), format!(":(icase)*{query}*")])?,
    };
    let truncated = commits.len() > LIMIT;
    commits.truncate(LIMIT);
    Ok(Results { commits, truncated })
}

/// The commit whose hash starts with `query`, when it looks like the start of one.
fn by_hash(repo: &Path, query: &str, cancel: &Cancel) -> Option<Commit> {
    let is_hash = (4..=64).contains(&query.len()) && query.chars().all(|c| c.is_ascii_hexdigit());
    if !is_hash {
        return None;
    }
    let spec = format!("{query}^{{commit}}");
    let output = ops::run_cancellable(
        repo,
        &["log", "-1", FORMAT, "--end-of-options", &spec],
        cancel,
    )
    .ok()?;
    output.lines().next().and_then(parse)
}

fn parse(line: &str) -> Option<Commit> {
    let mut fields = line.splitn(5, FIELD);
    let oid = fields.next()?;
    let author = fields.next()?;
    let email = fields.next()?;
    let time = fields.next()?.parse().ok()?;
    let subject = fields.next()?;
    Some(Commit {
        oid: oid.into(),
        parents: Box::default(),
        author: author.into(),
        email: email.into(),
        time,
        subject: subject.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    fn find(repo: &Repo, query: &str, mode: Mode) -> Vec<String> {
        search(repo.path(), query, mode, &Cancel::new())
            .unwrap()
            .commits
            .iter()
            .map(|commit| commit.subject.to_string())
            .collect()
    }

    fn repo() -> (Repo, String) {
        let repo = Repo::new();
        repo.write("src/main.rs", "one");
        repo.commit("feat: add main");
        repo.write("docs/Guide.md", "text");
        let guide = repo.commit("docs: write the guide");
        repo.write("src/main.rs", "two");
        repo.commit("fix: change main");
        (repo, guide)
    }

    #[test]
    fn a_message_is_found_whatever_its_case() {
        let (repo, _) = repo();
        assert_eq!(
            find(&repo, "THE GUIDE", Mode::Commit),
            ["docs: write the guide"]
        );
    }

    #[test]
    fn an_author_is_found() {
        let (repo, _) = repo();
        let author = repo.git(&["log", "-1", "--format=%an"]);
        assert_eq!(find(&repo, &author, Mode::Commit).len(), 3);
        assert!(find(&repo, "nobody-of-that-name", Mode::Commit).is_empty());
    }

    #[test]
    fn the_start_of_a_hash_finds_its_commit() {
        let (repo, guide) = repo();
        assert_eq!(
            find(&repo, &guide[..7], Mode::Commit),
            ["docs: write the guide"]
        );
    }

    #[test]
    fn a_file_finds_every_commit_that_changed_it() {
        let (repo, _) = repo();
        assert_eq!(
            find(&repo, "main.rs", Mode::File),
            ["fix: change main", "feat: add main"]
        );
        assert_eq!(
            find(&repo, "guide.md", Mode::File),
            ["docs: write the guide"]
        );
        assert_eq!(find(&repo, "src/", Mode::File).len(), 2);
    }

    #[test]
    fn a_query_that_looks_like_an_option_is_only_text() {
        let (repo, _) = repo();
        assert!(find(&repo, "--all", Mode::File).is_empty());
        assert!(find(&repo, "-n", Mode::Commit).is_empty());
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let (repo, _) = repo();
        assert!(find(&repo, "  ", Mode::Commit).is_empty());
    }

    #[test]
    fn the_summary_counts() {
        let results = Results::default();
        assert_eq!(results.summary("x"), "No commits match “x”.");
    }
}
