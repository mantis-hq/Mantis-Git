//! HTTPS for git. When git asks for a login (`Username for 'https://github.com'`), this
//! answers from the signed-in account's token, so a fetch, push or clone over HTTPS just
//! works. Git asks through the askpass program (see `askpass`), which tries this before
//! showing a popup. A prompt for a server nobody signed in to is left to the popup.

use super::host::{Host, origin_of};
use super::token::Store;
use super::transport::Transport;

/// What git is asking for.
#[derive(Debug, PartialEq, Eq)]
pub enum Question {
    Username,
    Password,
}

/// Reads git's prompt: the kind of question and which server it is about.
pub fn parse(prompt: &str) -> Option<(Question, String)> {
    let prompt = prompt.trim();
    let question = if prompt.starts_with("Username for") {
        Question::Username
    } else if prompt.starts_with("Password for") {
        Question::Password
    } else {
        return None;
    };
    // Git writes `Password for 'https://user@host': `, and the user part comes from the
    // remote's address, so it can hold a quote (`https://github.com'@evil.example/x`). The
    // address is what lies between the first quote and the last, never the first two,
    // or a hostile remote could be answered with a token that belongs to another server.
    let start = prompt.find('\'')? + 1;
    let end = prompt.rfind('\'')?;
    let quoted = prompt.get(start..end)?;
    Some((question, origin_of(quoted)?))
}

/// The answer for `prompt` if an account is signed in on that server. A token that has
/// expired is renewed first, and kept.
pub fn answer(
    prompt: &str,
    hosts: &[Host],
    store: &dyn Store,
    http: &dyn Transport,
) -> Option<String> {
    let (question, origin) = parse(prompt)?;
    let host = hosts.iter().find(|host| host.origin == origin)?;
    let token = super::fresh_token(host, store, http).ok()?;
    Some(match question {
        Question::Username => host.username().to_owned(),
        Question::Password => token.access,
    })
}

#[cfg(test)]
mod tests {
    use super::super::token::{Memory, Token, now};
    use super::super::transport::fake::Fake;
    use super::*;
    use crate::repo::forge::Forge;

    fn hosts() -> Vec<Host> {
        vec![Host::new(Forge::GitHub, "https://github.com", "id").unwrap()]
    }

    fn signed_in() -> Memory {
        let store = Memory::default();
        let token = Token {
            access: "gho_secret".into(),
            refresh: None,
            expires_at: None,
            login: Some("ada".into()),
        };
        store.set("https://github.com", &token).unwrap();
        store
    }

    #[test]
    fn gits_prompts_are_read() {
        assert_eq!(
            parse("Username for 'https://github.com': "),
            Some((Question::Username, "https://github.com".into()))
        );
        assert_eq!(
            parse("Password for 'https://oauth2@GitHub.com': "),
            Some((Question::Password, "https://github.com".into()))
        );
        assert_eq!(parse("Enter passphrase for key '/home/me/.ssh/id': "), None);
    }

    #[test]
    fn a_quote_in_a_remotes_user_name_cannot_borrow_another_servers_token() {
        // The remote is evil.example; its user part is `github.com'`, so the first two
        // quotes of git's prompt enclose `https://github.com`.
        let prompt = "Password for 'https://github.com'@evil.example': ";
        assert_eq!(
            parse(prompt),
            Some((Question::Password, "https://evil.example".into()))
        );
        let (store, http) = (signed_in(), Fake::default());
        assert_eq!(answer(prompt, &hosts(), &store, &http), None);
    }

    #[test]
    fn git_gets_the_token_as_the_password() {
        let (store, http) = (signed_in(), Fake::default());
        let ask = |prompt| answer(prompt, &hosts(), &store, &http);
        assert_eq!(
            ask("Username for 'https://github.com': ").as_deref(),
            Some("oauth2")
        );
        assert_eq!(
            ask("Password for 'https://oauth2@github.com': ").as_deref(),
            Some("gho_secret")
        );
    }

    #[test]
    fn a_server_nobody_signed_in_to_is_left_to_the_popup() {
        let (store, http) = (signed_in(), Fake::default());
        assert_eq!(
            answer(
                "Password for 'https://gitlab.com': ",
                &hosts(),
                &store,
                &http
            ),
            None
        );
        let empty = Memory::default();
        assert_eq!(
            answer(
                "Password for 'https://github.com': ",
                &hosts(),
                &empty,
                &http
            ),
            None
        );
    }

    #[test]
    fn an_expired_token_is_renewed_before_git_sees_it() {
        let store = Memory::default();
        let host = Host::new(Forge::GitLab, "https://gitlab.com", "id").unwrap();
        let old = Token {
            access: "old".into(),
            refresh: Some("r".into()),
            expires_at: Some(now() - 10),
            login: Some("ada".into()),
        };
        store.set("https://gitlab.com", &old).unwrap();
        let http = Fake::answering(&[(
            200,
            r#"{"access_token":"new","refresh_token":"r2","expires_in":7200}"#,
        )]);
        let got = answer(
            "Password for 'https://gitlab.com': ",
            &[host],
            &store,
            &http,
        );
        assert_eq!(got.as_deref(), Some("new"));
        assert_eq!(store.get("https://gitlab.com").unwrap().access, "new");
    }

    #[test]
    fn an_expired_token_that_cannot_be_renewed_is_not_handed_over() {
        let store = Memory::default();
        let host = Host::new(Forge::GitLab, "https://gitlab.com", "id").unwrap();
        let old = Token {
            access: "old".into(),
            refresh: None,
            expires_at: Some(now() - 10),
            login: None,
        };
        store.set("https://gitlab.com", &old).unwrap();
        let got = answer(
            "Password for 'https://gitlab.com': ",
            &[host],
            &store,
            &Fake::default(),
        );
        assert_eq!(got, None);
    }
}
