//! The servers a person can sign in to: where each forge keeps its OAuth endpoints and API,
//! which OAuth app (client ID) Mantis Git uses there, and what it asks to be allowed to do.

use crate::repo::forge::Forge;

/// A forge server, and the OAuth app registered on it for Mantis Git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub forge: Forge,
    /// `https://github.com`: scheme and host, lowercase, with no trailing slash.
    pub origin: String,
    /// The OAuth app's public ID. Empty when no app is registered for this server.
    pub client_id: String,
    /// Whether the app is confidential: the server wants its client secret with every
    /// token request. The secret itself is kept in the system keychain, never here.
    pub confidential: bool,
}

/// The public servers. Their client IDs are set when Mantis Git is built, from the named
/// environment variable, since each is an app registered by whoever ships the build.
const BUILT_IN: [(Forge, &str, Option<&str>); 3] = [
    (
        Forge::GitHub,
        "https://github.com",
        option_env!("MANTISGIT_GITHUB_CLIENT_ID"),
    ),
    (
        Forge::GitLab,
        "https://gitlab.com",
        option_env!("MANTISGIT_GITLAB_CLIENT_ID"),
    ),
    (
        Forge::Gitea,
        "https://codeberg.org",
        option_env!("MANTISGIT_CODEBERG_CLIENT_ID"),
    ),
];

/// Cuts an address down to `scheme://host[:port]` in lowercase, which is how hosts are
/// matched. `None` for anything that isn't an `http` or `https` address.
pub fn origin_of(address: &str) -> Option<String> {
    let address = address.trim();
    let (scheme, rest) = address.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.to_ascii_lowercase();
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

impl Host {
    pub fn new(forge: Forge, origin: &str, client_id: &str) -> Option<Self> {
        Some(Self {
            forge,
            origin: origin_of(origin)?,
            client_id: client_id.trim().to_owned(),
            confidential: false,
        })
    }

    /// The same server, with an app that does or doesn't need its client secret.
    pub fn with_confidential(self, confidential: bool) -> Self {
        Self {
            confidential,
            ..self
        }
    }

    /// Whether this is one of the public servers, which are always listed.
    pub fn is_public(&self) -> bool {
        BUILT_IN.iter().any(|(_, origin, _)| *origin == self.origin)
    }

    /// The public servers, with the client IDs this build was made with.
    pub fn built_in() -> Vec<Self> {
        BUILT_IN
            .iter()
            .map(|(forge, origin, id)| Self {
                forge: *forge,
                origin: (*origin).to_owned(),
                client_id: id.unwrap_or_default().to_owned(),
                confidential: false,
            })
            .collect()
    }

    /// What to call it: the host's name, such as `github.com`.
    pub fn name(&self) -> &str {
        self.origin.split_once("://").map_or(&self.origin, |p| p.1)
    }

    /// Whether Mantis Git has an OAuth app on this server to sign in with.
    pub fn can_sign_in(&self) -> bool {
        !self.client_id.is_empty()
    }

    pub fn authorize_url(&self) -> String {
        let path = match self.forge {
            Forge::GitHub => "/login/oauth/authorize",
            Forge::GitLab => "/oauth/authorize",
            Forge::Gitea => "/login/oauth/authorize",
        };
        format!("{}{path}", self.origin)
    }

    pub fn token_url(&self) -> String {
        let path = match self.forge {
            Forge::GitHub => "/login/oauth/access_token",
            Forge::GitLab => "/oauth/token",
            Forge::Gitea => "/login/oauth/access_token",
        };
        format!("{}{path}", self.origin)
    }

    /// An API address. GitHub.com serves its API from its own host.
    pub fn api(&self, path: &str) -> String {
        let base = match self.forge {
            Forge::GitHub if self.origin == "https://github.com" => {
                "https://api.github.com".to_owned()
            }
            Forge::GitHub => format!("{}/api/v3", self.origin),
            Forge::GitLab => format!("{}/api/v4", self.origin),
            Forge::Gitea => format!("{}/api/v1", self.origin),
        };
        format!("{base}{path}")
    }

    /// What the app asks to be allowed: read the account, add SSH keys, and read what
    /// runs on repositories (CI), which GitHub only gives with `repo`.
    pub fn scopes(&self) -> &'static str {
        match self.forge {
            Forge::GitHub => "repo write:public_key read:user",
            Forge::GitLab => "api read_user",
            Forge::Gitea => "write:user write:repository",
        }
    }

    /// How to register an OAuth app for Mantis Git on a server of this forge, named
    /// `server`: where to go, what to fill in, and what to bring back.
    pub fn setup_steps(forge: Forge, server: &str) -> String {
        match forge {
            Forge::GitHub => format!(
                "On {server}, open Settings → Developer settings → OAuth Apps → New OAuth \
                 App, and set its callback URL to the redirect address below. Then generate a \
                 client secret and paste it with the client ID: GitHub asks for it even though \
                 Mantis Git uses PKCE."
            ),
            Forge::GitLab => format!(
                "On {server}, open Preferences → Applications → Add new application (or Admin \
                 → Applications, for everyone on the server). Set its redirect URI to the \
                 address below, tick the scopes api and read_user, and clear Confidential, so \
                 no secret is needed. If you leave Confidential ticked, paste the secret too."
            ),
            Forge::Gitea => format!(
                "On {server}, open Settings → Applications → Manage OAuth2 applications, and \
                 add one with the redirect URI below. Clear \"Confidential client\": Gitea and \
                 Forgejo let only a public app come back on the port Mantis Git listens on."
            ),
        }
    }

    /// The name git is given with the token. Servers read the token and ignore this, but
    /// GitLab insists on `oauth2` and the others accept it.
    pub fn username(&self) -> &'static str {
        "oauth2"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_drops_everything_but_scheme_and_host() {
        assert_eq!(
            origin_of("https://Me@GitHub.com/owner/repo.git").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            origin_of("http://git.lan:3000/x").as_deref(),
            Some("http://git.lan:3000")
        );
        assert_eq!(origin_of("git@github.com:owner/repo"), None);
        assert_eq!(origin_of("ssh://git@github.com/x"), None);
        assert_eq!(origin_of("/home/me/repo"), None);
    }

    #[test]
    fn each_forge_has_its_own_endpoints() {
        let github = Host::new(Forge::GitHub, "https://github.com", "id").unwrap();
        assert_eq!(
            github.token_url(),
            "https://github.com/login/oauth/access_token"
        );
        assert_eq!(github.api("/user"), "https://api.github.com/user");
        let ghe = Host::new(Forge::GitHub, "https://ghe.corp", "id").unwrap();
        assert_eq!(ghe.api("/user"), "https://ghe.corp/api/v3/user");
        let gitlab = Host::new(Forge::GitLab, "https://gitlab.com/", "id").unwrap();
        assert_eq!(gitlab.authorize_url(), "https://gitlab.com/oauth/authorize");
        assert_eq!(gitlab.api("/user"), "https://gitlab.com/api/v4/user");
        let gitea = Host::new(Forge::Gitea, "https://codeberg.org", "id").unwrap();
        assert_eq!(gitea.api("/user"), "https://codeberg.org/api/v1/user");
    }

    #[test]
    fn a_host_without_a_client_id_cannot_sign_in() {
        assert!(
            !Host::new(Forge::GitLab, "https://git.corp", " ")
                .unwrap()
                .can_sign_in()
        );
        assert!(
            Host::new(Forge::GitLab, "https://git.corp", "abc")
                .unwrap()
                .can_sign_in()
        );
    }

    #[test]
    fn the_public_servers_are_listed() {
        let names: Vec<_> = Host::built_in()
            .iter()
            .map(|h| h.name().to_owned())
            .collect();
        assert_eq!(names, ["github.com", "gitlab.com", "codeberg.org"]);
    }
}
