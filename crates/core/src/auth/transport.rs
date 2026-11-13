//! The one place the sign-in talks HTTP, behind a trait so tests can play the server.

use std::time::Duration;

pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub bearer: Option<&'a str>,
    /// Sent as a form (`application/x-www-form-urlencoded`).
    pub form: Option<&'a [(&'a str, &'a str)]>,
    /// Sent as JSON.
    pub json: Option<&'a str>,
}

pub struct Reply {
    pub status: u16,
    pub body: String,
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
        let body = response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .unwrap_or_default();
        Ok(Reply { status, body })
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
            Ok(Reply { status, body })
        }
    }
}
