//! Parses unified diffs, and rebuilds patches from parts of them.

use crate::cache::{Cacheable, Reader, Writer};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: Arc<str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    /// What git says this hunk is inside, such as a function's first line.
    pub title: String,
    pub lines: Vec<Line>,
    /// The hunk as git wrote it, to put back into a patch unchanged.
    raw: String,
}

/// One file's part of a diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilePatch {
    pub path: String,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    /// The lines before the first hunk.
    header: String,
}

impl Cacheable for FilePatch {
    const FORMAT: u32 = 1;

    fn weight(&self) -> usize {
        let hunks: usize = self
            .hunks
            .iter()
            .map(|hunk| {
                let lines: usize = hunk.lines.iter().map(|line| line.text.len() + 48).sum();
                hunk.title.len() + hunk.raw.len() + lines + 48
            })
            .sum();
        self.path.len() + self.header.len() + hunks + 64
    }

    fn encode(&self, out: &mut Writer) {
        out.str(&self.path);
        out.bool(self.binary);
        out.str(&self.header);
        out.len(self.hunks.len());
        for hunk in &self.hunks {
            out.str(&hunk.title);
            out.str(&hunk.raw);
            out.len(hunk.lines.len());
            for line in &hunk.lines {
                out.u8(match line.kind {
                    LineKind::Context => 0,
                    LineKind::Added => 1,
                    LineKind::Removed => 2,
                });
                out.opt_u32(line.old);
                out.opt_u32(line.new);
                out.str(&line.text);
            }
        }
    }

    fn decode(input: &mut Reader) -> Option<Self> {
        let path = input.str()?.to_owned();
        let binary = input.bool()?;
        let header = input.str()?.to_owned();
        let hunk_count = input.len()?;
        let mut hunks = Vec::new();
        for _ in 0..hunk_count {
            let title = input.str()?.to_owned();
            let raw = input.str()?.to_owned();
            let line_count = input.len()?;
            let mut lines = Vec::new();
            for _ in 0..line_count {
                let kind = match input.u8()? {
                    0 => LineKind::Context,
                    1 => LineKind::Added,
                    2 => LineKind::Removed,
                    _ => return None,
                };
                lines.push(Line {
                    kind,
                    old: input.opt_u32()?,
                    new: input.opt_u32()?,
                    text: input.str()?.into(),
                });
            }
            hunks.push(Hunk { title, lines, raw });
        }
        Some(Self {
            path,
            binary,
            hunks,
            header,
        })
    }
}

impl FilePatch {
    /// A file with no text to show.
    pub fn without_text(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            binary: true,
            ..Self::default()
        }
    }

    /// What happened to the file: `A`, `D` or `M`.
    pub fn status(&self) -> char {
        if self.header.contains("\nnew file mode ") {
            'A'
        } else if self.header.contains("\ndeleted file mode ") {
            'D'
        } else {
            'M'
        }
    }

    /// A patch containing only the chosen hunks, by index. Hunks keep their original
    /// line numbers, which still hold because they apply against the unchanged side.
    pub fn with_hunks(&self, chosen: impl IntoIterator<Item = usize>) -> String {
        let mut patch = self.header.clone();
        for index in chosen {
            patch.push_str(&self.hunks[index].raw);
        }
        patch
    }
}

/// The arguments for a diff that [`parse`] can read whatever the user's settings are.
/// `command` is the git subcommand and any options of its own.
pub fn diff_args<'a>(command: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["-c", "core.quotepath=off"];
    args.extend(command);
    args.extend([
        "--no-color",
        "--no-ext-diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]);
    args
}

/// Parses the output of `git diff` and its relatives, run with [`diff_args`].
pub fn parse(diff: &str) -> Vec<FilePatch> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut numbers = (0, 0);
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") || line.starts_with("diff --cc ") {
            files.push(FilePatch {
                path: path_from_diff_line(line.trim_end()),
                header: line.to_owned(),
                ..FilePatch::default()
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("@@ ") {
            numbers = hunk_start(rest);
            file.hunks.push(Hunk {
                title: rest
                    .split_once(" @@")
                    .map_or("", |(_, title)| title)
                    .trim()
                    .to_owned(),
                lines: Vec::new(),
                raw: line.to_owned(),
            });
            continue;
        }
        let Some(hunk) = file.hunks.last_mut() else {
            file.header.push_str(line);
            let trimmed = line.trim_end();
            if trimmed.starts_with("Binary files ") || trimmed == "GIT binary patch" {
                file.binary = true;
            } else if let Some(path) = trimmed.strip_prefix("+++ b/") {
                file.path = unquote(path);
            } else if let Some(path) = trimmed.strip_prefix("--- a/")
                && file.path.is_empty()
            {
                file.path = unquote(path);
            } else if let Some(path) = trimmed.strip_prefix("rename to ") {
                file.path = unquote(path);
            }
            continue;
        };
        hunk.raw.push_str(line);
        let text = line.strip_suffix('\n').unwrap_or(line);
        let text = text.strip_suffix('\r').unwrap_or(text);
        let (old, new) = numbers;
        let parsed = match text.as_bytes().first() {
            Some(b'+') => {
                numbers.1 += 1;
                Some((LineKind::Added, None, Some(new)))
            }
            Some(b'-') => {
                numbers.0 += 1;
                Some((LineKind::Removed, Some(old), None))
            }
            Some(b' ') | None => {
                numbers = (old + 1, new + 1);
                Some((LineKind::Context, Some(old), Some(new)))
            }
            // "\ No newline at end of file" belongs to the patch, not the display.
            _ => None,
        };
        if let Some((kind, old, new)) = parsed {
            hunk.lines.push(Line {
                kind,
                old,
                new,
                text: text.get(1..).unwrap_or("").into(),
            });
        }
    }
    files
}

/// Reads the line numbers out of the text after a hunk's opening `@@ `.
fn hunk_start(rest: &str) -> (u32, u32) {
    let number = |prefix: char| {
        rest.split_whitespace()
            .find_map(|field| field.strip_prefix(prefix))
            .and_then(|range| range.split(',').next())
            .and_then(|start| start.parse().ok())
            .unwrap_or(0)
    };
    (number('-'), number('+'))
}

/// The path from a `diff --git a/x b/x` line. It is ambiguous with spaces when the two
/// sides differ, so a later `+++` or `rename to` line replaces this where there is one.
fn path_from_diff_line(line: &str) -> String {
    let rest = line.strip_prefix("diff --git ").unwrap_or(line);
    let half = rest.len().saturating_sub(1) / 2;
    match rest
        .get(half + 1..)
        .and_then(|side| side.strip_prefix("b/"))
    {
        Some(path) => unquote(path),
        None => String::new(),
    }
}

/// Undoes git's C-style quoting of unusual paths.
fn unquote(path: &str) -> String {
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return path.split('\t').next().unwrap_or(path).to_owned();
    };
    let inner = inner
        .strip_prefix("b/")
        .or(inner.strip_prefix("a/"))
        .unwrap_or(inner);
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes();
    while let Some(byte) = chars.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match chars.next() {
            Some(b't') => bytes.push(b'\t'),
            Some(b'n') => bytes.push(b'\n'),
            Some(digit @ b'0'..=b'7') => {
                let rest = [chars.next(), chars.next()];
                let octal = rest
                    .into_iter()
                    .flatten()
                    .fold(u32::from(digit - b'0'), |value, d| {
                        value * 8 + u32::from(d.wrapping_sub(b'0'))
                    });
                bytes.push(octal as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_HUNKS: &str = "\
diff --git a/src/app.rs b/src/app.rs
index 1111111..2222222 100644
--- a/src/app.rs
+++ b/src/app.rs
@@ -1,3 +1,3 @@ fn first() {
 keep
-old
+new
 keep
@@ -20,2 +20,3 @@ fn second() {
 keep
+added
 keep
diff --git a/other b/other
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/other
@@ -0,0 +1 @@
+hello
\\ No newline at end of file
";

    #[test]
    fn files_hunks_and_line_numbers_are_read() {
        let files = parse(TWO_HUNKS);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/app.rs");
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[0].title, "fn first() {");

        let lines = &files[0].hunks[0].lines;
        assert_eq!(
            (lines[1].kind, lines[1].old, lines[1].new),
            (LineKind::Removed, Some(2), None)
        );
        assert_eq!(
            (lines[2].kind, lines[2].old, lines[2].new),
            (LineKind::Added, None, Some(2))
        );
        assert_eq!((lines[3].old, lines[3].new), (Some(3), Some(3)));
        assert_eq!(&*lines[2].text, "new");

        let second = &files[0].hunks[1].lines;
        assert_eq!((second[0].old, second[0].new), (Some(20), Some(20)));
    }

    #[test]
    fn a_hunk_starting_at_line_zero_and_a_missing_newline_are_handled() {
        let files = parse(TWO_HUNKS);
        assert_eq!(files[1].path, "other");
        let lines = &files[1].hunks[0].lines;
        assert_eq!(lines.len(), 1, "the no-newline marker isn't a line");
        assert_eq!(lines[0].new, Some(1));
    }

    #[test]
    fn a_patch_keeps_only_the_chosen_hunks_and_stays_byte_exact() {
        let files = parse(TWO_HUNKS);
        let second_only = files[0].with_hunks([1]);
        assert!(second_only.starts_with("diff --git a/src/app.rs"));
        assert!(second_only.contains("fn second()"));
        assert!(!second_only.contains("fn first()"));

        let whole = files[1].with_hunks([0]);
        assert!(whole.ends_with("\\ No newline at end of file\n"));
    }

    #[test]
    fn binary_files_have_no_hunks() {
        let diff = "diff --git a/img.png b/img.png\nindex 1..2 100644\nBinary files a/img.png and b/img.png differ\n";
        let files = parse(diff);
        assert!(files[0].binary);
        assert!(files[0].hunks.is_empty());
        assert_eq!(files[0].path, "img.png");
    }

    #[test]
    fn deletions_and_renames_name_the_right_path() {
        let diff = "\
diff --git a/gone b/gone
deleted file mode 100644
--- a/gone
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/before name b/after name
similarity index 90%
rename from before name
rename to after name
";
        let files = parse(diff);
        assert_eq!(files[0].path, "gone");
        assert_eq!(files[1].path, "after name");
        assert_eq!(files[0].status(), 'D');
        assert_eq!(files[1].status(), 'M');
    }

    #[test]
    fn new_files_say_so() {
        assert_eq!(parse(TWO_HUNKS)[1].status(), 'A');
        assert_eq!(parse(TWO_HUNKS)[0].status(), 'M');
    }

    #[test]
    fn quoted_paths_are_unquoted() {
        assert_eq!(unquote("\"a\\tb.txt\""), "a\tb.txt");
        assert_eq!(unquote("\"b/caf\\303\\251\""), "café");
        assert_eq!(unquote("plain\t"), "plain");
    }

    #[test]
    fn lines_that_look_like_headers_stay_inside_their_hunk() {
        let diff = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1,2 +1,2 @@
---- not a header
+++++ not a header
 keep
";
        let files = parse(diff);
        assert_eq!(files.len(), 1);
        let lines = &files[0].hunks[0].lines;
        assert_eq!(&*lines[0].text, "--- not a header");
        assert_eq!(&*lines[1].text, "++++ not a header");
    }
}
