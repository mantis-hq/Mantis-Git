//! Signing in to GitHub, GitLab and Gitea with the OAuth app flow, so Mantis Git can use
//! HTTPS remotes without asking for a password, and add SSH keys to the account.
//!
//! The flow is authorization code with PKCE and a redirect to this machine (`flow`). The
//! token is kept in the system keychain (`token`). Git's HTTPS questions are answered from
//! it (`credentials`) and keys are added through the forge's API (`api`).
//!
//! Everything here blocks, so call it off the UI thread.

pub mod api;
pub mod credentials;
pub mod flow;
pub mod host;
pub mod pkce;
pub mod token;
pub mod transport;

pub use host::Host;
pub use token::Token;

use crate::ops::Cancel;
use crate::repo::forge::Forge;
use std::path::PathBuf;
use token::{Keychain, Store, now};
use transport::{Transport, Web};

/// Servers beyond the public ones, which people add with their own OAuth app's ID. Kept
/// in `hosts` beside the `state` file, one line each: `forge origin client_id`.
fn registry_file() -> Option<PathBuf> {
    crate::settings::state::config_dir().map(|dir| dir.join("hosts"))
}

pub fn parse_registry(text: &str) -> Vec<Host> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let forge = Forge::from_config(words.next()?)?;
            Host::new(forge, words.next()?, words.next().unwrap_or_default())
        })
        .collect()
}

pub fn render_registry(hosts: &[Host]) -> String {
    hosts
        .iter()
        .map(|h| format!("{} {} {}\n", h.forge.config_value(), h.origin, h.client_id))
        .collect()
}

/// The public servers, then the ones added.
pub fn hosts() -> Vec<Host> {
    let mut all = Host::built_in();
    let added = registry_file()
        .map(|file| read_registry(&file))
        .unwrap_or_default();
    for host in added {
        match all.iter_mut().find(|known| known.origin == host.origin) {
            // A public server with no ID in this build can be given one.
            Some(known) if known.client_id.is_empty() => *known = host,
            Some(_) => {}
            None => all.push(host),
        }
    }
    all
}

fn read_registry(file: &std::path::Path) -> Vec<Host> {
    std::fs::read_to_string(file)
        .map(|text| parse_registry(&text))
        .unwrap_or_default()
}

fn write_registry(file: &std::path::Path, hosts: &[Host]) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(file, render_registry(hosts)).map_err(|e| e.to_string())
}

/// The servers that were added by hand, which can be edited and removed. The public ones
/// are always there.
pub fn custom_hosts() -> Vec<Host> {
    let public: Vec<String> = Host::built_in().into_iter().map(|h| h.origin).collect();
    registry_file()
        .map(|file| read_registry(&file))
        .unwrap_or_default()
        .into_iter()
        .filter(|host| !public.contains(&host.origin))
        .collect()
}

/// Remembers a server, or the client ID of one already there.
pub fn add_host(host: &Host) -> Result<(), String> {
    let file = registry_file().ok_or("There is no folder to keep settings in.")?;
    save_host_in(&file, None, host, &Keychain)
}

/// Saves `host`. With `replacing` the server of that address is changed into it: the
/// old entry goes, and so does its token when the address changed, since a token is for
/// one server.
pub fn save_host(replacing: Option<&str>, host: &Host) -> Result<(), String> {
    let file = registry_file().ok_or("There is no folder to keep settings in.")?;
    save_host_in(&file, replacing, host, &Keychain)
}

pub fn save_host_in(
    file: &std::path::Path,
    replacing: Option<&str>,
    host: &Host,
    store: &dyn Store,
) -> Result<(), String> {
    let mut added = read_registry(file);
    // Moving onto another server that is already added would silently overwrite it.
    let taken = |origin: &str| added.iter().any(|known| known.origin == origin);
    if replacing != Some(host.origin.as_str()) && replacing.is_some() && taken(&host.origin) {
        return Err(format!("{} is already added.", host.name()));
    }
    if let Some(old) = replacing {
        added.retain(|known| known.origin != old);
        if old != host.origin {
            store.remove(old)?;
        }
    }
    added.retain(|known| known.origin != host.origin);
    added.push(host.clone());
    write_registry(file, &added)
}

/// Forgets a server added by hand, and the token for it.
pub fn remove_host(origin: &str) -> Result<(), String> {
    let file = registry_file().ok_or("There is no folder to keep settings in.")?;
    remove_host_in(&file, origin, &Keychain)
}

pub fn remove_host_in(
    file: &std::path::Path,
    origin: &str,
    store: &dyn Store,
) -> Result<(), String> {
    let mut added = read_registry(file);
    added.retain(|known| known.origin != origin);
    store.remove(origin)?;
    write_registry(file, &added)
}

/// An account that is signed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub host: Host,
    pub login: String,
}

/// The servers with a token in the keychain.
pub fn accounts() -> Vec<Account> {
    accounts_in(&Keychain, &hosts())
}

pub fn accounts_in(store: &dyn Store, hosts: &[Host]) -> Vec<Account> {
    hosts
        .iter()
        .filter_map(|host| {
            let token = store.get(&host.origin)?;
            Some(Account {
                host: host.clone(),
                login: token.login.unwrap_or_else(|| "signed in".into()),
            })
        })
        .collect()
}

/// A token that works now: the stored one, or after renewing it with its refresh token.
pub(crate) fn fresh_token(
    host: &Host,
    store: &dyn Store,
    http: &dyn Transport,
) -> Result<Token, String> {
    let token = store
        .get(&host.origin)
        .ok_or_else(|| format!("Not signed in to {}.", host.name()))?;
    if !token.is_expired(now()) {
        return Ok(token);
    }
    let renewed = flow::refresh(host, http, &token)?;
    store.set(&host.origin, &renewed)?;
    Ok(renewed)
}

/// Starts a sign-in: listens on this machine and makes the address to open in the browser
/// (`url` of the result). It doesn't block. Hand the result to [`finish_sign_in`].
pub fn begin_sign_in(host: &Host) -> Result<flow::Pending, String> {
    flow::begin(host)
}

/// Waits for the person to finish in the browser, then keeps the token. Blocks until they
/// do, `cancel` is set, or five minutes pass.
pub fn finish_sign_in(
    host: &Host,
    pending: flow::Pending,
    cancel: &Cancel,
) -> Result<Account, String> {
    finish_sign_in_with(host, pending, cancel, &Keychain, &Web)
}

pub fn finish_sign_in_with(
    host: &Host,
    pending: flow::Pending,
    cancel: &Cancel,
    store: &dyn Store,
    http: &dyn Transport,
) -> Result<Account, String> {
    let mut token = pending.wait(http, cancel)?;
    let login = api::login(host, http, &token.access)?;
    token.login = Some(login.clone());
    store.set(&host.origin, &token)?;
    Ok(Account {
        host: host.clone(),
        login,
    })
}

/// This computer's name, for titling a key on the forge.
pub fn machine_name() -> String {
    ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this computer".to_owned())
}

/// How a server's row reads on the Accounts page.
pub fn status_sentence(host: &Host, account: Option<&Account>) -> String {
    match account {
        Some(account) => format!("Signed in as {}.", account.login),
        None if host.can_sign_in() => "Not signed in.".to_owned(),
        None => format!(
            "This build of Mantis Git has no OAuth app on {}. Add the client ID of one you registered there.",
            host.name()
        ),
    }
}

/// The button of a row, named for its result.
pub fn action_label(host: &Host, account: Option<&Account>) -> String {
    match account {
        Some(_) => format!("Sign out of {}", host.name()),
        None => format!("Sign in to {}", host.name()),
    }
}

/// What signing out did and didn't do.
pub fn signed_out_message(host: &Host) -> String {
    format!(
        "Signed out of {}. The token is gone from this computer; to revoke it there too, remove \
         Mantis Git from your authorized applications on {}.",
        host.name(),
        host.name()
    )
}

/// Forgets the token on this machine. It stays valid on the server until revoked there,
/// which the sentence the app shows says.
pub fn sign_out(host: &Host) -> Result<(), String> {
    Keychain.remove(&host.origin)
}

/// Adds the public key at `key_file` to the signed-in account on `host`.
pub fn upload_ssh_key(
    host: &Host,
    key_file: &std::path::Path,
    machine: &str,
) -> Result<String, String> {
    upload_ssh_key_with(host, key_file, machine, &Keychain, &Web)
}

pub fn upload_ssh_key_with(
    host: &Host,
    key_file: &std::path::Path,
    machine: &str,
    store: &dyn Store,
    http: &dyn Transport,
) -> Result<String, String> {
    let text = std::fs::read_to_string(key_file)
        .map_err(|e| format!("Couldn't read {}: {e}.", key_file.display()))?;
    let token = fresh_token(host, store, http)?;
    let user = token.login.as_deref().unwrap_or("me");
    api::add_ssh_key(
        host,
        http,
        &token.access,
        &api::key_title(user, machine),
        &text,
    )
}

/// What git's HTTPS prompt should be answered with, for the askpass program. `None` means
/// show the person the question.
pub(crate) fn answer_git(prompt: &str) -> Option<String> {
    credentials::parse(prompt)?;
    credentials::answer(prompt, &hosts(), &Keychain, &Web)
}

#[cfg(test)]
mod tests {
    use super::token::Memory;
    use super::transport::fake::Fake;
    use super::*;

    fn host() -> Host {
        Host::new(Forge::Gitea, "https://codeberg.org", "id").unwrap()
    }

    #[test]
    fn a_server_can_be_edited_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hosts");
        let store = Memory::default();
        let one = Host::new(Forge::Gitea, "https://git.corp", "a").unwrap();
        let two = Host::new(Forge::GitLab, "https://lab.corp", "b").unwrap();
        let token = Token {
            access: "t".into(),
            refresh: None,
            expires_at: None,
            login: None,
        };
        save_host_in(&file, None, &one, &store).unwrap();
        save_host_in(&file, None, &two, &store).unwrap();
        store.set(&one.origin, &token).unwrap();
        // A new client ID keeps the token, since the server is the same.
        let newer = Host::new(Forge::Gitea, "https://git.corp", "a2").unwrap();
        save_host_in(&file, Some(&one.origin), &newer, &store).unwrap();
        assert_eq!(read_registry(&file), vec![two.clone(), newer.clone()]);
        assert!(store.get(&one.origin).is_some());
        // A new address drops the token, and can't land on another added server.
        let moved = Host::new(Forge::Gitea, "https://git.new", "a2").unwrap();
        save_host_in(&file, Some(&newer.origin), &moved, &store).unwrap();
        assert!(store.get(&one.origin).is_none());
        let clash = Host::new(Forge::Gitea, "https://lab.corp", "x").unwrap();
        let err = save_host_in(&file, Some(&moved.origin), &clash, &store).unwrap_err();
        assert!(err.contains("already added"));
        assert_eq!(read_registry(&file), vec![two.clone(), moved.clone()]);
        remove_host_in(&file, &moved.origin, &store).unwrap();
        assert_eq!(read_registry(&file), vec![two]);
    }

    #[test]
    fn a_row_says_where_the_account_stands() {
        let with_id = host();
        let bare = Host::new(Forge::GitLab, "https://git.corp", "").unwrap();
        let account = Account {
            host: with_id.clone(),
            login: "ada".into(),
        };
        assert_eq!(
            status_sentence(&with_id, Some(&account)),
            "Signed in as ada."
        );
        assert_eq!(status_sentence(&with_id, None), "Not signed in.");
        assert!(status_sentence(&bare, None).contains("client ID"));
        assert_eq!(action_label(&with_id, None), "Sign in to codeberg.org");
        assert_eq!(
            action_label(&with_id, Some(&account)),
            "Sign out of codeberg.org"
        );
        assert!(signed_out_message(&with_id).contains("revoke"));
    }

    #[test]
    fn the_registry_round_trips_and_skips_bad_lines() {
        let hosts = parse_registry("gitlab https://git.corp abc\nnonsense\ngitea ssh://x y\n");
        assert_eq!(hosts.len(), 1);
        assert_eq!(parse_registry(&render_registry(&hosts)), hosts);
    }

    #[test]
    fn signing_in_stores_the_token_with_the_login() {
        let store = Memory::default();
        let host = host();
        // The "browser" goes to the address and is sent back by the test below.
        let http = Fake::answering(&[
            (200, r#"{"access_token":"tok"}"#),
            (200, r#"{"login":"ada"}"#),
        ]);
        let opened = std::sync::Mutex::new(String::new());
        std::thread::scope(|scope| {
            let pending = begin_sign_in(&host).unwrap();
            *opened.lock().unwrap() = pending.url.clone();
            let signing =
                scope.spawn(|| finish_sign_in_with(&host, pending, &Cancel::new(), &store, &http));
            let url = loop {
                let url = opened.lock().unwrap().clone();
                if !url.is_empty() {
                    break url;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let query = pkce::parse_query(url.split_once('?').unwrap().1);
            let get = |k: &str| query.iter().find(|(n, _)| n == k).unwrap().1.clone();
            let redirect = get("redirect_uri");
            let port = redirect
                .rsplit(':')
                .next()
                .unwrap()
                .split('/')
                .next()
                .unwrap()
                .to_owned();
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            write!(
                stream,
                "GET /callback?code=c&state={} HTTP/1.1\r\n\r\n",
                get("state")
            )
            .unwrap();
            let mut page = String::new();
            let _ = stream.read_to_string(&mut page);
            let account = signing.join().unwrap().unwrap();
            assert_eq!(account.login, "ada");
        });
        assert_eq!(
            store.get("https://codeberg.org").unwrap().login.as_deref(),
            Some("ada")
        );
        assert_eq!(accounts_in(&store, &[host]).len(), 1);
    }

    #[test]
    fn a_key_is_uploaded_under_the_signed_in_account() {
        let store = Memory::default();
        let host = host();
        store
            .set(
                &host.origin,
                &Token {
                    access: "tok".into(),
                    refresh: None,
                    expires_at: None,
                    login: Some("ada".into()),
                },
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("id.pub");
        std::fs::write(&file, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me@box\n").unwrap();
        let http = Fake::answering(&[(201, "{}")]);
        let said = upload_ssh_key_with(&host, &file, "box", &store, &http).unwrap();
        assert_eq!(said, "Added “Mantis Git, ada@box” to codeberg.org.");
        // Signed out, nothing is sent.
        let out = Memory::default();
        let http = Fake::default();
        assert!(
            upload_ssh_key_with(&host, &file, "box", &out, &http)
                .unwrap_err()
                .contains("Not signed in")
        );
        assert!(http.seen.lock().unwrap().is_empty());
    }
}
