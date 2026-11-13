//! The OAuth app flow: authorization code with PKCE, and a redirect to a port on this
//! machine. No client secret is involved, since a desktop app can't keep one.
//!
//! [`begin`] makes the address to open in the browser and starts listening; [`Pending::wait`]
//! blocks until the browser comes back, then trades the code for a token. Both block.

use super::host::Host;
use super::pkce::{self, Pkce};
use super::token::{Token, now};
use super::transport::{Request, Transport};
use crate::ops::Cancel;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// How long the browser has to come back.
const PATIENCE: Duration = Duration::from_secs(5 * 60);

/// The path of the redirect. People register `REDIRECT` with the forge; servers compare the
/// path exactly and ignore only the port, so this must be what is registered.
pub const REDIRECT_PATH: &str = "/callback";

/// What to register as the OAuth app's redirect address.
pub const REDIRECT: &str = "http://127.0.0.1/callback";

pub struct Pending {
    host: Host,
    /// What to open in the browser.
    pub url: String,
    redirect: String,
    state: String,
    pkce: Pkce,
    listener: TcpListener,
}

/// Starts a sign-in on `host`.
pub fn begin(host: &Host) -> Result<Pending, String> {
    if !host.can_sign_in() {
        return Err(format!(
            "Mantis Git has no OAuth app on {} yet. Add its client ID first.",
            host.name()
        ));
    }
    // Port 0 lets the system pick one that is free. Servers match the loopback address
    // by host and ignore the port.
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Couldn't listen for the sign-in: {e}."))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}{REDIRECT_PATH}");
    let pkce = Pkce::new()?;
    let state = pkce::random_text(16)?;
    let query = pkce::build_query(&[
        ("client_id", &host.client_id),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", host.scopes()),
        ("state", &state),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
    ]);
    Ok(Pending {
        url: format!("{}?{query}", host.authorize_url()),
        host: host.clone(),
        redirect,
        state,
        pkce,
        listener,
    })
}

impl Pending {
    /// Waits for the browser, then asks the server for the token. Gives up when `cancel`
    /// is set or after five minutes.
    pub fn wait(self, http: &dyn Transport, cancel: &Cancel) -> Result<Token, String> {
        let code = self.listen(cancel)?;
        let form = [
            ("client_id", self.host.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", self.redirect.as_str()),
            ("code_verifier", self.pkce.verifier.as_str()),
        ];
        token_from(&self.host, http, &form)
    }

    fn listen(&self, cancel: &Cancel) -> Result<String, String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err("Sign-in cancelled.".into());
            }
            if started.elapsed() > PATIENCE {
                return Err("The browser never came back, so sign-in stopped.".into());
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(outcome) = self.answer(stream) {
                        return outcome;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(format!("Listening for the sign-in failed: {e}.")),
            }
        }
    }

    /// Reads one request. `None` means it wasn't the redirect (a favicon, a stray probe),
    /// and listening goes on.
    fn answer(&self, mut stream: TcpStream) -> Option<Result<String, String>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buffer = [0u8; 8192];
        let n = stream.read(&mut buffer).ok()?;
        let request = String::from_utf8_lossy(&buffer[..n]);
        let target = request.lines().next()?.split_whitespace().nth(1)?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != REDIRECT_PATH {
            respond(&mut stream, "404 Not Found", "Not found.");
            return None;
        }
        let outcome = self.check(&pkce::parse_query(query));
        let page = match &outcome {
            Ok(_) => "Signed in. You can close this tab and go back to Mantis Git.",
            Err(_) => "Sign-in failed. Go back to Mantis Git to see why.",
        };
        respond(&mut stream, "200 OK", page);
        Some(outcome)
    }

    fn check(&self, pairs: &[(String, String)]) -> Result<String, String> {
        let get = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        // A redirect that wasn't ours must not be able to finish the sign-in.
        if get("state") != Some(self.state.as_str()) {
            return Err("The sign-in answer didn't match the request, so it was ignored.".into());
        }
        if let Some(error) = get("error") {
            let why = get("error_description").unwrap_or(error);
            return Err(format!("{} said no: {why}.", self.host.name()));
        }
        get("code")
            .filter(|code| !code.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "The sign-in answer had no code.".into())
    }
}

fn respond(stream: &mut TcpStream, status: &str, text: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Mantis Git</title>\
         <body style=\"font:16px monospace;background:#1e1e2e;color:#cdd6f4;padding:3em\">{text}"
    );
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

/// Gets a new token with the refresh token, for servers whose tokens expire (GitLab's do
/// after two hours). The login carries over.
pub fn refresh(host: &Host, http: &dyn Transport, old: &Token) -> Result<Token, String> {
    let refresh = old
        .refresh
        .as_deref()
        .ok_or("The sign-in has expired and can't be renewed. Sign in again.")?;
    let form = [
        ("client_id", host.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
    ];
    let mut token = token_from(host, http, &form)?;
    token.login = old.login.clone();
    token.refresh = token.refresh.or_else(|| old.refresh.clone());
    Ok(token)
}

fn token_from(host: &Host, http: &dyn Transport, form: &[(&str, &str)]) -> Result<Token, String> {
    let url = host.token_url();
    let reply = http.send(&Request {
        method: "POST",
        url: &url,
        bearer: None,
        form: Some(form),
        json: None,
        if_none_match: None,
    })?;
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::Null);
    let text_of = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    // GitHub answers 200 with an `error` in the body.
    if let Some(error) = text_of("error_description").or_else(|| text_of("error")) {
        return Err(format!("{} refused the sign-in: {error}", host.name()));
    }
    let access = text_of("access_token")
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            format!(
                "{} answered with no token (status {}).",
                host.name(),
                reply.status
            )
        })?;
    Ok(Token {
        access,
        refresh: text_of("refresh_token"),
        expires_at: value
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(|seconds| now() + seconds),
        login: None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::transport::fake::Fake;
    use super::*;
    use crate::repo::forge::Forge;

    fn host() -> Host {
        Host::new(Forge::GitLab, "https://gitlab.com", "app-id").unwrap()
    }

    fn query_of(pending: &Pending) -> Vec<(String, String)> {
        pkce::parse_query(pending.url.split_once('?').unwrap().1)
    }

    #[test]
    fn the_address_carries_pkce_and_a_loopback_redirect() {
        let pending = begin(&host()).unwrap();
        assert!(
            pending
                .url
                .starts_with("https://gitlab.com/oauth/authorize?")
        );
        let query = query_of(&pending);
        let get = |k: &str| query.iter().find(|(n, _)| n == k).unwrap().1.clone();
        assert_eq!(get("client_id"), "app-id");
        assert_eq!(get("response_type"), "code");
        assert_eq!(get("code_challenge_method"), "S256");
        assert_eq!(get("code_challenge"), pending.pkce.challenge);
        assert!(get("redirect_uri").starts_with("http://127.0.0.1:"));
        assert!(!pending.url.contains(&pending.pkce.verifier));
    }

    #[test]
    fn the_registered_address_is_the_one_sent_without_its_port() {
        let pending = begin(&host()).unwrap();
        let sent = query_of(&pending)
            .into_iter()
            .find(|(n, _)| n == "redirect_uri")
            .unwrap()
            .1;
        // Servers drop the port of a loopback address and compare the rest exactly.
        let without_port = sent
            .rsplit_once(':')
            .map(|(head, tail)| format!("{head}{}", &tail[tail.find('/').unwrap()..]));
        assert_eq!(without_port.as_deref(), Some(REDIRECT));
    }

    #[test]
    fn a_server_with_no_client_id_is_turned_down_before_anything_opens() {
        let bare = Host::new(Forge::Gitea, "https://git.corp", "").unwrap();
        assert!(begin(&bare).err().unwrap().contains("client ID"));
    }

    #[test]
    fn a_full_round_trip_through_the_listener() {
        let pending = begin(&host()).unwrap();
        let query = query_of(&pending);
        let state = query.iter().find(|(n, _)| n == "state").unwrap().1.clone();
        let verifier = pending.pkce.verifier.clone();
        let port = pending.listener.local_addr().unwrap().port();
        let http = Fake::answering(&[(
            200,
            r#"{"access_token":"tok","refresh_token":"ref","expires_in":7200}"#,
        )]);
        let token = std::thread::scope(|scope| {
            let waiting = scope.spawn(|| pending.wait(&http, &Cancel::new()));
            // A stray request is ignored, a forged state is refused by `check`, and
            // the real redirect finishes it.
            let mut stray = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(stray, "GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
            let mut sink = String::new();
            let _ = stray.read_to_string(&mut sink);
            let mut real = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                real,
                "GET /callback?code=the-code&state={state} HTTP/1.1\r\n\r\n"
            )
            .unwrap();
            let _ = real.read_to_string(&mut sink);
            assert!(sink.contains("Signed in"));
            waiting.join().unwrap()
        })
        .unwrap();
        assert_eq!(token.access, "tok");
        assert_eq!(token.refresh.as_deref(), Some("ref"));
        assert!(token.expires_at.unwrap() > now() + 7000);
        let sent = http.seen.lock().unwrap()[0].clone();
        assert!(sent.starts_with("POST https://gitlab.com/oauth/token"));
        assert!(sent.contains("code=the-code") && sent.contains("grant_type=authorization_code"));
        assert!(sent.contains(&format!("code_verifier={verifier}")));
        assert!(sent.contains(&format!("redirect_uri=http://127.0.0.1:{port}/callback")));
    }

    #[test]
    fn a_redirect_with_the_wrong_state_never_finishes_the_sign_in() {
        let pending = begin(&host()).unwrap();
        let outcome = pending.check(&pkce::parse_query("code=x&state=forged"));
        assert!(outcome.unwrap_err().contains("didn't match"));
        let outcome = pending.check(&pkce::parse_query("code=x"));
        assert!(outcome.is_err());
    }

    #[test]
    fn a_refusal_is_said_in_the_servers_words() {
        let pending = begin(&host()).unwrap();
        let state = pending.state.clone();
        let outcome = pending.check(&pkce::parse_query(&format!(
            "error=access_denied&error_description=You+said+no&state={state}"
        )));
        assert_eq!(outcome.unwrap_err(), "gitlab.com said no: You said no.");
    }

    #[test]
    fn cancelling_stops_the_wait() {
        let pending = begin(&host()).unwrap();
        let cancel = Cancel::new();
        cancel.cancel();
        let outcome = pending.wait(&Fake::default(), &cancel);
        assert_eq!(outcome.unwrap_err(), "Sign-in cancelled.");
    }

    #[test]
    fn github_errors_arrive_with_a_200() {
        let http = Fake::answering(&[(
            200,
            r#"{"error":"bad_verification_code","error_description":"The code is wrong."}"#,
        )]);
        let err = token_from(&host(), &http, &[]).unwrap_err();
        assert!(err.contains("The code is wrong."));
    }

    #[test]
    fn refreshing_keeps_the_login_and_the_old_refresh_token_if_none_comes_back() {
        let old = Token {
            access: "old".into(),
            refresh: Some("r1".into()),
            expires_at: Some(1),
            login: Some("ada".into()),
        };
        let http = Fake::answering(&[(200, r#"{"access_token":"new","expires_in":60}"#)]);
        let new = refresh(&host(), &http, &old).unwrap();
        assert_eq!(
            (new.access.as_str(), new.login.as_deref()),
            ("new", Some("ada"))
        );
        assert_eq!(new.refresh.as_deref(), Some("r1"));
        assert!(http.seen.lock().unwrap()[0].contains("refresh_token=r1"));
        let none = Token {
            refresh: None,
            ..old
        };
        assert!(refresh(&host(), &http, &none).is_err());
    }
}
