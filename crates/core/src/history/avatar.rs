//! What the graph draws on a commit's dot to say who wrote it: by default the author's
//! initials on a colour made from their address, or their Gravatar, or a picture the user
//! chose for themselves.
//!
//! Gravatar is asked by the SHA-256 of the lower-cased address, never the address itself,
//! but the hash still tells it which authors a repository has. That is why it is off
//! unless chosen, and why the setting says so.

use crate::cache::Cacheable;
use crate::cache::codec::{Reader, Writer};
use crate::ops;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

const HOST: &str = "https://gravatar.com/avatar";
/// Twice the 16 px the graph draws, so it stays sharp on a high-density screen.
const SIZE: u32 = 40;
const PATIENCE: Duration = Duration::from_secs(4);
/// A Gravatar picture is a few kilobytes; anything bigger isn't one.
const MAX_BYTES: u64 = 256 * 1024;
/// The biggest picture a user may choose for themselves.
pub const MAX_OWN_BYTES: u64 = 8 * 1024 * 1024;
/// How many colours `tint` picks from. The view maps each to a branch palette colour.
pub const TINTS: u32 = 7;

/// What the graph draws on commit dots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    /// The author's initials on a colour, and a chosen picture for the user.
    #[default]
    Initials,
    /// The author's Gravatar, with initials where there is none.
    Gravatar,
    /// Plain dots.
    Off,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Initials, Style::Gravatar, Style::Off];

    pub fn key(self) -> &'static str {
        match self {
            Style::Initials => "initials",
            Style::Gravatar => "gravatar",
            Style::Off => "off",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|style| style.key() == key.trim())
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Style::Initials => "Initials",
            Style::Gravatar => "Gravatar",
            Style::Off => "Plain dots",
        }
    }

    pub fn saved_message(self) -> &'static str {
        match self {
            Style::Initials => "Saved. Commit dots show the author's initials.",
            Style::Gravatar => {
                "Saved. Commit dots show Gravatars, and initials where there is none."
            }
            Style::Off => "Saved. Commit dots are plain.",
        }
    }
}

/// What Gravatar is asked for `email`. `d=404` makes an address with no picture an error
/// instead of a stand-in, so the graph can draw initials for it. The `.png` is how
/// Gravatar is told to answer in a format the app decodes.
pub fn url(email: &str) -> String {
    format!("{HOST}/{}.png?s={SIZE}&d=404", key(email))
}

/// The same address always gives the same key, whatever its case or padding.
pub fn key(email: &str) -> String {
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether an address is worth asking about: git writes `unknown` and the like when a
/// commit has no address, and those would all share one picture.
pub fn is_askable(email: &str) -> bool {
    let email = email.trim();
    email.contains('@') && !email.ends_with(".(none)") && !email.contains(char::is_whitespace)
}

/// Downloads the picture for `email`. `Ok(None)` means Gravatar answered that there is
/// none; `Err` says why it couldn't be asked or answered oddly.
pub fn fetch(email: &str) -> Result<Option<Vec<u8>>, String> {
    if !is_askable(email) {
        return Ok(None);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PATIENCE))
        .user_agent("gitclient")
        .build()
        .into();
    let mut response = match agent.get(&url(email)).call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(ureq::Error::StatusCode(code)) => {
            return Err(format!("gravatar.com answered with an error ({code})."));
        }
        Err(error) => return Err(format!("Couldn't reach gravatar.com: {error}")),
    };
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .map_err(|error| format!("Couldn't read gravatar.com's answer: {error}"))?;
    // Asked for a PNG, but whatever picture comes is drawn, so an account whose picture
    // Gravatar won't convert isn't left with initials.
    if sniff(&bytes).is_some() {
        Ok(Some(bytes))
    } else {
        Err("gravatar.com answered with something that isn't a picture.".to_owned())
    }
}

/// A Gravatar picture as it is kept in the cache: the file as Gravatar sent it. Only a
/// picture is kept; "none" and failures are remembered for the session alone.
pub struct Picture(pub Vec<u8>);

impl Cacheable for Picture {
    const FORMAT: u32 = 1;

    fn weight(&self) -> usize {
        self.0.len() + 64
    }

    fn encode(&self, out: &mut Writer) {
        out.bytes(&self.0);
    }

    fn decode(input: &mut Reader) -> Option<Self> {
        let bytes = input.bytes()?;
        // Whatever was read must still be a picture, so a damaged file is read again.
        (input.is_empty() && sniff(bytes).is_some()).then(|| Picture(bytes.to_vec()))
    }
}

/// The first letter of the first and of the last word of a name, in capitals ("Ada
/// Lovelace" is "AL", "ada" is "A"). A name with no letters or digits gives "?".
pub fn initials(name: &str) -> String {
    let mut words = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty());
    let first = words.next();
    let last = words.last();
    let letters: String = [first, last]
        .into_iter()
        .flatten()
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    }
}

/// Which of `TINTS` colours a person gets. It comes from their address, falling back to
/// their name, so it is the same in every repository and every session.
pub fn tint(email: &str, name: &str) -> u32 {
    let who = if email.trim().is_empty() { name } else { email };
    let digest = Sha256::digest(who.trim().to_lowercase().as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) % TINTS
}

/// The kinds of picture the app can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
}

/// What kind of picture `bytes` are, by their first bytes rather than their file name.
pub fn sniff(bytes: &[u8]) -> Option<Kind> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(Kind::Png),
        [0xff, 0xd8, 0xff, ..] => Some(Kind::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(Kind::Gif),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(Kind::Webp),
        [b'B', b'M', ..] => Some(Kind::Bmp),
        _ => None,
    }
}

/// Reads the picture the user chose, saying why when it can't be used.
pub fn read_own(file: &Path) -> Result<(Kind, Vec<u8>), String> {
    let name = file.display();
    let size = std::fs::metadata(file)
        .map_err(|error| format!("Couldn't read {name}: {error}"))?
        .len();
    if size > MAX_OWN_BYTES {
        return Err(format!(
            "{name} is over 8 MB, which is too big for an avatar."
        ));
    }
    let bytes = std::fs::read(file).map_err(|error| format!("Couldn't read {name}: {error}"))?;
    match sniff(&bytes) {
        Some(kind) => Ok((kind, bytes)),
        None => Err(format!(
            "{name} isn't a PNG, JPEG, GIF, WebP or BMP picture."
        )),
    }
}

/// The address the user's commits in this repository carry, which their own picture is
/// drawn for. `None` when git has none set.
pub fn own_email(repo: &Path) -> Option<String> {
    let email = ops::run(repo, &["config", "user.email"]).ok()?;
    let email = email.trim();
    (!email.is_empty()).then(|| email.to_lowercase())
}

/// The addresses Gravatar has been asked about this session, so each is asked once
/// however many commits it wrote, whatever the answer. Asking again takes a restart.
#[derive(Default)]
pub struct Asked {
    seen: Mutex<HashSet<String>>,
}

impl Asked {
    /// True the first time an address is claimed, which is when to start the download.
    pub fn claim(&self, email: &str) -> bool {
        is_askable(email)
            && self
                .seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key(email))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_ignores_case_and_padding() {
        assert_eq!(key(" Ada@Example.COM "), key("ada@example.com"));
        assert_eq!(key("a@b.c").len(), 64);
    }

    #[test]
    fn the_url_carries_only_the_hash() {
        let url = url("ada@example.com");
        assert!(url.starts_with("https://gravatar.com/avatar/"));
        assert!(!url.contains("ada"));
        assert!(url.ends_with(".png?s=40&d=404"));
    }

    #[test]
    fn addresses_that_are_not_addresses_are_not_asked_about() {
        assert!(is_askable("ada@example.com"));
        assert!(!is_askable(""));
        assert!(!is_askable("unknown"));
        assert!(!is_askable("ada@host.(none)"));
    }

    #[test]
    fn each_address_is_claimed_once() {
        let asked = Asked::default();
        assert!(asked.claim("a@b.c"));
        assert!(!asked.claim("A@B.C "));
        assert!(!asked.claim("nobody"));
    }

    #[test]
    fn initials_are_the_first_and_last_words_first_letters() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("ada"), "A");
        assert_eq!(initials("  ada  king-noel  lovelace "), "AL");
        assert_eq!(initials("Émile Zola"), "ÉZ");
        assert_eq!(initials("dependabot[bot]"), "DB");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("--"), "?");
    }

    #[test]
    fn a_person_keeps_their_colour() {
        let one = tint("Ada@Example.com", "Ada");
        assert_eq!(one, tint(" ada@example.com ", "Someone else"));
        assert!(one < TINTS);
        assert_eq!(tint("", "Ada"), tint("", "ada"));
        let spread: HashSet<u32> = (0..50)
            .map(|n| tint(&format!("user{n}@example.com"), ""))
            .collect();
        assert!(spread.len() >= 5, "colours should be spread out");
    }

    #[test]
    fn pictures_are_told_by_their_first_bytes() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0, 0]), Some(Kind::Png));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(Kind::Jpeg));
        assert_eq!(sniff(b"GIF89a"), Some(Kind::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Kind::Webp));
        assert_eq!(sniff(b"hello"), None);
    }

    #[test]
    fn a_chosen_file_that_is_not_a_picture_is_turned_down() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("me.png");
        std::fs::write(&text, "not a picture").unwrap();
        assert!(read_own(&text).unwrap_err().contains("isn't a PNG"));
        let png = dir.path().join("real.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 1, 2]).unwrap();
        assert_eq!(read_own(&png).unwrap().0, Kind::Png);
        assert!(read_own(&dir.path().join("gone.png")).is_err());
    }

    /// Reaches the real gravatar.com with the address its own documentation uses, so it
    /// needs a network and is run by hand: `cargo test -p gitclient-core -- --ignored gravatar`.
    #[test]
    #[ignore = "needs the network"]
    fn gravatar_answers_for_a_known_address_and_not_for_an_unknown_one() {
        let bytes = fetch("beau@dentedreality.com.au")
            .unwrap()
            .expect("a picture");
        assert!(sniff(&bytes).is_some());
        assert_eq!(fetch("nobody-has-this@example.invalid"), Ok(None));
    }

    #[test]
    fn the_style_is_read_from_its_key() {
        for style in Style::ALL {
            assert_eq!(Style::from_key(style.key()), style);
        }
        assert_eq!(Style::from_key("nonsense"), Style::Initials);
    }
}
