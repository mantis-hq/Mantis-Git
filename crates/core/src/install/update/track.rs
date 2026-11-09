//! Which releases an update looks at: the full releases, or the nightly pre-release that
//! every push to `main` replaces.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Track {
    #[default]
    Stable,
    Nightly,
}

impl Track {
    pub const ALL: [Self; 2] = [Self::Stable, Self::Nightly];

    /// The tag the nightly release always has, and the name its archives carry.
    pub const NIGHTLY_TAG: &'static str = "nightly";

    /// How it is written in the `state` file.
    pub fn key(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }

    /// Anything but `nightly` is the full releases, so a mangled file is the safe choice.
    pub fn from_key(key: &str) -> Self {
        match key.trim() {
            "nightly" => Self::Nightly,
            _ => Self::Stable,
        }
    }

    /// The chip's words.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stable => "Full releases",
            Self::Nightly => "Nightly builds",
        }
    }

    /// The sentence under the choice.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Stable => "gitclient updates to full releases only.",
            Self::Nightly => {
                "gitclient updates to the nightly build, a pre-release made from every push to main. It may be unfinished."
            }
        }
    }

    /// What the header says after the choice is saved.
    pub fn saved_message(self) -> &'static str {
        match self {
            Self::Stable => "Saved. gitclient will update to full releases.",
            Self::Nightly => "Saved. gitclient will update to nightly builds.",
        }
    }
}
