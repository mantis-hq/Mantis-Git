//! The commit graph: a file git keeps beside its objects that lists every commit with its
//! parents, date and generation, so that walking history doesn't unpack each commit. It
//! makes loading the graph and counting ahead and behind many times faster in a large
//! repository.
//!
//! Git writes it during `gc`, which rarely runs, and never on clone, so a repository can go
//! a long time without one. This reads what the file lists, says whether it covers what the
//! refs reach, and writes the missing part the way `git maintenance` does.

use crate::ops::{run, run_with_input};
use crate::text::count;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The command that brings the commit graph up to date. With `--split` it adds a layer for
/// what is missing instead of rewriting everything, as `git maintenance` does.
const WRITE: [&str; 4] = ["commit-graph", "write", "--reachable", "--split"];

/// Why git won't use a commit graph in this repository, whatever the file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unusable {
    Shallow,
    TurnedOff,
    ReplaceRefs,
}

/// How the commit graph stands against the commits the refs reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    NoCommits,
    Unusable(Unusable),
    Missing,
    /// It lists some reachable commits and not `missing` others.
    Behind {
        missing: u32,
    },
    UpToDate,
    Unreadable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub health: Health,
    /// Commits the refs reach, as `--reachable` counts them.
    pub reachable: u32,
    /// The setting that stops git from writing it on its own, when one is off.
    pub writing_off: Option<&'static str>,
}

impl Report {
    /// Says how the commit graph stands, in a sentence or two.
    pub fn sentence(&self) -> String {
        let reachable = count(self.reachable, "commit");
        let mut sentence = match &self.health {
            Health::NoCommits => {
                "This repository has no commits yet, so there is nothing for a commit graph to \
                 list."
                    .to_owned()
            }
            Health::Unusable(Unusable::Shallow) => {
                "This is a shallow clone, and git doesn't use a commit graph in one.".to_owned()
            }
            Health::Unusable(Unusable::TurnedOff) => {
                "core.commitGraph is off, so git doesn't read a commit graph here.".to_owned()
            }
            Health::Unusable(Unusable::ReplaceRefs) => {
                "This repository has replace refs, and git doesn't use a commit graph while \
                 they apply."
                    .to_owned()
            }
            Health::Missing => format!(
                "There is no commit graph, so git reads each of the {reachable} one by one \
                 when it walks history."
            ),
            Health::Behind { missing } => format!(
                "The commit graph lists {} of the {reachable} your branches and tags reach. \
                 Git reads the other {} one by one.",
                self.reachable.saturating_sub(*missing),
                missing
            ),
            Health::UpToDate => {
                format!("The commit graph lists all {reachable} your branches and tags reach.")
            }
            Health::Unreadable(message) => {
                format!("The commit graph can't be read: {message}.")
            }
        };
        if let Some(key) = self.writing_off
            && !matches!(self.health, Health::NoCommits | Health::Unusable(_))
        {
            sentence.push_str(&format!(" {key} is off, so git won't write it on its own."));
        }
        sentence
    }

    /// The button that writes it, named for its result, when writing would help.
    pub fn action(&self) -> Option<String> {
        match self.health {
            Health::Missing => Some("Write the commit graph".to_owned()),
            Health::Behind { missing } => Some(format!(
                "Add {} to the commit graph",
                count(missing, "commit")
            )),
            Health::Unreadable(_) => Some("Write the commit graph again".to_owned()),
            _ => None,
        }
    }

    /// What the header says after writing it.
    pub fn written_message(&self) -> String {
        match self.health {
            Health::UpToDate => format!(
                "Wrote the commit graph. It lists all {}.",
                count(self.reachable, "commit")
            ),
            _ => format!("Wrote the commit graph. {}", self.sentence()),
        }
    }
}

/// Reads how the commit graph stands. It lists every commit the refs reach and reads the
/// file, so call it off the UI thread.
pub fn check(repo: &Path) -> Result<Report, String> {
    let settings = Settings::read(repo);
    let refs = run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objecttype) %(objectname) %(*objecttype) %(*objectname)",
        ],
    )?;
    let (tips, replace_refs) = commit_tips(&refs);
    let listed = if tips.is_empty() {
        String::new()
    } else {
        run_with_input(repo, &["rev-list", "--stdin"], tips.join("\n").as_bytes())?
    };
    let reachable: Vec<&str> = listed.lines().collect();
    let report = |health| Report {
        health,
        reachable: reachable.len() as u32,
        writing_off: settings.writing_off,
    };

    if reachable.is_empty() {
        return Ok(report(Health::NoCommits));
    }
    let shallow = run(repo, &["rev-parse", "--is-shallow-repository"])?;
    if shallow.trim() == "true" {
        return Ok(report(Health::Unusable(Unusable::Shallow)));
    }
    if !settings.reads_graph {
        return Ok(report(Health::Unusable(Unusable::TurnedOff)));
    }
    if replace_refs && settings.uses_replace_refs {
        return Ok(report(Health::Unusable(Unusable::ReplaceRefs)));
    }

    let layers = match read_layers(&objects_info(repo)?) {
        Ok(layers) => layers,
        Err(message) => return Ok(report(Health::Unreadable(message))),
    };
    if layers.is_empty() {
        return Ok(report(Health::Missing));
    }
    let mut missing = 0;
    for oid in &reachable {
        let bytes = decode_hex(oid)
            .ok_or_else(|| format!("unexpected output from git rev-list: {oid:?}"))?;
        if !layers.iter().any(|layer| layer.contains(&bytes)) {
            missing += 1;
        }
    }
    Ok(report(if missing == 0 {
        Health::UpToDate
    } else {
        Health::Behind { missing }
    }))
}

/// Brings the commit graph up to date, then reads how it stands.
pub fn write(repo: &Path) -> Result<Report, String> {
    run(repo, &WRITE)?;
    check(repo)
}

/// The settings that decide whether git reads the commit graph and writes it itself.
#[derive(Debug, PartialEq, Eq)]
struct Settings {
    reads_graph: bool,
    uses_replace_refs: bool,
    writing_off: Option<&'static str>,
}

impl Settings {
    fn read(repo: &Path) -> Self {
        // git exits with 1 when none of them is set, which reads as all defaults.
        let found = run(
            repo,
            &[
                "config",
                "--type=bool",
                "--get-regexp",
                r"^(core\.commitgraph|core\.usereplacerefs|gc\.writecommitgraph|maintenance\.commit-graph\.enabled)$",
            ],
        )
        .unwrap_or_default();
        Self::parse(&found)
    }

    fn parse(found: &str) -> Self {
        // The last value set wins, as it does in git.
        let off = |key: &str| {
            found
                .lines()
                .filter_map(|line| line.split_once(' '))
                .rfind(|(name, _)| name.eq_ignore_ascii_case(key))
                .is_some_and(|(_, value)| value.trim() == "false")
        };
        let writing_off = ["gc.writeCommitGraph", "maintenance.commit-graph.enabled"]
            .into_iter()
            .find(|key| off(key));
        Self {
            reads_graph: !off("core.commitGraph"),
            uses_replace_refs: !off("core.useReplaceRefs"),
            writing_off,
        }
    }
}

/// The commits refs point at, peeling tags, which are where `--reachable` starts, and
/// whether any ref is a replace ref.
fn commit_tips(refs: &str) -> (Vec<&str>, bool) {
    let mut tips = Vec::new();
    let mut replace = false;
    for line in refs.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let [name, kind, oid, peeled_kind, peeled] = fields[..] else {
            continue;
        };
        replace |= name.starts_with("refs/replace/");
        match (kind, peeled_kind) {
            ("commit", _) => tips.push(oid),
            ("tag", "commit") => tips.push(peeled),
            _ => {}
        }
    }
    tips.sort_unstable();
    tips.dedup();
    (tips, replace)
}

/// Where the commit graph lives: `objects/info`, which every worktree shares.
fn objects_info(repo: &Path) -> Result<PathBuf, String> {
    let path = run(repo, &["rev-parse", "--git-path", "objects/info"])?;
    let path = PathBuf::from(path.trim_end_matches(['\n', '\r']));
    Ok(if path.is_absolute() {
        path
    } else {
        repo.join(path)
    })
}

/// The files git reads: `commit-graph` if there is one, otherwise each layer the chain
/// names. No files at all is an empty list.
fn read_layers(info: &Path) -> Result<Vec<Layer>, String> {
    let single = info.join("commit-graph");
    if single.exists() {
        return Ok(vec![Layer::read(&single)?]);
    }
    let folder = info.join("commit-graphs");
    let chain = match std::fs::read_to_string(folder.join("commit-graph-chain")) {
        Ok(chain) => chain,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("commit-graph-chain won't open: {error}")),
    };
    chain
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|hash| Layer::read(&folder.join(format!("graph-{hash}.graph"))))
        .collect()
}

/// One commit-graph file: the ids of the commits it lists, sorted, back to back.
struct Layer {
    ids: Vec<u8>,
    width: usize,
}

impl Layer {
    /// Reads the id list of a file in git's commit-graph format: an 8-byte header, a table
    /// of chunks, and an `OIDL` chunk holding every commit id in order.
    fn read(path: &Path) -> Result<Self, String> {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let broken = |what: &str| format!("{name} {what}");
        let mut file = File::open(path).map_err(|error| broken(&format!("won't open: {error}")))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)
            .map_err(|_| broken("is too short"))?;
        if &header[..4] != b"CGPH" {
            return Err(broken("is not a commit-graph file"));
        }
        if header[4] != 1 {
            return Err(broken(&format!("has an unknown version, {}", header[4])));
        }
        let width = match header[5] {
            1 => 20,
            2 => 32,
            other => return Err(broken(&format!("has an unknown hash version, {other}"))),
        };
        let chunks = header[6] as usize;
        // Each chunk has an id and where it starts, and one more entry marks the end.
        let mut table = vec![0u8; (chunks + 1) * 12];
        file.read_exact(&mut table)
            .map_err(|_| broken("ends in its table of chunks"))?;
        let entry = |index: usize| {
            let row = &table[index * 12..(index + 1) * 12];
            let offset = u64::from_be_bytes(row[4..].try_into().expect("eight bytes"));
            (&row[..4], offset)
        };
        let index = (0..chunks)
            .find(|index| entry(*index).0 == b"OIDL")
            .ok_or_else(|| broken("has no list of commits"))?;
        let (start, end) = (entry(index).1, entry(index + 1).1);
        let length = end
            .checked_sub(start)
            .filter(|length| length % width as u64 == 0)
            .ok_or_else(|| broken("has a list of commits of the wrong size"))?;
        let length = usize::try_from(length).map_err(|_| broken("is too large"))?;
        let mut ids = vec![0u8; length];
        file.seek(SeekFrom::Start(start))
            .and_then(|_| file.read_exact(&mut ids))
            .map_err(|_| broken("ends in its list of commits"))?;
        Ok(Self { ids, width })
    }

    fn contains(&self, oid: &[u8]) -> bool {
        if oid.len() != self.width {
            return false;
        }
        let id = |index: usize| &self.ids[index * self.width..(index + 1) * self.width];
        let (mut low, mut high) = (0, self.ids.len() / self.width);
        while low < high {
            let middle = low + (high - low) / 2;
            match id(middle).cmp(oid) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return true,
            }
        }
        false
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;
    use tempfile::TempDir;

    fn health(repo: &Path) -> Health {
        check(repo).unwrap().health
    }

    #[test]
    fn a_new_repository_has_nothing_to_list() {
        let repo = Repo::new();
        let report = check(repo.path()).unwrap();
        assert_eq!(report.health, Health::NoCommits);
        assert_eq!(report.action(), None);
    }

    #[test]
    fn writing_lists_every_commit_and_new_ones_show_as_missing() {
        let repo = Repo::new();
        repo.commit("a");
        repo.commit("b");
        let report = check(repo.path()).unwrap();
        assert_eq!(
            (report.health.clone(), report.reachable),
            (Health::Missing, 2)
        );
        assert_eq!(report.action().as_deref(), Some("Write the commit graph"));

        let report = write(repo.path()).unwrap();
        assert_eq!(report.health, Health::UpToDate);
        assert_eq!(report.action(), None);
        assert_eq!(
            report.written_message(),
            "Wrote the commit graph. It lists all 2 commits."
        );

        repo.commit("c");
        repo.git(&["tag", "-a", "-m", "v1", "v1"]);
        let report = check(repo.path()).unwrap();
        assert_eq!(report.health, Health::Behind { missing: 1 });
        assert_eq!(
            report.action().as_deref(),
            Some("Add 1 commit to the commit graph")
        );
        assert_eq!(
            report.sentence(),
            "The commit graph lists 2 of the 3 commits your branches and tags reach. Git \
             reads the other 1 one by one."
        );

        // The second write adds a layer, and the layers are read together.
        assert_eq!(write(repo.path()).unwrap().health, Health::UpToDate);
    }

    #[test]
    fn a_single_file_written_by_gc_counts_and_becomes_the_base_of_a_chain() {
        let repo = Repo::new();
        repo.commit("a");
        repo.git(&["commit-graph", "write", "--reachable"]);
        assert_eq!(health(repo.path()), Health::UpToDate);
        repo.commit("b");
        assert_eq!(health(repo.path()), Health::Behind { missing: 1 });
        assert_eq!(write(repo.path()).unwrap().health, Health::UpToDate);
    }

    #[test]
    fn commits_only_a_deleted_branch_reached_dont_count() {
        let repo = Repo::new();
        repo.commit("a");
        repo.git(&["checkout", "-q", "-b", "gone"]);
        repo.commit("b");
        write(repo.path()).unwrap();
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["branch", "-q", "-D", "gone"]);
        let report = check(repo.path()).unwrap();
        assert_eq!((report.health, report.reachable), (Health::UpToDate, 1));
    }

    #[test]
    fn worktrees_share_the_graph() {
        let repo = Repo::new();
        repo.commit("a");
        write(repo.path()).unwrap();
        let other = TempDir::new().unwrap();
        let path = other.path().join("wt");
        repo.git(&["worktree", "add", "-q", path.to_str().unwrap()]);
        assert_eq!(health(&path), Health::UpToDate);
    }

    #[test]
    fn a_damaged_file_says_so_and_can_be_written_again() {
        let repo = Repo::new();
        repo.commit("a");
        let info = objects_info(repo.path()).unwrap();
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("commit-graph"), b"CGPH\x01").unwrap();
        let report = check(repo.path()).unwrap();
        assert_eq!(
            report.health,
            Health::Unreadable("commit-graph is too short".to_owned())
        );
        assert_eq!(
            report.action().as_deref(),
            Some("Write the commit graph again")
        );
    }

    #[test]
    fn turned_off_settings_are_named() {
        let repo = Repo::new();
        repo.commit("a");
        repo.git(&["config", "gc.writeCommitGraph", "false"]);
        let report = check(repo.path()).unwrap();
        assert_eq!(report.writing_off, Some("gc.writeCommitGraph"));
        assert!(
            report
                .sentence()
                .ends_with(" gc.writeCommitGraph is off, so git won't write it on its own."),
            "{}",
            report.sentence()
        );
        // Asking for it by hand still works.
        assert_eq!(write(repo.path()).unwrap().health, Health::UpToDate);

        repo.git(&["config", "core.commitGraph", "false"]);
        let report = check(repo.path()).unwrap();
        assert_eq!(report.health, Health::Unusable(Unusable::TurnedOff));
        assert_eq!(report.action(), None);
    }

    #[test]
    fn the_last_setting_wins() {
        let settings = Settings::parse(
            "gc.writecommitgraph false\ncore.commitgraph false\ncore.commitgraph true\n",
        );
        assert!(settings.reads_graph && settings.uses_replace_refs);
        assert_eq!(settings.writing_off, Some("gc.writeCommitGraph"));
        assert_eq!(Settings::parse("").writing_off, None);
    }

    #[test]
    fn replace_refs_and_shallow_clones_cant_use_one() {
        let repo = Repo::new();
        let a = repo.commit("a");
        let b = repo.commit("b");
        repo.git(&["replace", &b, &a]);
        assert_eq!(health(repo.path()), Health::Unusable(Unusable::ReplaceRefs));
        repo.git(&["config", "core.useReplaceRefs", "false"]);
        assert_eq!(health(repo.path()), Health::Missing);

        let clone = TempDir::new().unwrap();
        let url = format!("file://{}", repo.path().display());
        let path = clone.path().join("shallow");
        crate::testing::git(
            clone.path(),
            &["clone", "-q", "--depth", "1", &url, path.to_str().unwrap()],
        );
        let report = check(&path).unwrap();
        assert_eq!(report.health, Health::Unusable(Unusable::Shallow));
        assert_eq!(report.action(), None);
    }
}
