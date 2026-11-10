//! What the About page says: which build this is, where its source is, and the licenses of
//! everything compiled into it. The licenses travel inside the program, so they can't be
//! separated from a copy that is passed on.
//!
//! The crates' licenses are written by `cargo about` into `THIRD-PARTY-LICENSES.txt` at the
//! repository root and read here. Maple Mono is not a crate,
//! so its license is added by hand. Everything works without a window.
//!
//! `cargo about` writes one entry for each distinct text, so a license that crates word a
//! little differently (a copyright line, a space) comes out many times. [`licenses`] puts
//! them together: one [`License`] for each name, and under it one [`Text`] for each body,
//! with every copyright line of every crate that used it kept.

use crate::install::update::Build;
use crate::settings::config::{RECOMMENDED, Version};
use crate::text::count;
use std::sync::OnceLock;

/// Where the source of this program is published.
pub const SOURCE_URL: &str = "https://github.com/pretext8335/gitclient";

/// What `cargo about` wrote, one entry per distinct license text.
const CRATES: &str = include_str!("../../../THIRD-PARTY-LICENSES.txt");
const MAPLE_MONO: &str = include_str!("../../../src/fonts/OFL.txt");

/// The line that starts each entry of the generated file.
const RULE: &str =
    "================================================================================";

/// One license, and everything that is under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct License {
    /// "MIT License".
    pub name: String,
    /// What is under it, as "name version".
    pub used_by: Vec<String>,
    /// Its text, once for each wording that is really different.
    pub texts: Vec<Text>,
}

/// One wording of a license: the text, and the copyright lines of those who used it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// Who used this wording, as "name version".
    pub used_by: Vec<String>,
    /// Whose copyright it was used under, each set of lines with the projects it came with.
    pub notices: Vec<Notice>,
    /// The license itself, without those lines.
    pub body: String,
}

/// Copyright lines, and the projects they came with: "Copyright 2023 Jacob Pratt et al."
/// belongs to `deranged` and `powerfmt`, not to every crate under the license.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub copyrights: Vec<String>,
    pub used_by: Vec<String>,
}

impl License {
    /// The projects under this license, on one line.
    pub fn used_by_line(&self) -> String {
        self.used_by.join(", ")
    }

    /// How many projects are under this license: "3 projects".
    pub fn projects(&self) -> String {
        count(self.used_by.len(), "project")
    }

    /// Whether this is gitclient's own license.
    pub fn is_own(&self) -> bool {
        self.used_by
            .iter()
            .any(|name| name.split(' ').next() == Some("gitclient"))
    }

    /// Whether passing the program on means passing this license's text on with it.
    pub fn must_be_shown(&self) -> bool {
        !NO_NOTICE.contains(&self.name.as_str())
    }
}

impl Text {
    /// Whom this wording is for, when the license has more than one: "a 1.0, b 2.0", or
    /// "523 projects" when naming them would fill the page.
    pub fn used_by_line(&self) -> String {
        who(&self.used_by)
    }

    /// Every copyright line of every notice, as they were written.
    pub fn copyright_lines(&self) -> impl Iterator<Item = &String> {
        self.notices.iter().flat_map(|notice| &notice.copyrights)
    }
}

impl Notice {
    /// Which projects these lines came with: "a 1.0, b 2.0", or "12 projects".
    pub fn used_by_line(&self) -> String {
        who(&self.used_by)
    }
}

/// Names projects, or counts them when naming them would fill the page.
fn who(used_by: &[String]) -> String {
    if used_by.len() > 8 {
        count(used_by.len(), "project")
    } else {
        used_by.join(", ")
    }
}

/// Licenses that ask nothing of whoever passes the program on: no notice and no text. They
/// are in the generated file, since `cargo about` lists everything, but the page leaves
/// them out.
const NO_NOTICE: [&str; 3] = [
    "BSD Zero Clause License",
    "Creative Commons Zero v1.0 Universal",
    "The Unlicense",
];

/// The licenses that have to be shown for everything in this program, one for each name:
/// gitclient's own first, then the font, then the crates' in the order they were written.
pub fn licenses() -> &'static [License] {
    static ALL: OnceLock<Vec<License>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = parse(CRATES);
        all.retain(License::must_be_shown);
        all.insert(
            0,
            License {
                name: "SIL Open Font License 1.1".to_owned(),
                used_by: vec!["Maple Mono".to_owned()],
                texts: vec![text_of(vec!["Maple Mono".to_owned()], MAPLE_MONO)],
            },
        );
        let mut all = combine(all);
        // Stable, so the others keep the order they were written in.
        all.sort_by_key(|license| !license.is_own());
        all
    })
}

/// Reads the file `cargo about` writes with `about.hbs`, an entry for each distinct text. A
/// line is the start of an entry only when it is the rule followed by a name, an empty line
/// and "Used by:", so that a license text with a rule of its own stays in one piece.
pub fn parse(file: &str) -> Vec<License> {
    let lines: Vec<&str> = file.lines().collect();
    let starts_entry = |at: usize| {
        lines[at] == RULE
            && lines.get(at + 2) == Some(&"")
            && lines.get(at + 3) == Some(&"Used by:")
    };
    let starts: Vec<usize> = (0..lines.len()).filter(|&at| starts_entry(at)).collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let end = starts.get(n + 1).copied().unwrap_or(lines.len());
            let name = lines[start + 1].trim().to_owned();
            let body = &lines[start + 4..end];
            let split = body
                .iter()
                .position(|line| line.is_empty())
                .unwrap_or(body.len());
            let used_by: Vec<String> = body[..split]
                .iter()
                .map(|line| line.trim().to_owned())
                .collect();
            let text = text_of(used_by.clone(), &body[split..].join("\n"));
            License {
                name,
                used_by,
                texts: vec![text],
            }
        })
        .collect()
}

/// Puts the entries with the same name together, and in each the wordings that differ only
/// in their copyright lines or in white space. Every copyright line is kept, once.
pub fn combine(entries: Vec<License>) -> Vec<License> {
    let mut all: Vec<License> = Vec::new();
    for entry in entries {
        let Some(license) = all.iter_mut().find(|l| l.name == entry.name) else {
            all.push(entry);
            continue;
        };
        push_unique(&mut license.used_by, entry.used_by);
        for text in entry.texts {
            match license
                .texts
                .iter_mut()
                .find(|known| same_words(&known.body, &text.body))
            {
                Some(known) => {
                    push_unique(&mut known.used_by, text.used_by);
                    for notice in text.notices {
                        // The same lines from another project are one notice with both.
                        match known
                            .notices
                            .iter_mut()
                            .find(|have| have.copyrights == notice.copyrights)
                        {
                            Some(have) => push_unique(&mut have.used_by, notice.used_by),
                            None => known.notices.push(notice),
                        }
                    }
                }
                None => license.texts.push(text),
            }
        }
    }
    all
}

/// Adds what `from` has that `into` hasn't, in order.
fn push_unique(into: &mut Vec<String>, from: Vec<String>) {
    for item in from {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

/// Splits a license text into its copyright lines and the rest.
fn text_of(used_by: Vec<String>, text: &str) -> Text {
    let (copyright, rest): (Vec<&str>, Vec<&str>) =
        text.lines().partition(|line| is_copyright(line));
    // Taking lines out leaves runs of empty ones, so keep one.
    let mut body = String::new();
    let mut blank = true;
    for line in rest {
        let line = line.trim_end();
        if line.is_empty() {
            if !blank {
                body.push('\n');
            }
            blank = true;
        } else {
            body.push_str(line);
            body.push('\n');
            blank = false;
        }
    }
    let mut copyrights: Vec<String> = Vec::new();
    push_unique(
        &mut copyrights,
        copyright
            .iter()
            .map(|line| line.trim().to_owned())
            .collect(),
    );
    let notices = if copyrights.is_empty() {
        Vec::new()
    } else {
        vec![Notice {
            copyrights,
            used_by: used_by.clone(),
        }]
    };
    Text {
        used_by,
        notices,
        body: body.trim().to_owned(),
    }
}

/// Whether a line is somebody's notice ("Copyright 2023 A", "Copyright (c) B", "© C") and
/// not a sentence of the license that says the word, such as the Apache License's
/// "copyright notice that is included in or attached to the work", its "(c) You must
/// retain…", or its template "Copyright [yyyy] [name of copyright owner]".
fn is_copyright(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with('©') {
        return true;
    }
    let Some(rest) = line.strip_prefix("Copyright ") else {
        return false;
    };
    rest.starts_with(['(', '©']) || rest.starts_with(|c: char| c.is_ascii_digit())
}

/// Whether two texts say the same, whatever their line breaks and spacing.
fn same_words(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

/// "gitclient 0.1.0", with the commit when the build knows it ("gitclient 0.1.0 (1a2b3c4)").
pub fn version_line(build: &Build) -> String {
    format!("gitclient {}", build.label())
}

/// Where to get a newer git.
pub const GIT_DOWNLOAD_URL: &str = "https://git-scm.com/downloads";

/// The About page's row for the installed git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRow {
    /// "git 2.39", or "git" when there is none to name.
    pub title: String,
    pub sentence: String,
    /// The row offers a way to get a newer git.
    pub update: bool,
}

/// Says which git this is, and suggests an update when it is older than [`RECOMMENDED`].
pub fn git_row(git: &Result<Version, String>) -> GitRow {
    match git {
        Ok(version) if version.is_old() => GitRow {
            title: format!("git {version}"),
            sentence: format!(
                "gitclient works best with git {RECOMMENDED} or newer. This one can't count how \
                 far each branch is from the default branch, so the sidebar has no counts. \
                 Updating git brings them back."
            ),
            update: true,
        },
        Ok(version) => GitRow {
            title: format!("git {version}"),
            sentence: "This git has everything gitclient uses. The latest version is always \
                       the best one to run."
                .to_owned(),
            update: false,
        },
        Err(message) => GitRow {
            title: "git".to_owned(),
            sentence: message.clone(),
            update: false,
        },
    }
}

/// What the toast at launch says when the installed git is older than [`RECOMMENDED`].
/// Nothing for a git that is recent enough, or one that couldn't be read, which the
/// settings pages already say.
pub fn old_git_notice(git: &Result<Version, String>) -> Option<String> {
    let version = git.as_ref().ok().filter(|version| version.is_old())?;
    Some(format!(
        "This is git {version}, and gitclient is made for {RECOMMENDED} or newer. The \
         sidebar has no branch counts with it. Settings → About has more."
    ))
}

/// The sentence over the page's cards.
pub fn page_sentence() -> String {
    "The version you are running, and the licenses of everything in it.".to_owned()
}

/// What gitclient is licensed under, and what else is in it.
pub fn license_sentence() -> String {
    let others = licenses()
        .iter()
        .filter(|license| !license.is_own())
        .map(|license| license.used_by.len())
        .sum::<usize>();
    format!(
        "gitclient is free software under the GNU General Public License, version 3 or \
         later. It also contains {} by others, each under its own license, which are \
         listed here as they ask.",
        count(others, "project")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        format!(
            "{RULE}\nMIT License\n\nUsed by:\na 1.0.0\nb 2.0.0\n\nMIT License\n\nCopyright (c) A\n\n\
             {RULE}\nZlib\n\nUsed by:\nc 0.1.0\n\nline one\n{RULE}\nline three\n\n"
        )
    }

    #[test]
    fn an_old_git_is_named_and_an_update_suggested() {
        let old = git_row(&Ok(Version(2, 39)));
        assert_eq!(old.title, "git 2.39");
        assert!(old.update);
        assert!(old.sentence.contains("2.41"));

        let recent = git_row(&Ok(Version(2, 41)));
        assert_eq!(recent.title, "git 2.41");
        assert!(!recent.update);
        assert!(!git_row(&Ok(Version(3, 0))).update);

        let missing = git_row(&Err("Git wasn't found.".to_owned()));
        assert_eq!(missing.sentence, "Git wasn't found.");
        assert!(!missing.update);
    }

    #[test]
    fn only_an_old_git_gets_a_notice_at_launch() {
        let notice = old_git_notice(&Ok(Version(2, 39))).unwrap();
        assert!(notice.contains("2.39") && notice.contains("2.41"));
        assert_eq!(old_git_notice(&Ok(Version(2, 41))), None);
        assert_eq!(old_git_notice(&Ok(Version(2, 56))), None);
        assert_eq!(old_git_notice(&Err("Git wasn't found.".to_owned())), None);
    }

    #[test]
    fn reads_names_projects_and_texts() {
        let all = parse(&sample());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "MIT License");
        assert_eq!(all[0].used_by, ["a 1.0.0", "b 2.0.0"]);
        assert_eq!(
            all[0].texts[0].notices,
            [Notice {
                copyrights: vec!["Copyright (c) A".to_owned()],
                used_by: vec!["a 1.0.0".to_owned(), "b 2.0.0".to_owned()],
            }]
        );
        assert_eq!(all[0].texts[0].body, "MIT License");
        assert_eq!(all[0].projects(), "2 projects");
        assert_eq!(all[0].used_by_line(), "a 1.0.0, b 2.0.0");
    }

    #[test]
    fn a_rule_inside_a_text_does_not_split_it() {
        let all = parse(&sample());
        assert_eq!(
            all[1].texts[0].body,
            format!("line one\n{RULE}\nline three")
        );
    }

    #[test]
    fn an_empty_file_has_no_licenses() {
        assert!(parse("").is_empty());
    }

    fn entry(name: &str, used_by: &[&str], text: &str) -> License {
        let used_by: Vec<String> = used_by.iter().map(|s| (*s).to_owned()).collect();
        License {
            name: name.to_owned(),
            texts: vec![text_of(used_by.clone(), text)],
            used_by,
        }
    }

    #[test]
    fn the_same_license_under_different_copyright_lines_is_one() {
        let all = combine(vec![
            entry(
                "MIT License",
                &["a 1"],
                "Copyright (c) A\n\nPermission is granted.",
            ),
            entry("Zlib", &["z 1"], "zlib text"),
            entry(
                "MIT License",
                &["b 2"],
                "Copyright (c) B\n\nPermission is granted.",
            ),
            entry(
                "MIT License",
                &["c 3"],
                "Copyright (c) A\n\nPermission  is\ngranted.\n",
            ),
        ]);
        assert_eq!(all.len(), 2);
        let mit = &all[0];
        assert_eq!(mit.used_by, ["a 1", "b 2", "c 3"]);
        // One text, with both lines and all three projects, each line with its own.
        assert_eq!(mit.texts.len(), 1);
        let notices = &mit.texts[0].notices;
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0].copyrights, ["Copyright (c) A"]);
        assert_eq!(notices[0].used_by, ["a 1", "c 3"]);
        assert_eq!(notices[1].copyrights, ["Copyright (c) B"]);
        assert_eq!(notices[1].used_by, ["b 2"]);
        assert_eq!(mit.texts[0].used_by, ["a 1", "b 2", "c 3"]);
        assert_eq!(mit.texts[0].body, "Permission is granted.");
    }

    #[test]
    fn a_wording_that_really_differs_stays_apart() {
        let all = combine(vec![
            entry("Apache License 2.0", &["a 1"], "Preamble\n\nTerms."),
            entry("Apache License 2.0", &["b 2"], "Terms."),
        ]);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].texts.len(), 2);
    }

    #[test]
    fn every_project_keeps_its_own_copyright_lines() {
        let raw = parse(CRATES);
        let all = combine(raw.clone());
        let mut checked = 0;
        for entry in &raw {
            let license = all.iter().find(|l| l.name == entry.name).unwrap();
            for notice in &entry.texts[0].notices {
                for project in &notice.used_by {
                    for line in &notice.copyrights {
                        // Somewhere under this license, that line is with that project.
                        assert!(
                            license.texts.iter().flat_map(|t| &t.notices).any(|n| {
                                n.copyrights.contains(line) && n.used_by.contains(project)
                            }),
                            "{project} lost “{line}” under {}",
                            entry.name
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 50, "only {checked} lines were checked");
    }

    #[test]
    fn a_license_keeps_its_own_sentences_about_copyright() {
        assert!(is_copyright("Copyright 2023 Jacob Pratt et al."));
        assert!(is_copyright("  Copyright (c) Microsoft Corporation."));
        assert!(is_copyright("Copyright © 2020 A"));
        assert!(is_copyright("© B"));
        // These are the Apache License's own words, not anybody's notice.
        assert!(!is_copyright(
            "copyright notice that is included in or attached to the work"
        ));
        assert!(!is_copyright(
            "copyright license to reproduce, prepare Derivative Works of,"
        ));
        assert!(!is_copyright(
            "(c) You must retain, in the Source form of any Derivative Works"
        ));
        assert!(!is_copyright("Copyright [yyyy] [name of copyright owner]"));
        assert!(!is_copyright("Copyright and related rights"));

        let apache: Vec<&License> = licenses()
            .iter()
            .filter(|l| l.name == "Apache License 2.0")
            .collect();
        assert_eq!(apache.len(), 1);
        // The usual wording, which most crates ship, has these sentences whole. A few crates
        // ship a shorter or reworded one, so this can't be asked of every text.
        let usual = apache[0]
            .texts
            .iter()
            .filter(|t| {
                t.body
                    .contains("copyright notice that is included in or attached to the work")
                    && t.body.contains("(c) You must retain, in the Source form")
            })
            .count();
        assert!(usual > 0, "no Apache text has its own sentences");
        // Some crates left the appendix's template as it is, and the others filled in their
        // own line, which is then a notice with the crate it came from.
        assert!(apache[0].texts.iter().any(|t| {
            t.body
                .contains("Copyright [yyyy] [name of copyright owner]")
        }));
        assert!(
            apache[0]
                .texts
                .iter()
                .any(|t| t.copyright_lines().count() > 0)
        );
    }

    #[test]
    fn the_shipped_file_is_whole() {
        let all = licenses();
        assert!(
            all.iter().all(|l| !l.name.is_empty()
                && !l.used_by.is_empty()
                && !l.texts.is_empty()
                && l.texts.iter().all(|t| !t.body.is_empty())),
            "an entry is missing its name, projects or text"
        );
        // HTML escaping would show as &quot; in a license's name.
        assert!(all.iter().all(|l| !l.name.contains('&')));
        // One row for each name.
        let mut names: Vec<&String> = all.iter().map(|l| &l.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len());
        assert!(all.len() < 25, "{} licenses is too many to read", all.len());
        // Every project is under some text of its license.
        for license in all {
            for project in &license.used_by {
                assert!(
                    license.texts.iter().any(|t| t.used_by.contains(project)),
                    "{project} has no text under {}",
                    license.name
                );
            }
        }
    }

    #[test]
    fn licenses_that_ask_for_nothing_are_left_out() {
        let file = parse(CRATES);
        assert!(
            file.iter().any(|l| !l.must_be_shown()),
            "the generated file has none of them, so this list matches nothing"
        );
        assert!(licenses().iter().all(License::must_be_shown));
    }

    #[test]
    fn gitclients_own_license_comes_first_then_the_font() {
        let all = licenses();
        assert!(all[0].is_own());
        assert!(all[0].name.contains("General Public License"));
        let font = all
            .iter()
            .position(|l| l.used_by == ["Maple Mono"])
            .unwrap();
        assert!(all[font].texts[0].body.contains("SIL OPEN FONT LICENSE"));
        assert!(all.iter().skip(1).all(|l| !l.is_own()));
    }

    #[test]
    fn says_which_build_this_is() {
        assert_eq!(
            version_line(&Build::current(None)),
            format!("gitclient {}", env!("CARGO_PKG_VERSION"))
        );
        let commit = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
        assert!(version_line(&Build::current(Some(commit))).ends_with("(1a2b3c4)"));
    }

    #[test]
    fn counts_the_projects_by_others() {
        assert!(license_sentence().contains("projects by others"));
    }
}
