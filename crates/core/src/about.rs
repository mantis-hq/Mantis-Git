//! What the About page says: which build this is, where its source is, and the licenses of
//! everything compiled into it. The licenses travel inside the program, so they can't be
//! separated from a copy that is passed on.
//!
//! The crates' licenses are written by `cargo about` into `THIRD-PARTY-LICENSES.txt` at the
//! repository root (see "Releasing" in the README) and read here. Maple Mono is not a crate,
//! so its license is added by hand. Everything works without a window.

use crate::install::update::Build;
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

/// One license text and everything that is under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct License {
    /// "MIT License".
    pub name: String,
    /// What is under it, as "name version".
    pub used_by: Vec<String>,
    pub text: String,
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
}

/// Licenses that ask nothing of whoever passes the program on: no notice and no text. They
/// are in the generated file, since `cargo about` lists everything, but the page leaves
/// them out.
const NO_NOTICE: [&str; 3] = [
    "BSD Zero Clause License",
    "Creative Commons Zero v1.0 Universal",
    "The Unlicense",
];

impl License {
    /// Whether passing the program on means passing this license's text on with it.
    pub fn must_be_shown(&self) -> bool {
        !NO_NOTICE.contains(&self.name.as_str())
    }
}

/// The licenses that have to be shown for everything in this program: gitclient's own
/// first, then the font, then the crates by license.
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
                text: MAPLE_MONO.trim().to_owned(),
            },
        );
        // Stable, so the crates keep the order they were written in.
        all.sort_by_key(|license| !license.is_own());
        all
    })
}

/// Reads the file `cargo about` writes with `about.hbs`. A line is the start of an entry
/// only when it is the rule followed by a name, an empty line and "Used by:", so that a
/// license text with a rule of its own stays in one piece.
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
            License {
                name,
                used_by: body[..split]
                    .iter()
                    .map(|line| line.trim().to_owned())
                    .collect(),
                text: body[split..].join("\n").trim().to_owned(),
            }
        })
        .collect()
}

/// "gitclient 0.1.0", with the commit when the build knows it ("gitclient 0.1.0 (1a2b3c4)").
pub fn version_line(build: &Build) -> String {
    format!("gitclient {}", build.label())
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
    fn reads_names_projects_and_texts() {
        let all = parse(&sample());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "MIT License");
        assert_eq!(all[0].used_by, ["a 1.0.0", "b 2.0.0"]);
        assert_eq!(all[0].text, "MIT License\n\nCopyright (c) A");
        assert_eq!(all[0].projects(), "2 projects");
        assert_eq!(all[0].used_by_line(), "a 1.0.0, b 2.0.0");
    }

    #[test]
    fn a_rule_inside_a_text_does_not_split_it() {
        let all = parse(&sample());
        assert_eq!(all[1].text, format!("line one\n{RULE}\nline three"));
    }

    #[test]
    fn an_empty_file_has_no_licenses() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn the_shipped_file_is_whole() {
        let all = licenses();
        assert!(all.len() > 100, "only {} licenses", all.len());
        assert!(
            all.iter()
                .all(|l| !l.name.is_empty() && !l.used_by.is_empty() && !l.text.is_empty()),
            "an entry is missing its name, projects or text"
        );
        // HTML escaping would show as &quot; in a license's name.
        assert!(all.iter().all(|l| !l.name.contains('&')));
    }

    #[test]
    fn licenses_that_ask_for_nothing_are_left_out() {
        let file = parse(CRATES);
        assert!(
            file.iter().any(|l| !l.must_be_shown()),
            "the generated file has none of them, so this list matches nothing"
        );
        assert!(licenses().iter().all(License::must_be_shown));
        assert!(licenses().len() < file.len());
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
        assert!(all[font].text.contains("SIL OPEN FONT LICENSE"));
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
