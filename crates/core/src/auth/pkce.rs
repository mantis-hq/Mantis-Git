//! The small pieces of OAuth that need no network: PKCE (RFC 7636), percent-encoding, and
//! reading a redirect's query.

use sha2::{Digest, Sha256};

const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64 with the URL-safe alphabet and no padding, as PKCE and `state` use it.
pub fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(URL_SAFE[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// `n` random bytes from the operating system, as URL-safe text.
pub fn random_text(n: usize) -> Result<String, String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| format!("No source of randomness: {e}."))?;
    Ok(base64url(&bytes))
}

/// A PKCE pair: the secret kept here, and its hash sent with the sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Result<Self, String> {
        // 32 bytes make 43 characters, the shortest verifier the RFC allows.
        Ok(Self::from_verifier(random_text(32)?))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// Percent-encodes `text` for a query or a form body.
pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Undoes [`encode`], and `+` as a space. Bad escapes are kept as written.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let escaped = text
                    .get(i + 1..i + 3)
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                match escaped {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a=1&b=2` as pairs, decoded.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// A query from pairs, encoded.
pub fn build_query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_rfc_example() {
        // RFC 7636 appendix B.
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_fresh_verifier_is_long_enough_and_never_repeats() {
        let (a, b) = (Pkce::new().unwrap(), Pkce::new().unwrap());
        assert_eq!(a.verifier.len(), 43);
        assert_ne!(a.verifier, b.verifier);
    }

    #[test]
    fn base64url_pads_nothing() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn text_survives_encoding() {
        let text = "a b&c=d/é%";
        assert_eq!(decode(&encode(text)), text);
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%zzb"), "a%zzb");
    }

    #[test]
    fn a_redirect_query_is_read() {
        let pairs = parse_query("code=ab%20c&state=xyz&empty=");
        assert_eq!(pairs[0], ("code".into(), "ab c".into()));
        assert_eq!(pairs[2], ("empty".into(), "".into()));
        assert_eq!(build_query(&[("a", "b c")]), "a=b%20c");
    }
}
