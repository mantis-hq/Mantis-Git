//! What Mantis Git asks a forge once signed in. GitHub, GitLab and Gitea all take an SSH key
//! as `POST /user/keys` with a title and the key, and name the account in `GET /user`.

use super::host::Host;
use super::transport::{Request, Transport};
use serde_json::{Value, json};

/// The account's login, which is how the sign-in is shown.
pub fn login(host: &Host, http: &dyn Transport, token: &str) -> Result<String, String> {
    let url = host.api("/user");
    let reply = http.send(&Request {
        method: "GET",
        url: &url,
        bearer: Some(token),
        form: None,
        json: None,
    })?;
    if reply.status == 401 {
        return Err(format!("{} didn't accept the token.", host.name()));
    }
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::Null);
    ["login", "username"]
        .iter()
        .find_map(|key| value.get(key)?.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{} didn't say who is signed in.", host.name()))
}

/// Whether `text` is a public key line: a known type, then the key, then a comment.
pub fn is_public_key(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let kind = words.next().unwrap_or_default();
    let known = kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-");
    known && words.next().is_some_and(|key| key.len() > 20)
}

/// Adds a public key to the signed-in account. `title` is what the forge lists it as.
pub fn add_ssh_key(
    host: &Host,
    http: &dyn Transport,
    token: &str,
    title: &str,
    public_key: &str,
) -> Result<String, String> {
    let key = public_key.trim();
    // Only a public key may leave this machine. A private one pasted by mistake stops here.
    if !is_public_key(key) || key.contains("PRIVATE KEY") {
        return Err("That isn't a public key. Pick the file that ends in .pub.".into());
    }
    let url = host.api("/user/keys");
    let body = json!({ "title": title, "key": key }).to_string();
    let reply = http.send(&Request {
        method: "POST",
        url: &url,
        bearer: Some(token),
        form: None,
        json: Some(&body),
    })?;
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::Null);
    let said = ["message", "error"]
        .iter()
        .find_map(|k| value.get(k)?.as_str())
        .unwrap_or_default();
    match reply.status {
        200 | 201 => Ok(format!("Added “{title}” to {}.", host.name())),
        401 => Err(format!(
            "{} didn't accept the sign-in. Sign in again.",
            host.name()
        )),
        403 => Err(format!(
            "{} won't let this sign-in add keys. Sign in again and allow it.",
            host.name()
        )),
        // Every forge words "this key is on an account already" its own way.
        422 | 400 | 409 if said.to_lowercase().contains("already") => {
            Err(format!("{} already has this key.", host.name()))
        }
        status => Err(format!(
            "{} refused the key ({status}){}",
            host.name(),
            if said.is_empty() {
                ".".into()
            } else {
                format!(": {said}")
            }
        )),
    }
}

/// How a key is listed on the forge: who it is and where it came from.
pub fn key_title(user: &str, machine: &str) -> String {
    format!("Mantis Git, {user}@{machine}")
}

#[cfg(test)]
mod tests {
    use super::super::transport::fake::Fake;
    use super::*;
    use crate::repo::forge::Forge;

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me@box";

    fn host() -> Host {
        Host::new(Forge::Gitea, "https://codeberg.org", "id").unwrap()
    }

    #[test]
    fn the_login_is_read_whichever_word_the_forge_uses() {
        let http = Fake::answering(&[(200, r#"{"login":"ada"}"#), (200, r#"{"username":"bob"}"#)]);
        assert_eq!(login(&host(), &http, "t").unwrap(), "ada");
        assert_eq!(login(&host(), &http, "t").unwrap(), "bob");
        assert!(
            http.seen.lock().unwrap()[0]
                .starts_with("GET https://codeberg.org/api/v1/user bearer=t")
        );
        let http = Fake::answering(&[(401, "")]);
        assert!(
            login(&host(), &http, "t")
                .unwrap_err()
                .contains("didn't accept")
        );
    }

    #[test]
    fn a_public_key_is_sent_with_a_title() {
        let http = Fake::answering(&[(201, "{}")]);
        let said = add_ssh_key(&host(), &http, "t", "laptop", KEY).unwrap();
        assert_eq!(said, "Added “laptop” to codeberg.org.");
        let sent = http.seen.lock().unwrap()[0].clone();
        assert!(sent.starts_with("POST https://codeberg.org/api/v1/user/keys bearer=t json="));
        assert!(sent.contains("\"title\":\"laptop\"") && sent.contains("ssh-ed25519"));
    }

    #[test]
    fn a_private_key_never_leaves_the_machine() {
        let http = Fake::default();
        for text in [
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----",
            "ssh-ed25519",
            "hello world",
        ] {
            assert!(add_ssh_key(&host(), &http, "t", "x", text).is_err());
        }
        assert!(http.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn refusals_are_told_apart() {
        let cases = [
            (
                422,
                r#"{"message":"Key is already in use"}"#,
                "already has this key",
            ),
            (403, "{}", "allow it"),
            (401, "{}", "Sign in again"),
            (500, r#"{"message":"boom"}"#, "(500): boom"),
        ];
        for (status, body, expect) in cases {
            let http = Fake::answering(&[(status, body)]);
            let err = add_ssh_key(&host(), &http, "t", "x", KEY).unwrap_err();
            assert!(err.contains(expect), "{err}");
        }
    }

    #[test]
    fn the_title_says_where_the_key_came_from() {
        assert_eq!(key_title("ada", "box"), "Mantis Git, ada@box");
    }
}
