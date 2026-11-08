//! A paused rebase, cherry-pick, revert or merge, and the decisions it is waiting on.
//!
//! Git's "ours" and "theirs" swap meaning during a rebase, so nothing here uses them.
//! Each conflict has a `mine` and an `other`, and every label names a branch.

use crate::ops::{self, git};
use crate::repo::status;
use crate::text::{count, short_oid};
use std::collections::BTreeSet;
use std::fs;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rebase,
    CherryPick,
    Revert,
    Merge,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Rebase => "Rebase",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Merge => "Merge",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Merge => "merge",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Applied,
    Paused,
    Waiting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub subject: String,
    pub state: StepState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Mine,
    Other,
}

/// One place where both sides changed the same thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub path: String,
    pub line: u32,
    pub mine: String,
    pub other: String,
    /// There are no lines to merge: a binary file, or one side deleted the file.
    pub whole_file: bool,
    pub mine_why: String,
    pub other_why: String,
}

impl Decision {
    /// What one side of the decision would put there.
    pub fn side(&self, side: Side) -> &str {
        match side {
            Side::Mine => &self.mine,
            Side::Other => &self.other,
        }
    }
}

/// One file with conflicts, and which of the decisions belong to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: String,
    /// Positions in [`Progress::decisions`], in order.
    pub decisions: Vec<usize>,
}

impl ConflictFile {
    /// How far along the file is, given how many of its conflicts are left in its result.
    pub fn note(&self, left: usize) -> String {
        match left {
            0 => "Ready to save".to_owned(),
            1 => "1 conflict left".to_owned(),
            left => format!("{left} conflicts left"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub kind: Kind,
    /// The branch being worked on.
    pub mine: String,
    /// What it is meeting: a branch where there is one, otherwise a commit.
    pub other: String,
    pub steps: Vec<Step>,
    pub decisions: Vec<Decision>,
    /// Files that were in conflict at this stop and are resolved now, in path order. Git
    /// keeps a record of them in the index, so this holds those from before the app was
    /// opened too.
    pub resolved: Vec<String>,
    mine_is_second: bool,
}

impl Progress {
    /// Puts git's two sides of a conflict in the order mine, other.
    fn by_side<T>(&self, first: T, second: T) -> (T, T) {
        if self.mine_is_second {
            (second, first)
        } else {
            (first, second)
        }
    }

    /// Whether `other` is the same stop as this one, with the same files still to settle or
    /// fewer: saving a file changes the decisions but not where the operation is.
    pub fn is_same_stop(&self, other: &Progress) -> bool {
        self.kind == other.kind
            && self.mine == other.mine
            && self.other == other.other
            && self.steps == other.steps
    }

    /// For each decision in `path`, in order: the block that still stands for it in `text`,
    /// or `None` once the text has dealt with it. `text` is the file's result, which starts
    /// as the file with its markers and is edited from there, so a block is matched by what
    /// is in it.
    pub fn open_chunks(&self, path: &str, text: &str) -> Vec<Option<Block>> {
        let mut blocks = blocks(text).into_iter().peekable();
        self.decisions
            .iter()
            .filter(|decision| decision.path == path && !decision.whole_file)
            .map(|decision| {
                let (first, second) = blocks.peek()?.sides(text);
                let (mine, other) = self.by_side(first, second);
                (mine == decision.mine && other == decision.other)
                    .then(|| blocks.next())
                    .flatten()
            })
            .collect()
    }

    /// Reads a file's result once for which of its conflicts still stand, and where each
    /// conflict is in it, so that picks can be put in its place and the result lined up
    /// with the file's two versions. `agreed[k]` is how many lines both sides agree on
    /// before conflict `k`, and `picked[k]` the text picks put in its place, if they did.
    ///
    /// A conflict still standing is where its markers are. A picked one is found by its
    /// text, where the lines before it end, or else further on before the next one
    /// standing. One that is neither was edited by hand: where it is can only be guessed,
    /// from the lines before it and the length of the branch's own side, and the next one
    /// found puts the count right again.
    pub fn check_result<S: AsRef<str>>(
        &self,
        path: &str,
        text: &str,
        agreed: &[usize],
        picked: &[Option<S>],
    ) -> ResultCheck {
        let open = self.open_chunks(path, text);
        let decisions: Vec<&Decision> = self
            .decisions
            .iter()
            .filter(|decision| decision.path == path && !decision.whole_file)
            .collect();
        let count = open.len();
        // Where the next block standing starts, from each conflict on: a picked one is
        // looked for no further than that.
        let mut limits = vec![text.len(); count + 1];
        for index in (0..count).rev() {
            limits[index] = open[index].as_ref().map_or(limits[index + 1], |b| b.start);
        }

        let mut regions = Vec::with_capacity(count);
        let mut at = 0;
        for index in 0..count {
            let expected = skip_lines(text, at, agreed.get(index).copied().unwrap_or(0));
            let picks = picked.get(index).and_then(Option::as_ref);
            let region = match (&open[index], picks) {
                (Some(block), _) => Some(block.start..block.end),
                (None, Some(picks)) => {
                    let limit = limits[index + 1];
                    let picks = picks.as_ref();
                    find_line_start(text, picks, expected, limit)
                        .or_else(|| find_line_start(text, picks, at, limit))
                        .map(|start| start..start + picks.len())
                }
                (None, None) => None,
            };
            at = region.as_ref().map_or(expected, |region| region.end);
            regions.push(region);
        }

        // The regions' lines, counted in one pass down the text, and a guess for the rest.
        let (mut line, mut counted) = (0, 0);
        let mut line_at = |byte: usize| {
            line += text[counted..byte].matches('\n').count();
            counted = byte;
            line
        };
        let mut places = Vec::with_capacity(count);
        let mut at = 0;
        for (index, region) in regions.iter().enumerate() {
            at += agreed.get(index).copied().unwrap_or(0);
            let place = match region {
                Some(region) => line_at(region.start)..line_at(region.end),
                None => {
                    let guess = decisions.get(index).map_or(0, |d| lines_of(&d.mine).len());
                    at..at + guess
                }
            };
            at = place.end;
            places.push(place);
        }
        // A guess that ran past the next conflict found is pulled back to it.
        let mut limit = usize::MAX;
        for place in places.iter_mut().rev() {
            place.end = place.end.min(limit);
            place.start = place.start.min(place.end);
            limit = place.start;
        }
        ResultCheck {
            standing: open.iter().map(Option::is_some).collect(),
            places,
            regions,
        }
    }

    /// A file's text cut into what both sides agree on and what they fight over, in order,
    /// with each conflict's sides put in the order mine, other.
    pub fn pieces(&self, text: &str) -> Vec<Piece> {
        split_conflicts(text)
            .into_iter()
            .map(|segment| match segment {
                Segment::Text(text) => Piece::Text(text.to_owned()),
                Segment::Conflict { first, second } => {
                    let (mine, other) = self.by_side(first, second);
                    Piece::Conflict {
                        mine: mine.to_owned(),
                        other: other.to_owned(),
                    }
                }
            })
            .collect()
    }

    /// The files with decisions to make, in the order their first decision comes.
    pub fn files(&self) -> Vec<ConflictFile> {
        let mut files: Vec<ConflictFile> = Vec::new();
        for (index, decision) in self.decisions.iter().enumerate() {
            match files.iter_mut().find(|file| file.path == decision.path) {
                Some(file) => file.decisions.push(index),
                None => files.push(ConflictFile {
                    path: decision.path.clone(),
                    decisions: vec![index],
                }),
            }
        }
        files
    }

    /// Says what happened in plain words.
    pub fn sentence(&self) -> String {
        let decisions = match self.decisions.len() {
            0 => "nothing to decide, so it can carry on".to_owned(),
            1 => format!("one decision in {}", self.decisions[0].path),
            count => format!("{count} decisions to make"),
        };
        match self.kind {
            Kind::Rebase => {
                let total = self.steps.len();
                let current = self
                    .steps
                    .iter()
                    .position(|step| step.state == StepState::Paused)
                    .map_or(total, |index| index + 1);
                format!(
                    "Replaying {} onto {}. Stopped at commit {current} of {total}: {decisions}.",
                    self.mine, self.other
                )
            }
            Kind::CherryPick => format!("Copying {} onto {}: {decisions}.", self.other, self.mine),
            Kind::Revert => format!("Reverting {} on {}: {decisions}.", self.other, self.mine),
            Kind::Merge => format!("Merging {} into {}: {decisions}.", self.other, self.mine),
        }
    }

    /// What the inspector's list of conflicted files is called, given how many files have
    /// been saved at this stop.
    pub fn list_heading(&self, resolved: usize) -> String {
        match (self.files().len(), resolved) {
            (0, 0) => "Nothing to decide".to_owned(),
            (0, _) => "Every conflict is resolved".to_owned(),
            (files, _) => format!("Conflicts · {}", count(files, "file")),
        }
    }

    /// What to tell the user when the last conflicted file is saved.
    pub fn resolved_sentence(&self) -> String {
        format!(
            "Every conflict is resolved. Continue the {} to go on.",
            self.kind.title().to_lowercase()
        )
    }

    /// What the list of steps shows.
    pub fn steps_heading(&self) -> String {
        if self.steps.is_empty() {
            format!("{} on {}", self.kind.title(), self.mine)
        } else {
            format!("Your commits, replayed on {}", self.other)
        }
    }

    /// What cancelling does, said before anyone does it.
    pub fn cancel_note(&self) -> String {
        format!("Cancelling puts {} back exactly as it was.", self.mine)
    }

    /// What's true now that it finished.
    pub fn finished_sentence(&self) -> String {
        match self.kind {
            Kind::Rebase => format!(
                "{} now sits on {}. Commits have new hashes, so the next push needs a force.",
                self.mine, self.other
            ),
            Kind::CherryPick => format!("{} is copied onto {}.", self.other, self.mine),
            Kind::Revert => format!("{} is reverted on {}.", self.other, self.mine),
            Kind::Merge => format!("{} is merged into {}.", self.other, self.mine),
        }
    }
}

/// The paused operation, if there is one.
pub fn detect(repo: &Path) -> Option<Progress> {
    let git_dir = ops::run(repo, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let git_dir = Path::new(git_dir.trim());
    let rebase_state = ["rebase-merge", "rebase-apply"]
        .map(|name| git_dir.join(name))
        .into_iter()
        .find(|dir| dir.is_dir());
    let (kind, target) = if rebase_state.is_some() {
        (Kind::Rebase, String::new())
    } else if let Some(oid) = read_oid(&git_dir.join("CHERRY_PICK_HEAD")) {
        (Kind::CherryPick, oid)
    } else if let Some(oid) = read_oid(&git_dir.join("REVERT_HEAD")) {
        (Kind::Revert, oid)
    } else {
        (Kind::Merge, read_oid(&git_dir.join("MERGE_HEAD"))?)
    };

    let branch = ops::run(repo, &["branch", "--show-current"]).unwrap_or_default();
    let branch = branch.trim().to_owned();
    let mut progress = Progress {
        kind,
        mine: branch,
        other: String::new(),
        steps: Vec::new(),
        decisions: Vec::new(),
        resolved: Vec::new(),
        mine_is_second: kind == Kind::Rebase,
    };

    let mut other_commit = target.clone();
    let mut mine_why = format!("On {}", progress.mine);
    match kind {
        Kind::Rebase => {
            let state = rebase_state?;
            let head_name = fs::read_to_string(state.join("head-name")).unwrap_or_default();
            progress.mine = head_name
                .trim()
                .strip_prefix("refs/heads/")
                .unwrap_or(head_name.trim())
                .to_owned();
            other_commit = fs::read_to_string(state.join("onto"))
                .unwrap_or_default()
                .trim()
                .to_owned();
            progress.other = name_commit(repo, &other_commit);
            progress.steps = rebase_steps(repo, &state);
            let paused = progress.steps.iter().find(|s| s.state == StepState::Paused);
            mine_why = match paused {
                Some(step) => format!("Your commit: {}", step.subject),
                None => "Your commit".to_owned(),
            };
        }
        Kind::Merge => progress.other = name_commit(repo, &target),
        Kind::CherryPick | Kind::Revert => {
            progress.other = short_oid(&target).to_owned();
        }
    }

    progress.decisions = read_decisions(repo, &progress, &other_commit, &mine_why);
    progress.resolved = read_resolved(repo, &progress.decisions);
    Some(progress)
}

/// The files git remembers as conflicted and since resolved, from the index's undo record
/// (`git ls-files --resolve-undo`), leaving out any that have conflicts again.
fn read_resolved(repo: &Path, decisions: &[Decision]) -> Vec<String> {
    let listing = ops::run(repo, &["ls-files", "--resolve-undo", "-z"]).unwrap_or_default();
    // One entry per stage the file had: `<mode> <oid> <stage>\t<path>`.
    let paths: BTreeSet<&str> = (listing.split('\0'))
        .filter_map(|entry| entry.split_once('\t').map(|(_, path)| path))
        .filter(|path| !decisions.iter().any(|decision| decision.path == *path))
        .collect();
    paths.into_iter().map(str::to_owned).collect()
}

fn read_oid(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let oid = text.lines().next()?.trim();
    (!oid.is_empty()).then(|| oid.to_owned())
}

/// A branch at the commit if there is one, otherwise its short hash.
fn name_commit(repo: &Path, oid: &str) -> String {
    let at = [
        "for-each-ref",
        "--format=%(refname:short)",
        "--points-at",
        oid,
    ];
    let names = ops::run(
        repo,
        &[at[0], at[1], at[2], at[3], "refs/heads", "refs/remotes"],
    );
    let names = names.unwrap_or_default();
    names
        .lines()
        .next()
        .map_or_else(|| short_oid(oid).to_owned(), str::to_owned)
}

fn rebase_steps(repo: &Path, state: &Path) -> Vec<Step> {
    let subject_of = |line: &str| {
        let mut fields = line.split_whitespace();
        let (action, oid) = (fields.next()?, fields.next()?);
        if action.starts_with('#') || matches!(action, "exec" | "x" | "label" | "reset" | "merge") {
            return None;
        }
        let subject = ops::run(repo, &["log", "-1", "--format=%s", oid]).ok()?;
        Some(subject.trim().to_owned())
    };
    let read = |name: &str| fs::read_to_string(state.join(name)).unwrap_or_default();

    let done: Vec<String> = read("done").lines().filter_map(subject_of).collect();
    let last = done.len().saturating_sub(1);
    let applied = done.into_iter().enumerate().map(|(index, subject)| Step {
        subject,
        state: if index == last {
            StepState::Paused
        } else {
            StepState::Applied
        },
    });
    let todo = read("git-rebase-todo");
    let waiting = todo.lines().filter_map(subject_of).map(|subject| Step {
        subject,
        state: StepState::Waiting,
    });
    applied.chain(waiting).collect()
}

fn read_decisions(
    repo: &Path,
    progress: &Progress,
    other_commit: &str,
    mine_why: &str,
) -> Vec<Decision> {
    let args = [
        "-c",
        "core.quotepath=off",
        "diff",
        "--name-only",
        "--diff-filter=U",
        "-z",
    ];
    let conflicted = ops::run(repo, &args).unwrap_or_default();
    let mut decisions = Vec::new();
    for path in conflicted.split('\0').filter(|path| !path.is_empty()) {
        let other_why = why_changed(repo, other_commit, path);
        let bytes = fs::read(repo.join(path)).ok();
        let text = bytes.and_then(|bytes| String::from_utf8(bytes).ok());
        let parsed = text.as_deref().map(split_conflicts).filter(|segments| {
            segments
                .iter()
                .any(|s| matches!(s, Segment::Conflict { .. }))
        });

        let Some(segments) = parsed else {
            let (mine, other) = whole_file_sides(repo, path, progress);
            decisions.push(Decision {
                path: path.to_owned(),
                line: 0,
                mine,
                other,
                whole_file: true,
                mine_why: mine_why.to_owned(),
                other_why,
            });
            continue;
        };
        let mut line = 1;
        for segment in segments {
            match segment {
                Segment::Text(text) => line += text.matches('\n').count() as u32,
                Segment::Conflict { first, second } => {
                    let (mine, other) = progress.by_side(first, second);
                    decisions.push(Decision {
                        path: path.to_owned(),
                        line,
                        mine: mine.to_owned(),
                        other: other.to_owned(),
                        whole_file: false,
                        mine_why: mine_why.to_owned(),
                        other_why: other_why.clone(),
                    });
                }
            }
        }
    }
    decisions
}

fn why_changed(repo: &Path, commit: &str, path: &str) -> String {
    if commit.is_empty() {
        return String::new();
    }
    let args = ["log", "-1", "--format=%ar: %s", commit, "--", path];
    match ops::run(repo, &args) {
        Ok(line) if !line.trim().is_empty() => format!("Changed {}", line.trim()),
        _ => String::new(),
    }
}

/// Which sides of a conflict without markers have the file at all.
fn file_stages(repo: &Path, path: &str) -> (bool, bool) {
    let listing = ops::run(repo, &["ls-files", "-u", "-z", "--", path]).unwrap_or_default();
    let stage = |number: &str| {
        listing.split('\0').any(|entry| {
            let meta = entry.split('\t').next().unwrap_or_default();
            meta.split(' ').nth(2) == Some(number)
        })
    };
    (stage("2"), stage("3"))
}

/// Describes each side's outcome. Each names its branch, which also keeps the two apart
/// when both sides simply have a different version of a binary file.
fn whole_file_sides(repo: &Path, path: &str, progress: &Progress) -> (String, String) {
    let describe = |exists: bool, branch: &str| {
        if exists {
            format!("Keeps the version on {branch}.")
        } else {
            format!("Deletes the file, as {branch} did.")
        }
    };
    let (first, second) = file_stages(repo, path);
    let (mine, other) = progress.by_side(first, second);
    (
        describe(mine, &progress.mine),
        describe(other, &progress.other),
    )
}

/// What [`Progress::check_result`] found in a file's result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultCheck {
    /// For each conflict, whether it still stands.
    pub standing: Vec<bool>,
    /// For each conflict, the lines it takes in the result.
    pub places: Vec<Range<usize>>,
    /// For each conflict, the bytes it takes in the result, where that is known: always
    /// while it stands, and for a picked one while its picks are there as they were put.
    /// `None` for one edited by hand.
    pub regions: Vec<Option<Range<usize>>>,
}

/// Where the line `lines` lines on from the one starting at `at` starts.
fn skip_lines(text: &str, mut at: usize, lines: usize) -> usize {
    for _ in 0..lines {
        match text[at..].find('\n') {
            Some(newline) => at += newline + 1,
            None => return text.len(),
        }
    }
    at
}

/// Where `needle` first stands at the start of a line in `text`, from `from` and ending
/// by `limit`.
fn find_line_start(text: &str, needle: &str, from: usize, limit: usize) -> Option<usize> {
    let haystack = text.get(from..limit)?;
    if needle.is_empty() {
        let line_start = from == 0 || text.as_bytes()[from - 1] == b'\n';
        return line_start.then_some(from);
    }
    haystack
        .match_indices(needle)
        .map(|(at, _)| from + at)
        .find(|&at| at == 0 || text.as_bytes()[at - 1] == b'\n')
}

/// The marker blocks of `text`, the file as git left it, in order: what a conflict goes
/// back to when nothing is picked for it.
pub fn marker_blocks(text: &str) -> Vec<String> {
    blocks(text)
        .iter()
        .map(|block| text[block.start..block.end].to_owned())
        .collect()
}

/// Where a position in one of two texts that are lined up falls in the other. `anchors`
/// are pairs of places, in lines, that match, in order in both; between two anchors the
/// lines are spread evenly. With `back`, `at` is in the second text and the answer in the
/// first.
pub fn line_up(anchors: &[(f32, f32)], at: f32, back: bool) -> f32 {
    let pick = |pair: &(f32, f32)| if back { (pair.1, pair.0) } else { *pair };
    let after = anchors.partition_point(|pair| pick(pair).0 <= at);
    match (
        after.checked_sub(1).map(|i| pick(&anchors[i])),
        anchors.get(after).map(pick),
    ) {
        (Some((from, to)), Some((next_from, next_to))) if next_from > from => {
            to + (at - from) / (next_from - from) * (next_to - to)
        }
        (Some((from, to)), _) => to + (at - from),
        (None, Some((next_from, next_to))) => next_to - (next_from - at),
        (None, None) => at,
    }
}

/// A part of a conflicted file: text both sides agree on, or a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Conflict { mine: String, other: String },
}

/// What the resolver says about a file's result, given how many conflicts it still holds.
pub fn result_hint(left: usize) -> String {
    match left {
        0 => "Ready to save".to_owned(),
        left => format!("{} left in the result", count(left, "conflict")),
    }
}

/// What a conflict's header says of it besides its buttons: nothing while it stands, that
/// it is in the result while its ticks make it up, and that it was edited in the result
/// once typing has taken over from them.
pub fn result_note(standing: bool, ticks_apply: bool) -> Option<&'static str> {
    match (standing, ticks_apply) {
        (true, _) => None,
        (false, true) => Some("in the result"),
        (false, false) => Some("edited in the result"),
    }
}

/// The conflict to go to from a point between two, where `at` is how many conflicts come
/// before that point: the first one after it that still stands in the result, or with
/// `forward` false the last one before it. It goes round at either end, and skips the
/// conflicts the result has settled. `None` once none stand.
pub fn next_standing(standing: &[bool], at: usize, forward: bool) -> Option<usize> {
    let at = at.min(standing.len());
    let open = |index: &usize| standing[*index];
    if forward {
        (at..standing.len()).chain(0..at).find(open)
    } else {
        (0..at).rev().chain((at..standing.len()).rev()).find(open)
    }
}

/// A block of conflict markers in a file's text, by byte position. It holds no text of its
/// own, so finding every block in a large file copies nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    /// Where the sides are, in the order git wrote them.
    first: Range<usize>,
    second: Range<usize>,
}

impl Block {
    /// The two sides in the order git wrote them, read from the text the block was found in.
    pub fn sides<'a>(&self, text: &'a str) -> (&'a str, &'a str) {
        (&text[self.first.clone()], &text[self.second.clone()])
    }

    /// What goes in the block's place to put `chosen` there. Typed or picked text often
    /// loses its last newline, which would join two lines, so it gets it back.
    pub fn replacement(&self, text: &str, chosen: &str) -> String {
        let mut replacement = chosen.to_owned();
        if !chosen.is_empty() && !chosen.ends_with('\n') && self.sides(text).0.ends_with('\n') {
            replacement.push('\n');
        }
        replacement
    }
}

/// The blocks of conflict markers in `text`, in order. A block that doesn't close is left
/// alone, as it is in [`split_conflicts`]. It reads the text once, however many blocks
/// there are.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = next_start(text, at) {
        let body = start
            + text[start..]
                .find('\n')
                .map_or(text.len() - start, |n| n + 1);
        // Once one doesn't close, none after it can, so the rest is text.
        let Some(read) = read_block(&text[body..]) else {
            break;
        };
        let end = body + read.end;
        found.push(Block {
            start,
            end,
            first: body..body + read.first_end,
            second: body + read.second.start..body + read.second.end,
        });
        at = end;
    }
    found
}

/// `text` with `block` replaced by `chosen`.
pub fn replace_block(text: &str, block: &Block, chosen: &str) -> String {
    let mut merged = String::with_capacity(text.len());
    merged.push_str(&text[..block.start]);
    merged.push_str(&block.replacement(text, chosen));
    merged.push_str(&text[block.end..]);
    merged
}

/// The lines of `text`, each with its newline, which is how they are picked and put back.
pub fn lines_of(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A file as it stands, markers and all: where its result starts.
pub fn read_text(repo: &Path, path: &str) -> Result<String, String> {
    let bytes = fs::read(repo.join(path)).map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Saves a file's result and stages it, which is what takes the conflict off it.
pub fn save_file(repo: &Path, path: &str, content: &str) -> Result<(), String> {
    fs::write(repo.join(path), content).map_err(|error| error.to_string())?;
    ops::run(repo, &["--literal-pathspecs", "add", "--", path]).map(drop)
}

/// Takes one side for every conflict in a file, and stages it.
pub fn resolve_with_side(
    repo: &Path,
    progress: &Progress,
    path: &str,
    side: Side,
) -> Result<(), String> {
    let decisions: Vec<Decision> = progress
        .decisions
        .iter()
        .filter(|decision| decision.path == path)
        .cloned()
        .collect();
    let answers: Vec<String> = decisions
        .iter()
        .map(|decision| decision.side(side).to_owned())
        .collect();
    let only = Progress {
        decisions,
        ..progress.clone()
    };
    resolve(repo, &only, &answers)
}

#[derive(Debug, PartialEq, Eq)]
enum Segment<'a> {
    Text(&'a str),
    Conflict { first: &'a str, second: &'a str },
}

/// Splits a file at its conflict markers. A marker block that doesn't close is left as
/// text, so a file that merely contains `<<<<<<<` isn't cut up.
fn split_conflicts(text: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut pending = 0;
    for block in blocks(text) {
        if block.start > pending {
            segments.push(Segment::Text(&text[pending..block.start]));
        }
        let (first, second) = block.sides(text);
        segments.push(Segment::Conflict { first, second });
        pending = block.end;
    }
    if pending < text.len() {
        segments.push(Segment::Text(&text[pending..]));
    }
    segments
}

/// Where the next line that opens a conflict starts, from the line starting at `at`.
fn next_start(text: &str, mut at: usize) -> Option<usize> {
    while at < text.len() {
        if text[at..].starts_with("<<<<<<< ") {
            return Some(at);
        }
        at += text[at..].find('\n')? + 1;
    }
    None
}

/// Where the parts of a conflict body are, by byte offset into it.
struct Read {
    /// Where the first side ends: at the base, if there is one, or the divider.
    first_end: usize,
    second: Range<usize>,
    /// Just after the closing marker's line.
    end: usize,
}

/// Reads a conflict body: the first side, an optional base, `=======`, the second side.
/// It goes down the lines once and stops at the closing marker, so reading every block in
/// a file reads the file once.
fn read_block(body: &str) -> Option<Read> {
    let mut base = None;
    let mut second_start = None;
    let mut offset = 0;
    while offset < body.len() {
        let line_end = body[offset..]
            .find('\n')
            .map_or(body.len(), |n| offset + n + 1);
        let line = &body[offset..line_end];
        match second_start {
            None if line == "=======\n" || line == "=======\r\n" => {
                second_start = Some((offset, line_end));
            }
            None if base.is_none() && line.starts_with("||||||| ") => base = Some(offset),
            None => {}
            Some((divider, start)) if line.starts_with(">>>>>>> ") => {
                return Some(Read {
                    first_end: base.unwrap_or(divider),
                    second: start..offset,
                    end: line_end,
                });
            }
            Some(_) => {}
        }
        offset = line_end;
    }
    None
}

/// One answer per decision, in order: for a whole-file decision the side's text as given.
pub fn resolve(repo: &Path, progress: &Progress, answers: &[String]) -> Result<(), String> {
    let mut decisions = progress.decisions.iter().zip(answers).peekable();
    while let Some((decision, answer)) = decisions.next() {
        let path = decision.path.as_str();
        if decision.whole_file {
            resolve_whole_file(repo, progress, decision, answer)?;
            continue;
        }
        let text = fs::read_to_string(repo.join(path)).map_err(|error| error.to_string())?;
        let mut merged = String::with_capacity(text.len());
        let mut answer = Some(answer.as_str());
        for segment in split_conflicts(&text) {
            match segment {
                Segment::Text(text) => merged.push_str(text),
                Segment::Conflict { first, .. } => {
                    let chosen = match answer.take() {
                        Some(chosen) => chosen,
                        None => {
                            let (_, next) = decisions.next().ok_or("too few answers")?;
                            next.as_str()
                        }
                    };
                    merged.push_str(chosen);
                    // Typed text often loses the last newline, which would join lines.
                    if first.ends_with('\n') && !chosen.is_empty() && !chosen.ends_with('\n') {
                        merged.push('\n');
                    }
                }
            }
        }
        fs::write(repo.join(path), merged).map_err(|error| error.to_string())?;
        ops::run(repo, &["add", "--", path])?;
    }
    Ok(())
}

fn resolve_whole_file(
    repo: &Path,
    progress: &Progress,
    decision: &Decision,
    answer: &str,
) -> Result<(), String> {
    let use_second = (answer == decision.mine) == progress.mine_is_second;
    let (first, second) = file_stages(repo, &decision.path);
    let (exists, flag) = if use_second {
        (second, "--theirs")
    } else {
        (first, "--ours")
    };
    if exists {
        ops::run(repo, &["checkout", flag, "--", &decision.path])?;
        ops::run(repo, &["add", "--", &decision.path])?;
    } else {
        ops::run(repo, &["rm", "-q", "-f", "--", &decision.path])?;
    }
    Ok(())
}

/// Applies the answers and moves on. Another pause isn't a failure, because the conflict
/// screen picks it up from [`detect`].
pub fn finish(repo: &Path, progress: &Progress, answers: &[String]) -> Result<(), String> {
    resolve(repo, progress, answers)?;
    carry_on(repo, progress.kind).map(|_| ())
}

/// Moves on once every conflict is resolved. Returns the next pause, if there is one.
pub fn carry_on(repo: &Path, kind: Kind) -> Result<Option<Progress>, String> {
    // A resolution can leave nothing to commit, which git refuses to continue from.
    let nothing_staged = ops::run(repo, &["diff", "--cached", "--quiet"]).is_ok();
    let step = match (kind, nothing_staged) {
        (Kind::Merge, _) => vec!["commit", "--no-edit"],
        (_, true) => vec![kind.command(), "--skip"],
        (_, false) => vec![kind.command(), "--continue"],
    };
    let result = ops::run_command(git(repo).args(&step).env("GIT_EDITOR", "true"), None);
    match (result, detect(repo)) {
        (_, Some(next)) => Ok(Some(next)),
        (Ok(_), None) => Ok(None),
        (Err(message), None) => Err(message),
    }
}

/// Abandons the operation, putting everything back.
pub fn cancel(repo: &Path, kind: Kind) -> Result<Option<String>, String> {
    ops::run(repo, &[kind.command(), "--abort"]).map(|_| None)
}

/// Says what a rebase does and how to undo it.
pub fn rebase_warning(target: &str) -> String {
    format!(
        "This replays your commits on top of {target} and gives them new hashes. To undo it \
         afterwards, run `git reset --hard ORIG_HEAD`."
    )
}

/// Starts a rebase. A conflict isn't a failure, because the conflict screen takes over.
pub fn rebase_onto(repo: &Path, target: &str) -> Result<Option<String>, String> {
    rebase(repo, &[target])
}

/// Rebases `branch` onto `target`, checking `branch` out first.
pub fn rebase_branch(repo: &Path, branch: &str, target: &str) -> Result<Option<String>, String> {
    rebase(repo, &[target, branch])
}

fn rebase(repo: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let mut command = git(repo);
    command.arg("rebase").args(args).env("GIT_EDITOR", "true");
    match ops::run_command(&mut command, None) {
        Err(_) if detect(repo).is_some() => Ok(None),
        Err(message) => Err(message),
        Ok(_) => Ok(None),
    }
}

/// The default branch, when rebasing onto it would do something: there's a local copy
/// with commits this branch doesn't have, and a branch to move that isn't that one.
pub fn rebase_target(repo: &Path) -> Option<String> {
    let current = ops::run(repo, &["branch", "--show-current"]).ok()?;
    let current = current.trim();
    let (target, _) = status::default_branch(repo)?;
    if current.is_empty()
        || current == target
        || !ops::resolves(repo, &format!("refs/heads/{target}"))
    {
        return None;
    }
    let range = format!("HEAD..{target}");
    let count = ops::run(repo, &["rev-list", "--count", &range]).ok()?;
    (count.trim().parse::<u32>().ok()? > 0).then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_are_grouped_by_file_in_the_order_they_come() {
        let decision = |path: &str, line| Decision {
            path: path.to_owned(),
            line,
            mine: String::new(),
            other: String::new(),
            whole_file: false,
            mine_why: String::new(),
            other_why: String::new(),
        };
        let progress = Progress {
            kind: Kind::Merge,
            mine: "main".to_owned(),
            other: "feat".to_owned(),
            steps: Vec::new(),
            decisions: vec![decision("b", 1), decision("a", 1), decision("b", 9)],
            resolved: Vec::new(),
            mine_is_second: false,
        };
        let files = progress.files();
        assert_eq!(files.len(), 2);
        assert_eq!(
            (files[0].path.as_str(), &files[0].decisions[..]),
            ("b", &[0, 2][..])
        );
        assert_eq!(files[0].note(1), "1 conflict left");
        assert_eq!(files[0].note(0), "Ready to save");
        assert_eq!(files[1].note(3), "3 conflicts left");
        assert_eq!(progress.list_heading(0), "Conflicts · 2 files");
        assert_eq!(result_hint(0), "Ready to save");
        assert_eq!(result_hint(2), "2 conflicts left in the result");
        assert_eq!(result_note(true, true), None);
        assert_eq!(result_note(false, true), Some("in the result"));
        assert_eq!(result_note(false, false), Some("edited in the result"));
    }
    use crate::testing::Repo;

    fn two_conflicts() -> (Progress, String) {
        let decision = |mine: &str, other: &str, line| Decision {
            path: "f".to_owned(),
            line,
            mine: mine.to_owned(),
            other: other.to_owned(),
            whole_file: false,
            mine_why: String::new(),
            other_why: String::new(),
        };
        let progress = Progress {
            kind: Kind::Merge,
            mine: "main".to_owned(),
            other: "feat".to_owned(),
            steps: Vec::new(),
            decisions: vec![decision("m1\n", "o1\n", 2), decision("m2\n", "o2\n", 8)],
            resolved: Vec::new(),
            mine_is_second: false,
        };
        let text = "a\n<<<<<<< HEAD\nm1\n=======\no1\n>>>>>>> feat\nb\n\
                    <<<<<<< HEAD\nm2\n=======\no2\n>>>>>>> feat\nc\n";
        (progress, text.to_owned())
    }

    #[test]
    fn blocks_are_found_by_position_and_replaced_in_the_result() {
        let (progress, text) = two_conflicts();
        let found = blocks(&text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].sides(&text), ("m1\n", "o1\n"));
        assert!(text[found[0].start..].starts_with("<<<<<<< HEAD"));
        assert!(text[..found[0].end].ends_with(">>>>>>> feat\n"));

        let open = progress.open_chunks("f", &text);
        assert!(open.iter().all(Option::is_some));
        // Picking the first conflict's lines leaves the second one standing.
        let picked = replace_block(&text, open[0].as_ref().unwrap(), "o1\n");
        assert!(picked.starts_with("a\no1\nb\n<<<<<<< HEAD"));
        let open = progress.open_chunks("f", &picked);
        assert!(open[0].is_none() && open[1].is_some());
        // Typing without the last newline doesn't join lines.
        let typed = replace_block(&picked, open[1].as_ref().unwrap(), "both");
        assert_eq!(typed, "a\no1\nb\nboth\nc\n");
        assert!(
            progress
                .open_chunks("f", &typed)
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn a_conflict_the_user_removed_by_hand_is_no_longer_open() {
        let (progress, text) = two_conflicts();
        let first = blocks(&text).remove(0);
        let by_hand = replace_block(&text, &first, "written myself\n");
        let open = progress.open_chunks("f", &by_hand);
        assert!(open[0].is_none());
        assert!(open[1].is_some());
    }

    #[test]
    fn a_file_is_cut_into_agreed_text_and_conflicts_with_the_sides_in_order() {
        let (mut progress, text) = two_conflicts();
        let pieces = progress.pieces(&text);
        assert_eq!(pieces.len(), 5);
        assert_eq!(pieces[0], Piece::Text("a\n".to_owned()));
        assert_eq!(
            pieces[1],
            Piece::Conflict {
                mine: "m1\n".to_owned(),
                other: "o1\n".to_owned()
            }
        );
        // In a rebase git writes the sides the other way round.
        progress.mine_is_second = true;
        assert_eq!(
            progress.pieces(&text)[1],
            Piece::Conflict {
                mine: "o1\n".to_owned(),
                other: "m1\n".to_owned()
            }
        );
    }

    #[test]
    fn going_between_conflicts_skips_settled_ones_and_goes_round() {
        let standing = [true, false, true, true];
        // From between the first and second: the third, since the second is settled.
        assert_eq!(next_standing(&standing, 1, true), Some(2));
        assert_eq!(next_standing(&standing, 1, false), Some(0));
        // Past the last it starts again at the first, and before the first at the last.
        assert_eq!(next_standing(&standing, 4, true), Some(0));
        assert_eq!(next_standing(&standing, 0, false), Some(3));
        assert_eq!(next_standing(&standing, 9, false), Some(3));
        assert_eq!(next_standing(&[false, false], 0, true), None);
        assert_eq!(next_standing(&[], 0, false), None);
    }

    #[test]
    fn conflicts_are_placed_in_the_result_by_line() {
        let (progress, text) = two_conflicts();
        // "a", the first block (5 lines), "b", the second, "c".
        let agreed = [1, 1];
        let none: [Option<&str>; 2] = [None, None];
        let check = progress.check_result("f", &text, &agreed, &none);
        assert_eq!(check.standing, [true, true]);
        assert_eq!(check.places, [1..6, 7..12]);
        assert_eq!(check.regions, [Some(2..42), Some(44..84)]);

        // The first picked with two lines: found by its text, so the second is found too.
        let block = blocks(&text).remove(0);
        let picked = replace_block(&text, &block, "x\ny\n");
        let check = progress.check_result("f", &picked, &agreed, &[Some("x\ny\n"), None]);
        assert_eq!(check.standing, [false, true]);
        assert_eq!(check.places, [1..3, 4..9]);
        assert_eq!(check.regions[0], Some(2..6));
        assert_eq!(&picked[2..6], "x\ny\n");

        // Edited by hand, it can't be found: it is taken to be as long as the branch's own
        // side, one line, and still ends where the next one standing starts.
        let check = progress.check_result("f", &picked, &agreed, &none);
        assert_eq!(check.places, [1..2, 4..9]);
        assert_eq!(check.regions[0], None);
    }

    #[test]
    fn a_pick_is_found_where_typing_moved_it() {
        let (progress, text) = two_conflicts();
        let block = blocks(&text).remove(0);
        let picked = replace_block(&text, &block, "x\n");
        // A line typed above it moves it down one.
        let typed = picked.replacen("a\n", "a\nnew\n", 1);
        let check = progress.check_result("f", &typed, &[1, 1], &[Some("x\n"), None]);
        assert_eq!(check.regions[0], Some(6..8));
        assert_eq!(check.places[0], 2..3);
        // An empty pick, as when the side taken has no lines, is where the lines before end.
        let emptied = replace_block(&text, &block, "");
        let check = progress.check_result("f", &emptied, &[1, 1], &[Some(""), None]);
        assert_eq!(check.regions[0], Some(2..2));
        assert_eq!(marker_blocks(&text)[0], &text[2..42]);
    }

    #[test]
    fn two_texts_are_lined_up_between_anchors() {
        // Ten rows above take four lines below, and the rest match one to one.
        let anchors = [(0., 0.), (5., 5.), (15., 9.)];
        assert_eq!(line_up(&anchors, 3., false), 3.);
        assert_eq!(line_up(&anchors, 10., false), 7.);
        assert_eq!(line_up(&anchors, 20., false), 14.);
        assert_eq!(line_up(&anchors, 7., true), 10.);
        assert_eq!(line_up(&[], 4., false), 4.);
    }

    #[test]
    fn lines_keep_their_newlines() {
        assert_eq!(lines_of("a\nb"), ["a\n", "b"]);
        assert!(lines_of("").is_empty());
    }

    /// `feature` and `main` both changed line 2 of `file`.
    fn diverged() -> Repo {
        let repo = Repo::new();
        repo.write("file", "one\ntwo\nthree\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("other", "fine\n");
        repo.commit("first of mine");
        repo.write("file", "one\nmine\nthree\n");
        repo.commit("second of mine");
        repo.write("later", "fine\n");
        repo.commit("third of mine");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("file", "one\nmain's\nthree\n");
        repo.commit("change on main");
        repo.git(&["checkout", "-q", "feature"]);
        repo
    }

    #[test]
    fn saving_a_result_takes_the_conflict_off_the_file_and_lets_the_rebase_go_on() {
        let repo = diverged();
        assert_eq!(rebase_onto(repo.path(), "main"), Ok(None));
        let progress = detect(repo.path()).unwrap();

        // The result starts as the file with its markers, and is edited from there.
        let text = read_text(repo.path(), "file").unwrap();
        let open = progress.open_chunks("file", &text);
        assert_eq!(open.len(), 1);
        let block = open[0].as_ref().unwrap();
        let both = format!(
            "{}{}",
            progress.decisions[0].mine, progress.decisions[0].other
        );
        let result = replace_block(&text, block, &both);
        assert!(blocks(&result).is_empty());

        save_file(repo.path(), "file", &result).unwrap();
        // Staged, so git no longer lists it as unmerged.
        assert_eq!(repo.git(&["diff", "--name-only", "--diff-filter=U"]), "");
        assert!(progress.resolved.is_empty());
        let after = detect(repo.path()).unwrap();
        assert!(after.decisions.is_empty());
        assert!(after.is_same_stop(&progress));
        // Git remembers it, so a window opened later still lists it.
        assert_eq!(after.resolved, ["file"]);

        assert!(carry_on(repo.path(), progress.kind).is_ok());
        assert_eq!(detect(repo.path()), None);
        assert!(repo.read("file").contains("mine") && repo.read("file").contains("main's"));
    }

    #[test]
    fn a_file_resolved_at_one_stop_isnt_listed_at_the_next() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.write("b", "1\n");
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("a", "feature\n");
        repo.commit("change a");
        repo.write("b", "feature\n");
        repo.commit("change b");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("a", "main\n");
        repo.write("b", "main\n");
        repo.commit("change both");
        repo.git(&["checkout", "-q", "feature"]);

        rebase_onto(repo.path(), "main").unwrap();
        let first = detect(repo.path()).unwrap();
        resolve_with_side(repo.path(), &first, "a", Side::Mine).unwrap();
        let next = carry_on(repo.path(), Kind::Rebase).unwrap().unwrap();
        assert_eq!(next.files()[0].path, "b", "the next commit stops on b");
        assert!(next.resolved.is_empty(), "{:?}", next.resolved);
    }

    #[test]
    fn one_side_can_be_taken_for_a_whole_file() {
        let repo = diverged();
        assert_eq!(rebase_onto(repo.path(), "main"), Ok(None));
        let progress = detect(repo.path()).unwrap();
        resolve_with_side(repo.path(), &progress, "file", Side::Other).unwrap();
        assert_eq!(repo.git(&["diff", "--name-only", "--diff-filter=U"]), "");
        assert_eq!(repo.read("file"), "one\nmain's\nthree\n");
    }

    #[test]
    fn a_clean_repository_has_nothing_paused() {
        let repo = Repo::new();
        repo.commit("base");
        assert_eq!(detect(repo.path()), None);
    }

    #[test]
    fn a_rebase_conflict_names_both_branches_and_the_right_sides() {
        let repo = diverged();
        assert_eq!(rebase_onto(repo.path(), "main"), Ok(None));

        let progress = detect(repo.path()).unwrap();
        assert_eq!(progress.kind, Kind::Rebase);
        assert_eq!(progress.mine, "feature");
        assert_eq!(progress.other, "main");

        // Git calls main "ours" here; to the person it is the other side.
        let decision = &progress.decisions[0];
        assert_eq!(decision.mine, "mine\n");
        assert_eq!(decision.other, "main's\n");
        assert_eq!((decision.path.as_str(), decision.line), ("file", 2));
        assert_eq!(decision.mine_why, "Your commit: second of mine");
        assert!(
            decision.other_why.contains("change on main"),
            "{}",
            decision.other_why
        );

        let states: Vec<_> = progress.steps.iter().map(|s| s.state).collect();
        assert_eq!(
            states,
            [StepState::Applied, StepState::Paused, StepState::Waiting]
        );
        assert_eq!(progress.steps[1].subject, "second of mine");
        assert_eq!(
            progress.sentence(),
            "Replaying feature onto main. Stopped at commit 2 of 3: one decision in file."
        );
    }

    #[test]
    fn resolving_and_carrying_on_finishes_the_rebase() {
        let repo = diverged();
        rebase_onto(repo.path(), "main").unwrap();
        let progress = detect(repo.path()).unwrap();

        resolve(repo.path(), &progress, &["both\n".to_owned()]).unwrap();
        assert_eq!(carry_on(repo.path(), Kind::Rebase), Ok(None));

        assert_eq!(detect(repo.path()), None);
        assert_eq!(repo.read("file"), "one\nboth\nthree\n");
        assert_eq!(repo.git(&["branch", "--show-current"]), "feature");
        assert_eq!(repo.git(&["rev-list", "--count", "main..feature"]), "3");
        assert!(repo.path().join("later").exists());
    }

    #[test]
    fn a_later_conflict_pauses_again_rather_than_failing() {
        let repo = diverged();
        repo.write("later", "changed by mine\n");
        repo.commit("fourth of mine");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("later", "main's version\n");
        repo.commit("add later on main");
        repo.git(&["checkout", "-q", "feature"]);

        rebase_onto(repo.path(), "main").unwrap();
        let first = detect(repo.path()).unwrap();
        resolve(repo.path(), &first, &["mine\n".to_owned()]).unwrap();
        let next = carry_on(repo.path(), Kind::Rebase).unwrap().unwrap();
        assert_ne!(next.decisions, first.decisions);
    }

    #[test]
    fn typed_answers_get_their_newline_back() {
        let repo = diverged();
        rebase_onto(repo.path(), "main").unwrap();
        let progress = detect(repo.path()).unwrap();
        resolve(repo.path(), &progress, &["typed".to_owned()]).unwrap();
        assert_eq!(repo.read("file"), "one\ntyped\nthree\n");
    }

    #[test]
    fn a_resolution_that_leaves_nothing_skips_the_commit() {
        let repo = diverged();
        rebase_onto(repo.path(), "main").unwrap();
        let progress = detect(repo.path()).unwrap();

        let keep_main = progress.decisions[0].other.clone();
        resolve(repo.path(), &progress, &[keep_main]).unwrap();
        assert_eq!(carry_on(repo.path(), Kind::Rebase), Ok(None));
        assert_eq!(repo.git(&["rev-list", "--count", "main..feature"]), "2");
    }

    #[test]
    fn cancelling_puts_the_branch_back_exactly() {
        let repo = diverged();
        let before = repo.git(&["rev-parse", "HEAD"]);
        rebase_onto(repo.path(), "main").unwrap();
        let progress = detect(repo.path()).unwrap();
        assert_eq!(
            progress.cancel_note(),
            "Cancelling puts feature back exactly as it was."
        );

        cancel(repo.path(), Kind::Rebase).unwrap();
        assert_eq!(detect(repo.path()), None);
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
        assert_eq!(repo.git(&["branch", "--show-current"]), "feature");
        assert_eq!(repo.read("file"), "one\nmine\nthree\n");
    }

    #[test]
    fn cherry_picks_and_merges_keep_mine_on_the_current_branch() {
        let repo = diverged();
        let theirs = repo.git(&["rev-parse", "main"]);
        repo.git(&["checkout", "-q", "--detach", "feature"]);
        repo.git(&["branch", "-f", "feature"]);
        repo.git(&["checkout", "-q", "feature"]);

        let result = ops::run(repo.path(), &["cherry-pick", &theirs]);
        assert!(result.is_err());
        let progress = detect(repo.path()).unwrap();
        assert_eq!(progress.kind, Kind::CherryPick);
        assert_eq!(progress.decisions[0].mine, "mine\n");
        assert_eq!(progress.decisions[0].other, "main's\n");
        resolve(repo.path(), &progress, &["mine\n".to_owned()]).unwrap();
        // That resolution changes nothing, so the pick is skipped.
        assert_eq!(carry_on(repo.path(), Kind::CherryPick), Ok(None));

        assert!(ops::run(repo.path(), &["merge", "--no-edit", "main"]).is_err());
        let progress = detect(repo.path()).unwrap();
        assert_eq!(progress.kind, Kind::Merge);
        assert_eq!(progress.other, "main");
        assert_eq!(progress.decisions[0].mine, "mine\n");
        resolve(repo.path(), &progress, &["merged\n".to_owned()]).unwrap();
        assert_eq!(carry_on(repo.path(), Kind::Merge), Ok(None));
        assert_eq!(repo.read("file"), "one\nmerged\nthree\n");
        assert_eq!(
            repo.git(&["rev-list", "--count", "--merges", "HEAD~1..HEAD"]),
            "1"
        );
    }

    #[test]
    fn a_deleted_file_is_a_whole_file_choice_and_either_side_works() {
        for keep_mine in [true, false] {
            let repo = Repo::new();
            repo.write("file", "one\n");
            repo.commit("base");
            repo.git(&["checkout", "-q", "-b", "feature"]);
            repo.write("file", "changed\n");
            repo.commit("change it");
            repo.git(&["checkout", "-q", "main"]);
            repo.git(&["rm", "-q", "file"]);
            repo.commit("delete it");
            repo.git(&["checkout", "-q", "feature"]);

            rebase_onto(repo.path(), "main").unwrap();
            let progress = detect(repo.path()).unwrap();
            let decision = &progress.decisions[0];
            assert!(decision.whole_file);
            assert_eq!(decision.mine, "Keeps the version on feature.");
            assert_eq!(decision.other, "Deletes the file, as main did.");

            let answer = if keep_mine {
                &decision.mine
            } else {
                &decision.other
            };
            resolve(repo.path(), &progress, std::slice::from_ref(answer)).unwrap();
            assert_eq!(carry_on(repo.path(), Kind::Rebase), Ok(None));
            assert_eq!(repo.path().join("file").exists(), keep_mine);
        }
    }

    #[test]
    fn binary_conflicts_are_whole_file_choices_and_either_side_can_win() {
        for (keep_mine, expected) in [(true, 2u8), (false, 3u8)] {
            let repo = Repo::new();
            fs::write(repo.path().join("blob"), [0u8, 1]).unwrap();
            repo.commit("base");
            repo.git(&["checkout", "-q", "-b", "feature"]);
            fs::write(repo.path().join("blob"), [0u8, 2]).unwrap();
            repo.commit("mine");
            repo.git(&["checkout", "-q", "main"]);
            fs::write(repo.path().join("blob"), [0u8, 3]).unwrap();
            repo.commit("main");
            repo.git(&["checkout", "-q", "feature"]);

            rebase_onto(repo.path(), "main").unwrap();
            let progress = detect(repo.path()).unwrap();
            let decision = &progress.decisions[0];
            assert!(decision.whole_file);
            let answer = if keep_mine {
                &decision.mine
            } else {
                &decision.other
            };
            resolve(repo.path(), &progress, std::slice::from_ref(answer)).unwrap();
            let _ = carry_on(repo.path(), Kind::Rebase);
            assert_eq!(fs::read(repo.path().join("blob")).unwrap(), [0u8, expected]);
        }
    }

    #[test]
    fn several_conflicts_in_a_file_take_their_own_answers() {
        let repo = Repo::new();
        let lines = |a: &str, b: &str| format!("{a}\n1\n2\n3\n4\n5\n6\n7\n{b}\n");
        repo.write("file", &lines("top", "bottom"));
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("file", &lines("my top", "my bottom"));
        repo.commit("mine");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("file", &lines("main top", "main bottom"));
        repo.commit("main");
        repo.git(&["checkout", "-q", "feature"]);

        rebase_onto(repo.path(), "main").unwrap();
        let progress = detect(repo.path()).unwrap();
        assert_eq!(progress.decisions.len(), 2);
        assert_eq!(progress.decisions[0].line, 1);
        assert!(progress.decisions[1].line > 5);

        let answers = ["A\n".to_owned(), "B\n".to_owned()];
        resolve(repo.path(), &progress, &answers).unwrap();
        assert_eq!(repo.read("file"), lines("A", "B"));
    }

    #[test]
    fn the_diff3_style_base_block_is_left_out_of_both_sides() {
        let text = "a\n<<<<<<< HEAD\nfirst\n||||||| base\nold\n=======\nsecond\n>>>>>>> x\nb\n";
        assert_eq!(
            split_conflicts(text),
            [
                Segment::Text("a\n"),
                Segment::Conflict {
                    first: "first\n",
                    second: "second\n"
                },
                Segment::Text("b\n"),
            ]
        );
    }

    #[test]
    fn an_unclosed_marker_is_just_text() {
        let text = "<<<<<<< not a conflict\nand nothing closes it\n";
        assert_eq!(split_conflicts(text), [Segment::Text(text)]);
        let after = "a\n<<<<<<< HEAD\nm\n=======\no\n>>>>>>> x\nb\n<<<<<<< open\nc\n";
        assert_eq!(split_conflicts(after).len(), 3);
    }

    #[test]
    fn windows_line_endings_are_read_too() {
        let text = "a\r\n<<<<<<< HEAD\r\nm\r\n=======\r\no\r\n>>>>>>> x\r\nb\r\n";
        let found = blocks(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sides(text), ("m\r\n", "o\r\n"));
        assert_eq!(&text[found[0].end..], "b\r\n");
    }

    /// Each conflict used to be read by scanning the rest of the file, so a file with
    /// thousands of them took seconds on every keystroke in the resolver.
    #[test]
    fn a_large_file_with_many_conflicts_is_read_in_one_pass() {
        let mut text = String::new();
        for conflict in 0..1400 {
            for line in 0..100 {
                text.push_str(&format!("let shared_{conflict}_{line} = {line};\n"));
            }
            text.push_str("<<<<<<< HEAD\nmine();\n=======\nother();\n>>>>>>> feat\n");
        }
        let started = std::time::Instant::now();
        assert_eq!(blocks(&text).len(), 1400);
        let pieces = split_conflicts(&text);
        assert_eq!(pieces.len(), 2800);
        // About 30 ms in a debug build, where the old reader took over 4 s.
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn every_usual_default_branch_can_be_rebased_onto() {
        let repo = Repo::new();
        repo.git(&["branch", "-m", "trunk"]);
        repo.commit("base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.git(&["checkout", "-q", "trunk"]);
        repo.commit("new on trunk");
        repo.git(&["checkout", "-q", "feature"]);
        assert_eq!(rebase_target(repo.path()), Some("trunk".to_owned()));
    }

    #[test]
    fn a_rebase_is_offered_only_off_the_default_branch_and_behind_it() {
        let repo = diverged();
        assert_eq!(rebase_target(repo.path()), Some("main".to_owned()));
        repo.git(&["checkout", "-q", "main"]);
        assert_eq!(rebase_target(repo.path()), None);
    }
}
