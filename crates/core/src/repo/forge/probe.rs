//! Telling which forge a self-hosted server is, by asking it. Each forge answers one URL of
//! its own API without a sign-in, in a way no other does:
//!
//! - Gitea and Forgejo: `/api/v1/version` is JSON with a `version`.
//! - GitHub Enterprise Server: `/api/v3/meta` carries `X-GitHub-Enterprise-Version`.
//! - GitLab: `/api/v4/version` is `401 Unauthorized` (it wants a token) with GitLab's own
//!   header, or JSON with a `revision` when the server lets anyone read it.
//!
//! A server is asked once a session, and only when its name and the repository's setting
//! don't already say. The answer is kept only when the server really answered: one that
//! couldn't be reached (a VPN that is down, a certificate nobody trusts) is asked again.

use super::{Address, Forge};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long each question may take. The three are asked at once.
const PATIENCE: Duration = Duration::from_secs(2);

/// What a server said to one question.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    /// The start of the body.
    pub body: String,
}

impl Reply {
    fn has_header(&self, name: &str) -> bool {
        self.headers.iter().any(|(have, _)| have == name)
    }
}

/// Something that can ask a server a question. `None` is no answer at all.
pub trait Ask: Sync {
    fn get(&self, url: &str) -> Option<Reply>;
}

/// The real thing: an HTTP request.
pub struct Web;

impl Ask for Web {
    fn get(&self, url: &str) -> Option<Reply> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(PATIENCE))
            .http_status_as_error(false)
            .user_agent("mantisgit")
            .build()
            .into();
        let mut response = agent.get(url).call().ok()?;
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().ok()?.to_owned(),
                ))
            })
            .collect();
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(16 * 1024)
            .read_to_string()
            .unwrap_or_default();
        Some(Reply {
            status,
            headers,
            body,
        })
    }
}

/// The question that tells `forge` apart, as a path on the server.
fn question(forge: Forge) -> &'static str {
    match forge {
        Forge::Gitea => "/api/v1/version",
        Forge::GitHub => "/api/v3/meta",
        Forge::GitLab => "/api/v4/version",
    }
}

/// Whether `reply` is how `forge` answers its question.
fn is_answer_of(forge: Forge, reply: &Reply) -> bool {
    match forge {
        Forge::Gitea => reply.status == 200 && reply.body.contains("\"version\""),
        Forge::GitHub => {
            reply.has_header("x-github-enterprise-version")
                || (reply.status == 200 && reply.body.contains("\"installed_version\""))
        }
        Forge::GitLab => {
            reply.has_header("x-gitlab-meta")
                || (reply.status == 401 && reply.body.contains("401 Unauthorized"))
                || (reply.status == 200 && reply.body.contains("\"revision\""))
        }
    }
}

/// What asking came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Is(Forge),
    /// The server answered every question, and none of the answers was a forge's.
    IsNone,
    /// At least one question got no answer, so nothing is known.
    Unreachable,
}

/// Asks the server all three questions at once.
pub fn ask_all(address: &Address, ask: &dyn Ask) -> Outcome {
    let origin = address.origin();
    // Strictest first: a server that fits two takes the one whose answer is least vague.
    let order = [Forge::Gitea, Forge::GitHub, Forge::GitLab];
    let replies: Vec<Option<Reply>> = std::thread::scope(|scope| {
        let asked: Vec<_> = order
            .iter()
            .map(|&forge| {
                let url = format!("{origin}{}", question(forge));
                scope.spawn(move || ask.get(&url))
            })
            .collect();
        asked
            .into_iter()
            .map(|asked| asked.join().ok().flatten())
            .collect()
    });
    for (forge, reply) in order.iter().zip(&replies) {
        if reply
            .as_ref()
            .is_some_and(|reply| is_answer_of(*forge, reply))
        {
            return Outcome::Is(*forge);
        }
    }
    if replies.iter().all(Option::is_some) {
        Outcome::IsNone
    } else {
        Outcome::Unreachable
    }
}

/// Servers that are known not to be a forge Mantis Git speaks of, so that asking them three
/// questions would be pointless.
pub fn worth_asking(address: &Address) -> bool {
    let host = address.host.split(':').next().unwrap_or(&address.host);
    !matches!(
        host,
        "bitbucket.org"
            | "git.sr.ht"
            | "dev.azure.com"
            | "ssh.dev.azure.com"
            | "vs-ssh.visualstudio.com"
            | "source.developers.google.com"
    )
}

type Remembered = Mutex<HashMap<String, Option<Forge>>>;

fn memory() -> &'static Remembered {
    static MEMORY: OnceLock<Remembered> = OnceLock::new();
    MEMORY.get_or_init(Default::default)
}

/// What an earlier question settled this session: `Some(Some(forge))`, or `Some(None)` for
/// a server that is none of them, or `None` when it hasn't been asked or couldn't be reached.
pub fn remembered(address: &Address) -> Option<Option<Forge>> {
    memory().lock().ok()?.get(&address.origin()).copied()
}

/// Which forge the server is, asking it unless that was settled already. This reaches over
/// the network, so call it off the UI thread.
pub fn detect(address: &Address, ask: &dyn Ask) -> Option<Forge> {
    if let Some(known) = remembered(address) {
        return known;
    }
    let found = match ask_all(address, ask) {
        Outcome::Is(forge) => Some(forge),
        Outcome::IsNone => None,
        Outcome::Unreachable => return None,
    };
    if let Ok(mut memory) = memory().lock() {
        memory.insert(address.origin(), found);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::forge::{Hosting, hosting};
    use crate::repo::remote::Remote;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A server that answers from a table of paths, and counts how often it was asked.
    struct Server {
        replies: Vec<(&'static str, Reply)>,
        asked: AtomicUsize,
    }

    impl Server {
        fn new(replies: Vec<(&'static str, Reply)>) -> Self {
            Self {
                replies,
                asked: AtomicUsize::new(0),
            }
        }
    }

    impl Ask for Server {
        fn get(&self, url: &str) -> Option<Reply> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let path = url.split_once("://")?.1.split_once('/')?.1;
            let path = format!("/{path}");
            Some(
                self.replies
                    .iter()
                    .find(|(known, _)| *known == path)
                    .map(|(_, reply)| reply.clone())
                    .unwrap_or(Reply {
                        status: 404,
                        ..Reply::default()
                    }),
            )
        }
    }

    struct Silent(AtomicUsize);

    impl Ask for Silent {
        fn get(&self, _: &str) -> Option<Reply> {
            self.0.fetch_add(1, Ordering::SeqCst);
            None
        }
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &str) -> Reply {
        Reply {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.to_owned(),
        }
    }

    fn address(host: &str) -> Address {
        Address {
            scheme: "https",
            host: host.to_owned(),
            path: "o/r".to_owned(),
        }
    }

    fn outcome(replies: Vec<(&'static str, Reply)>) -> Outcome {
        ask_all(&address("a.test"), &Server::new(replies))
    }

    #[test]
    fn each_forge_is_known_by_its_own_answer() {
        let version = reply(200, &[], r#"{"version":"1.22.0"}"#);
        assert_eq!(
            outcome(vec![("/api/v1/version", version)]),
            Outcome::Is(Forge::Gitea)
        );
        let forgejo = reply(200, &[], r#"{"version":"7.0.0+gitea-1.22.0"}"#);
        assert_eq!(
            outcome(vec![("/api/v1/version", forgejo)]),
            Outcome::Is(Forge::Gitea)
        );
        let locked = reply(401, &[("x-github-enterprise-version", "3.12.0")], "{}");
        assert_eq!(
            outcome(vec![("/api/v3/meta", locked)]),
            Outcome::Is(Forge::GitHub)
        );
        let wants_a_token = reply(401, &[], r#"{"message":"401 Unauthorized"}"#);
        assert_eq!(
            outcome(vec![("/api/v4/version", wants_a_token)]),
            Outcome::Is(Forge::GitLab)
        );
        let open = reply(200, &[], r#"{"version":"16.0","revision":"abc"}"#);
        assert_eq!(
            outcome(vec![("/api/v4/version", open)]),
            Outcome::Is(Forge::GitLab)
        );
    }

    #[test]
    fn a_server_that_is_none_of_them_is_none_of_them() {
        assert_eq!(outcome(vec![]), Outcome::IsNone);
    }

    #[test]
    fn a_server_that_does_not_answer_is_unreachable() {
        let silent = Silent(AtomicUsize::new(0));
        assert_eq!(ask_all(&address("a.test"), &silent), Outcome::Unreachable);
    }

    #[test]
    fn a_settled_server_is_asked_once_and_an_unreachable_one_again() {
        let version = reply(200, &[], r#"{"version":"1"}"#);
        let gitea = Server::new(vec![("/api/v1/version", version)]);
        let home = address("settled.test");
        assert_eq!(remembered(&home), None);
        assert_eq!(detect(&home, &gitea), Some(Forge::Gitea));
        assert_eq!(gitea.asked.load(Ordering::SeqCst), 3);
        assert_eq!(detect(&home, &gitea), Some(Forge::Gitea));
        assert_eq!(
            gitea.asked.load(Ordering::SeqCst),
            3,
            "the second time it was remembered"
        );

        let other = Server::new(vec![]);
        let plain = address("plain.test");
        assert_eq!(detect(&plain, &other), None);
        assert_eq!(
            remembered(&plain),
            Some(None),
            "none of them is also an answer"
        );
        detect(&plain, &other);
        assert_eq!(other.asked.load(Ordering::SeqCst), 3);

        let silent = Silent(AtomicUsize::new(0));
        let away = address("away.test");
        assert_eq!(detect(&away, &silent), None);
        assert_eq!(remembered(&away), None, "nothing was learned");
        detect(&away, &silent);
        assert_eq!(silent.0.load(Ordering::SeqCst), 6, "so it was asked again");
    }

    #[test]
    fn what_was_found_out_is_what_hosting_says_unless_something_else_does() {
        let selfhosted = Remote {
            name: "origin".into(),
            url: "git@code.hosting.test:me/app.git".into(),
            push_url: None,
        };
        let version = reply(200, &[], r#"{"version":"1"}"#);
        let gitea = Server::new(vec![("/api/v1/version", version)]);

        let Some(Hosting::Unknown(found)) = hosting(&selfhosted, None) else {
            panic!("nothing says what it is yet");
        };
        detect(&found, &gitea);
        let Some(Hosting::Known(site)) = hosting(&selfhosted, None) else {
            panic!("it was recognised");
        };
        assert_eq!(site.forge, Forge::Gitea);

        // What was chosen for the repository wins over what the server said.
        let Some(Hosting::Known(site)) = hosting(&selfhosted, Some(Forge::GitLab)) else {
            panic!("chosen");
        };
        assert_eq!(site.forge, Forge::GitLab);
    }

    #[test]
    fn hosts_that_are_known_to_be_something_else_are_not_asked() {
        assert!(!worth_asking(&address("bitbucket.org")));
        assert!(!worth_asking(&address("dev.azure.com")));
        assert!(worth_asking(&address("git.acme.com:8443")));
    }
}
