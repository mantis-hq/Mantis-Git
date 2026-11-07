//! Small pieces of wording shared by the git layer and the views.

use std::fmt::Display;

/// A number and its noun: "1 commit", "2 commits".
pub fn count(number: impl Display, noun: &str) -> String {
    let number = number.to_string();
    let plural = if number == "1" { "" } else { "s" };
    format!("{number} {noun}{plural}")
}

/// A commit's hash, short enough to read.
/// `↑2 ↓1`: commits to push and commits to pull. Empty when both are zero.
pub fn ahead_behind(ahead: u32, behind: u32) -> String {
    match (ahead, behind) {
        (0, 0) => String::new(),
        (ahead, 0) => format!("↑{ahead}"),
        (0, behind) => format!("↓{behind}"),
        (ahead, behind) => format!("↑{ahead} ↓{behind}"),
    }
}

pub fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// The folder and file name of a path, to draw the folder quieter.
pub fn split_path(path: &str) -> (&str, &str) {
    let split = path.rfind('/').map_or(0, |slash| slash + 1);
    path.split_at(split)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nouns_are_plural_except_for_one() {
        assert_eq!(count(0_u32, "file"), "0 files");
        assert_eq!(count(1_usize, "file"), "1 file");
        assert_eq!(count(2_u32, "file"), "2 files");
        assert_eq!(count(11_u32, "file"), "11 files");
    }

    #[test]
    fn short_hashes_cope_with_anything_shorter() {
        assert_eq!(short_oid("0123456789"), "0123456");
        assert_eq!(short_oid("abc"), "abc");
    }

    #[test]
    fn paths_split_after_their_last_folder() {
        assert_eq!(split_path("src/view/app.rs"), ("src/view/", "app.rs"));
        assert_eq!(split_path("app.rs"), ("", "app.rs"));
    }
}

#[cfg(test)]
mod ahead_behind_tests {
    use super::ahead_behind;

    #[test]
    fn only_the_sides_with_commits_show() {
        assert_eq!(ahead_behind(0, 0), "");
        assert_eq!(ahead_behind(2, 0), "↑2");
        assert_eq!(ahead_behind(0, 3), "↓3");
        assert_eq!(ahead_behind(2, 3), "↑2 ↓3");
    }
}
