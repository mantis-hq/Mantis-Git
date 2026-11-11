//! Where releases are published. Everything that differs between forges is here: the
//! address of the release API, which downloads are trusted, and what the pages are called.
//! The rest of the updater reads one JSON shape that Gitea and GitHub share (`tag_name`,
//! `draft`, `body`, and `assets` with a `name` and `browser_download_url`).
//!
//! To update from a Gitea server instead, change [`HOST`] to `Host::gitea(site, "owner/name")`.

/// The forge this copy updates from.
pub const HOST: Host = Host::github("mantis-hq/Mantis-Git");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Gitea,
    GitHub,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Host {
    kind: Kind,
    /// `https://` and the host, without a closing slash.
    site: &'static str,
    /// `owner/name`.
    repository: &'static str,
}

impl Host {
    pub const fn gitea(site: &'static str, repository: &'static str) -> Self {
        Self {
            kind: Kind::Gitea,
            site,
            repository,
        }
    }

    pub const fn github(repository: &'static str) -> Self {
        Self {
            kind: Kind::GitHub,
            site: "https://github.com",
            repository,
        }
    }

    /// What to call the host in a sentence.
    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::GitHub => "GitHub",
            Kind::Gitea => self.domain(),
        }
    }

    fn domain(&self) -> &'static str {
        self.site.strip_prefix("https://").unwrap_or(self.site)
    }

    fn api(&self) -> String {
        match self.kind {
            Kind::Gitea => format!("{}/api/v1/repos/{}", self.site, self.repository),
            Kind::GitHub => format!("https://api.github.com/repos/{}", self.repository),
        }
    }

    /// The newest releases, newest first, drafts left out where the host can say so.
    pub fn releases_url(&self) -> String {
        match self.kind {
            Kind::Gitea => format!("{}/releases?draft=false&limit=50", self.api()),
            Kind::GitHub => format!("{}/releases?per_page=50", self.api()),
        }
    }

    /// The one release made at `tag`.
    pub fn tag_url(&self, tag: &str) -> String {
        format!("{}/releases/tags/{tag}", self.api())
    }

    /// Where a person reads about the release at `tag`, without the scheme.
    pub fn page(&self, tag: &str) -> String {
        format!("{}/{}/releases/tag/{tag}", self.domain(), self.repository)
    }

    /// Downloads only over HTTPS from the host itself, whatever a release says.
    pub fn trusts(&self, url: &str) -> bool {
        let Some(rest) = url.strip_prefix("https://") else {
            return false;
        };
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        match self.kind {
            Kind::Gitea => host == self.domain(),
            Kind::GitHub => {
                host == "github.com"
                    || host.ends_with(".github.com")
                    || host.ends_with(".githubusercontent.com")
            }
        }
    }
}
