//! The one place the sign-in talks HTTP, behind a trait so tests can play the server.

use crate::cache::{Cache, Cacheable, Reader, Writer, hash64};
use std::time::Duration;

#[derive(Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub bearer: Option<&'a str>,
    /// Sent as a form (`application/x-www-form-urlencoded`).
    pub form: Option<&'a [(&'a str, &'a str)]>,
    /// Sent as JSON.
    pub json: Option<&'a str>,
    /// An `ETag` from an earlier answer, sent as `If-None-Match`. A server that still has
    /// that version answers 304 with no body, which costs it no rate limit.
    pub if_none_match: Option<&'a str>,
}

pub struct Reply {
    pub status: u16,
    pub body: String,
    /// The `ETag` the server gave this answer, if any.
    pub etag: Option<String>,
}

pub trait Transport: Send + Sync {
    fn send(&self, request: &Request) -> Result<Reply, String>;
}

/// The real thing.
pub struct Web;

/// One agent for every request, so a server that was just asked is asked again over the
/// connection that is still open: no new TCP and TLS handshake, which is a few round trips
/// each. A job's log and the jobs of a commit are the case, a request after another to one
/// server.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::LazyLock<ureq::Agent> = std::sync::LazyLock::new(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(format!("mantisgit/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    });
    &AGENT
}

impl Transport for Web {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        let agent = agent();
        let url = request.url;
        let with_headers = |builder: ureq::RequestBuilder<ureq::typestate::WithBody>| {
            let builder = builder.header("Accept", "application/json");
            match request.bearer {
                Some(token) => builder.header("Authorization", format!("Bearer {token}")),
                None => builder,
            }
        };
        let result = match request.method {
            "GET" => {
                let builder = agent.get(url).header("Accept", "application/json");
                let builder = match request.if_none_match {
                    Some(etag) => builder.header("If-None-Match", etag),
                    None => builder,
                };
                match request.bearer {
                    Some(token) => builder.header("Authorization", format!("Bearer {token}")),
                    None => builder,
                }
                .call()
            }
            _ => {
                let builder = with_headers(agent.post(url));
                if let Some(form) = request.form {
                    let body = super::pkce::build_query(form);
                    builder
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .send(body)
                } else {
                    builder
                        .header("Content-Type", "application/json")
                        .send(request.json.unwrap_or("{}"))
                }
            }
        };
        let mut response = result.map_err(|e| format!("Couldn't reach {url}: {e}."))?;
        let status = response.status().as_u16();
        let etag = response
            .headers()
            .get("ETag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .unwrap_or_default();
        Ok(Reply { status, body, etag })
    }
}

/// The biggest answer kept. A job's log is the largest thing read, and the body limit of a
/// read is 16 MB.
const KEPT_BYTES: usize = 8 * 1024 * 1024;

/// One answer to a GET as the cache keeps it, in memory and on disk: what was asked, the
/// `ETag` the server gave it, and the body. Running or finished makes no difference here.
pub struct Page {
    url: String,
    etag: Option<String>,
    body: String,
}

impl Cacheable for Page {
    const FORMAT: u32 = 1;

    fn weight(&self) -> usize {
        self.url.len() + self.body.len() + self.etag.as_ref().map_or(0, String::len)
    }

    fn encode(&self, out: &mut Writer) {
        out.str(&self.url);
        out.opt_str(self.etag.as_deref());
        out.str(&self.body);
    }

    fn decode(input: &mut Reader) -> Option<Self> {
        Some(Self {
            url: input.str()?.to_owned(),
            etag: input.opt_str()?.map(str::to_owned),
            body: input.str()?.to_owned(),
        })
    }
}

/// The page kept for an address. The address is in the page too, so a clash of hashes is a
/// miss and not somebody else's answer.
fn kept(store: &Cache<Page>, url: &str) -> Option<std::sync::Arc<Page>> {
    store
        .get(&format!("page/{:016x}", hash64(url.as_bytes())))
        .filter(|page| page.url == url)
}

/// Asks again with what it was told last time. Every 200 to a GET is kept with its `ETag`,
/// and the next ask for that address sends it as `If-None-Match`; a 304 is answered from the
/// kept body as the 200 it stands for, so callers never see it. A server that gives no
/// `ETag` is asked in full, as before.
///
/// The kept pages outlive the run, so the first ask of the next one can be a 304 too, and
/// [`Stored`] can answer from them with no ask at all.
///
/// A page is kept by its address alone, not by who asked. That is right for CI, where one
/// account is signed in to a server, and it is cleared with the rest of the cache.
pub struct Revalidating<T> {
    inner: T,
    store: Cache<Page>,
}

impl<T> Revalidating<T> {
    pub fn new(inner: T, store: Cache<Page>) -> Self {
        Self { inner, store }
    }
}

impl<T: Transport> Transport for Revalidating<T> {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        if request.method != "GET" {
            return self.inner.send(request);
        }
        let known = kept(&self.store, request.url);
        let reply = self.inner.send(&Request {
            if_none_match: known.as_ref().and_then(|page| page.etag.as_deref()),
            ..*request
        })?;
        match (reply.status, known) {
            (304, Some(page)) => Ok(Reply {
                status: 200,
                body: page.body.clone(),
                etag: page.etag.clone(),
            }),
            (200, known) => {
                let same = known.is_some_and(|page| page.body == reply.body);
                if !same && reply.body.len() <= KEPT_BYTES {
                    let key = format!("page/{:016x}", hash64(request.url.as_bytes()));
                    self.store.insert(
                        &key,
                        Page {
                            url: request.url.to_owned(),
                            etag: reply.etag.clone(),
                            body: reply.body.clone(),
                        },
                    );
                }
                Ok(reply)
            }
            _ => Ok(reply),
        }
    }
}

/// Answers only from what [`Revalidating`] kept, and never asks anyone. A page it has never
/// had is an error, so a read built on it is whole or fails, and the interface can draw what
/// it has the moment it opens.
pub struct Stored(Cache<Page>);

impl Stored {
    pub fn new(store: Cache<Page>) -> Self {
        Self(store)
    }
}

impl Transport for Stored {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        kept(&self.0, request.url)
            .map(|page| Reply {
                status: 200,
                body: page.body.clone(),
                etag: page.etag.clone(),
            })
            .ok_or_else(|| "Not read yet.".to_owned())
    }
}

/// A server for tests: answers from a script and remembers what it was asked.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Fake {
        replies: Mutex<Vec<(u16, String)>>,
        pub seen: Mutex<Vec<String>>,
    }

    impl Fake {
        pub fn answering(replies: &[(u16, &str)]) -> Self {
            let mut replies: Vec<_> = replies.iter().map(|(s, b)| (*s, (*b).to_owned())).collect();
            replies.reverse();
            Self {
                replies: Mutex::new(replies),
                seen: Mutex::default(),
            }
        }
    }

    impl Transport for Fake {
        fn send(&self, request: &Request) -> Result<Reply, String> {
            let mut line = format!("{} {}", request.method, request.url);
            if let Some(token) = request.bearer {
                line += &format!(" bearer={token}");
            }
            for (key, value) in request.form.unwrap_or_default() {
                line += &format!(" {key}={value}");
            }
            if let Some(json) = request.json {
                line += &format!(" json={json}");
            }
            self.seen.lock().unwrap().push(line);
            let (status, body) = self.replies.lock().unwrap().pop().ok_or("no reply left")?;
            Ok(Reply {
                status,
                body,
                etag: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Config, DiskSettings};
    use std::sync::Mutex;

    const CONFIG: Config = Config {
        name: "pages",
        label: "Pages",
        memory_bytes: 1024 * 1024,
    };

    /// A server that has one version of one page and says so with an `ETag`.
    #[derive(Default)]
    struct Etagged {
        asked: Mutex<Vec<Option<String>>>,
    }

    impl Transport for Etagged {
        fn send(&self, request: &Request) -> Result<Reply, String> {
            self.asked
                .lock()
                .unwrap()
                .push(request.if_none_match.map(str::to_owned));
            Ok(match request.if_none_match {
                Some("\"v1\"") => Reply {
                    status: 304,
                    body: String::new(),
                    etag: None,
                },
                _ => Reply {
                    status: 200,
                    body: "page".into(),
                    etag: Some("\"v1\"".into()),
                },
            })
        }
    }

    fn get(token: &str) -> Request<'_> {
        Request {
            method: "GET",
            url: "https://x/y",
            bearer: Some(token),
            form: None,
            json: None,
            if_none_match: None,
        }
    }

    #[test]
    fn a_page_that_has_not_changed_is_answered_from_memory() {
        let http = Revalidating::new(Etagged::default(), Cache::memory_only(CONFIG));
        let first = http.send(&get("a")).unwrap();
        let second = http.send(&get("a")).unwrap();
        assert_eq!((first.status, first.body.as_str()), (200, "page"));
        assert_eq!((second.status, second.body.as_str()), (200, "page"));
        let asked = http.inner.asked.lock().unwrap();
        assert_eq!(asked[0], None);
        assert_eq!(asked[1].as_deref(), Some("\"v1\""));
    }

    #[test]
    fn the_first_ask_of_the_next_run_is_a_304_too_and_stored_asks_nobody() {
        let root = tempfile::tempdir().unwrap();
        let disk = || Cache::new(CONFIG, root.path().to_owned(), DiskSettings::default());

        let store = disk();
        let first = Revalidating::new(Etagged::default(), store.clone());
        first.send(&get("a")).unwrap();
        store.flush();

        let store = disk();
        let stored = Stored::new(store.clone());
        assert_eq!(stored.send(&get("a")).unwrap().body, "page");
        let mut other = get("a");
        other.url = "https://x/never-read";
        assert!(stored.send(&other).is_err());

        let next = Revalidating::new(Etagged::default(), store);
        assert_eq!(next.send(&get("a")).unwrap().body, "page");
        assert_eq!(
            next.inner.asked.lock().unwrap()[0].as_deref(),
            Some("\"v1\""),
            "the ETag came from the disk"
        );
    }
}
