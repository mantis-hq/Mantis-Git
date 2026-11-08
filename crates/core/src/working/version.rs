//! The version a commit would lead to, read from `v1.2.3` style tags and bumped by the
//! kind of change, following the README's table.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The highest `v1.2.3` style tag.
pub fn latest_version<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<Version> {
    tags.into_iter().filter_map(parse_version).max()
}

fn parse_version(tag: &str) -> Option<Version> {
    let mut parts = tag.strip_prefix('v').unwrap_or(tag).split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = Version(next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// What landing this commit would do to the version, following the README's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bump {
    None,
    Patch(Version),
    Minor(Version),
    Major(Version),
    /// Breaking, but versions before 1.0 only take a minor bump for it.
    BreakingMinor(Version),
}

pub fn bump(kind: &str, breaking: bool, current: Version) -> Bump {
    let Version(major, minor, patch) = current;
    match (kind, breaking) {
        ("feat", true) if major == 0 => Bump::BreakingMinor(Version(0, minor + 1, 0)),
        ("feat", true) => Bump::Major(Version(major + 1, 0, 0)),
        ("feat", false) => Bump::Minor(Version(major, minor + 1, 0)),
        ("fix", _) => Bump::Patch(Version(major, minor, patch + 1)),
        _ => Bump::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_tags_and_junk_is_skipped() {
        let tags = [
            "v0.1.0",
            "nightly",
            "v0.10.0",
            "v0.9.0",
            "1.2",
            "v1.0.0-rc1",
        ];
        assert_eq!(latest_version(tags), Some(Version(0, 10, 0)));
        assert_eq!(latest_version(["nope"]), None);
    }

    #[test]
    fn bumps_follow_the_readme_table() {
        let early = Version(0, 1, 0);
        assert_eq!(bump("fix", false, early), Bump::Patch(Version(0, 1, 1)));
        assert_eq!(bump("feat", false, early), Bump::Minor(Version(0, 2, 0)));
        assert_eq!(
            bump("feat", true, early),
            Bump::BreakingMinor(Version(0, 2, 0))
        );
        assert_eq!(bump("docs", false, early), Bump::None);

        let stable = Version(1, 2, 3);
        assert_eq!(bump("feat", true, stable), Bump::Major(Version(2, 0, 0)));
        assert_eq!(bump("feat", false, stable), Bump::Minor(Version(1, 3, 0)));
    }
}
