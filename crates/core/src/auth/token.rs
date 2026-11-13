//! A signed-in account's token, and where it is kept.
//!
//! Tokens live in the system's keychain (Secret Service on Linux, Keychain on macOS,
//! Credential Manager on Windows) and never in a file of ours.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a sign-in gave: the token, when it stops working and how to get another, and who
/// it is for.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    pub access: String,
    pub refresh: Option<String>,
    /// Seconds since the epoch. `None` for a token that doesn't expire (GitHub's).
    pub expires_at: Option<u64>,
    pub login: Option<String>,
}

// Never print a secret by accident.
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("login", &self.login)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl Token {
    /// Whether it has stopped working, or will within a minute.
    pub fn is_expired(&self, at: u64) -> bool {
        self.expires_at.is_some_and(|end| end <= at + 60)
    }

    pub fn to_json(&self) -> String {
        json!({
            "access": self.access,
            "refresh": self.refresh,
            "expires_at": self.expires_at,
            "login": self.login,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let text_of = |key: &str| value.get(key)?.as_str().map(str::to_owned);
        Some(Self {
            access: text_of("access").filter(|a| !a.is_empty())?,
            refresh: text_of("refresh"),
            expires_at: value.get("expires_at").and_then(Value::as_u64),
            login: text_of("login"),
        })
    }
}

/// Where tokens are kept, one for each server's origin.
pub trait Store: Send + Sync {
    fn get(&self, origin: &str) -> Option<Token>;
    fn set(&self, origin: &str, token: &Token) -> Result<(), String>;
    fn remove(&self, origin: &str) -> Result<(), String>;
}

/// The system keychain.
pub struct Keychain;

const SERVICE: &str = "mantisgit";

impl Keychain {
    fn entry(origin: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, origin).map_err(describe)
    }
}

fn describe(error: keyring::Error) -> String {
    format!("The system keychain said: {error}. Is a keyring service running?")
}

impl Store for Keychain {
    fn get(&self, origin: &str) -> Option<Token> {
        Token::from_json(&Self::entry(origin).ok()?.get_password().ok()?)
    }

    fn set(&self, origin: &str, token: &Token) -> Result<(), String> {
        Self::entry(origin)?
            .set_password(&token.to_json())
            .map_err(describe)
    }

    fn remove(&self, origin: &str) -> Result<(), String> {
        match Self::entry(origin)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(describe(error)),
        }
    }
}

/// Tokens kept in memory, for tests.
#[derive(Default)]
pub struct Memory(Mutex<HashMap<String, Token>>);

impl Store for Memory {
    fn get(&self, origin: &str) -> Option<Token> {
        self.0.lock().ok()?.get(origin).cloned()
    }

    fn set(&self, origin: &str, token: &Token) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .insert(origin.to_owned(), token.clone());
        Ok(())
    }

    fn remove(&self, origin: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(origin);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> Token {
        Token {
            access: "a".into(),
            refresh: Some("r".into()),
            expires_at: Some(1000),
            login: Some("ada".into()),
        }
    }

    #[test]
    fn a_token_round_trips_and_a_damaged_one_is_nothing() {
        assert_eq!(Token::from_json(&token().to_json()), Some(token()));
        assert_eq!(Token::from_json("{}"), None);
        assert_eq!(Token::from_json("nonsense"), None);
    }

    #[test]
    fn expiry_leaves_a_minute_of_margin() {
        assert!(!token().is_expired(900));
        assert!(token().is_expired(940));
        let forever = Token {
            expires_at: None,
            ..token()
        };
        assert!(!forever.is_expired(u64::MAX / 2));
    }

    #[test]
    fn debug_never_shows_the_secret() {
        let shown = format!("{:?}", token());
        assert!(!shown.contains("\"a\"") && !shown.contains("\"r\""));
    }

    #[test]
    fn the_memory_store_keeps_and_forgets() {
        let store = Memory::default();
        store.set("https://x", &token()).unwrap();
        assert_eq!(store.get("https://x"), Some(token()));
        store.remove("https://x").unwrap();
        assert_eq!(store.get("https://x"), None);
    }
}
