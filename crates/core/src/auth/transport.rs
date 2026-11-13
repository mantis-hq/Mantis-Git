//! The one place the sign-in talks HTTP, behind a trait so tests can play the server.

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

impl Transport for Web {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(format!("mantisgit/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
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

/// How many answers [`Revalidating`] keeps, and the biggest it keeps. A log is far over the
/// size, and a finished one is kept on disk by the CI cache instead.
const KEPT: usize = 256;
const KEPT_BYTES: usize = 1024 * 1024;

/// Asks again with what it was told last time. A GET that came with an `ETag` is sent with
/// `If-None-Match` next time, and a 304 is answered from memory as the 200 it stands for,
/// so callers never see it. Nothing else changes, and a server that gives no `ETag` is asked
/// in full, as before.
pub struct Revalidating<T> {
    inner: T,
    kept: std::sync::Mutex<std::collections::HashMap<u64, (String, String)>>,
}

impl<T> Revalidating<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            kept: Default::default(),
        }
    }
}

impl<T: Transport> Transport for Revalidating<T> {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        if request.method != "GET" {
            return self.inner.send(request);
        }
        // The token is part of the key, since two accounts may see different things at one
        // address. Only its hash is kept.
        let key = crate::cache::hash64(
            format!("{}\0{}", request.url, request.bearer.unwrap_or_default()).as_bytes(),
        );
        let known = self
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned();
        let reply = self.inner.send(&Request {
            if_none_match: known.as_ref().map(|(etag, _)| etag.as_str()),
            ..*request
        })?;
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match (reply.status, known) {
            (304, Some((etag, body))) => Ok(Reply {
                status: 200,
                body,
                etag: Some(etag),
            }),
            (200, _) => {
                match &reply.etag {
                    Some(etag) if reply.body.len() <= KEPT_BYTES => {
                        if kept.len() >= KEPT {
                            kept.clear();
                        }
                        kept.insert(key, (etag.clone(), reply.body.clone()));
                    }
                    _ => {
                        kept.remove(&key);
                    }
                }
                Ok(reply)
            }
            _ => {
                kept.remove(&key);
                Ok(reply)
            }
        }
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
    use std::sync::Mutex;

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
        let http = Revalidating::new(Etagged::default());
        let first = http.send(&get("a")).unwrap();
        let second = http.send(&get("a")).unwrap();
        assert_eq!((first.status, first.body.as_str()), (200, "page"));
        assert_eq!((second.status, second.body.as_str()), (200, "page"));
        let asked = http.inner.asked.lock().unwrap();
        assert_eq!(asked[0], None);
        assert_eq!(asked[1].as_deref(), Some("\"v1\""));
    }

    #[test]
    fn another_account_is_not_given_the_first_ones_answer() {
        let http = Revalidating::new(Etagged::default());
        http.send(&get("a")).unwrap();
        http.send(&get("b")).unwrap();
        assert_eq!(http.inner.asked.lock().unwrap()[1], None);
    }
}
