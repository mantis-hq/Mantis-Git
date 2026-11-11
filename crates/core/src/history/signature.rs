//! Whether a commit is signed, and whether git can vouch for it.
//!
//! Git checks a signature by running gpg or ssh-keygen once for each signed commit, which in
//! a repository of signed commits takes seconds. So it is not read with the history: the
//! rows in view ask for theirs afterwards ([`read`]), and what git said is kept in the cache
//! (`cache::keys::Caches::signatures`).

use crate::cache::{Cacheable, Reader, Writer};
use crate::ops;
use std::path::Path;
use std::sync::Arc;

/// What git says about a commit's signature (`%G?`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Signature {
    #[default]
    None,
    /// Signed, and the signature checks out.
    Good,
    /// Signed, but git can't say whether to trust it: the key is missing, expired,
    /// revoked or unknown, or there is nothing to check an SSH signature against.
    Unchecked,
    /// Signed, and the signature does not match the commit.
    Bad,
}

impl Signature {
    pub(crate) fn from_status(status: &str) -> Self {
        match status {
            "G" | "U" => Self::Good,
            "B" => Self::Bad,
            "N" | "" => Self::None,
            _ => Self::Unchecked,
        }
    }

    /// The tooltip of the mark a signed commit gets, or `None` for an unsigned one.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Good => Some("Signed"),
            Self::Unchecked => Some("Signed, but git can't check the signature"),
            Self::Bad => Some("Signature does not match this commit"),
        }
    }

    /// Whether the answer is worth keeping between runs. "Unchecked" mostly means a key
    /// that isn't there yet, which the user may fix, so it is asked again next time.
    pub fn is_final(self) -> bool {
        self != Self::Unchecked
    }
}

impl Cacheable for Signature {
    const FORMAT: u32 = 1;

    fn weight(&self) -> usize {
        // Mostly the entry's key and bookkeeping.
        128
    }

    fn encode(&self, out: &mut Writer) {
        out.u8(match self {
            Self::None => 0,
            Self::Good => 1,
            Self::Unchecked => 2,
            Self::Bad => 3,
        });
    }

    fn decode(input: &mut Reader) -> Option<Self> {
        match input.u8()? {
            0 => Some(Self::None),
            1 => Some(Self::Good),
            2 => Some(Self::Unchecked),
            3 => Some(Self::Bad),
            _ => None,
        }
    }
}

/// What git says about each of `oids`' signatures, in one call. Blocks, and runs gpg or
/// ssh-keygen for each signed commit, so call it off the UI thread and for few commits.
pub fn read(repo: &Path, oids: &[Arc<str>]) -> Result<Vec<(Arc<str>, Signature)>, String> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = oids.iter().map(|oid| format!("{oid}\n")).collect();
    let output = ops::run_with_input(
        repo,
        &[
            "log",
            "--stdin",
            "--no-walk=unsorted",
            "--no-color",
            // log.showSignature would otherwise mix gpg output into the records.
            "--no-show-signature",
            "--format=%H%x00%G?",
        ],
        input.as_bytes(),
    )?;
    Ok(output
        .lines()
        .filter_map(|line| line.split_once('\0'))
        .map(|(oid, status)| (Arc::from(oid), Signature::from_status(status)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn signature_status_maps_to_what_the_row_shows() {
        for (status, expected) in [
            ("N", Signature::None),
            ("G", Signature::Good),
            ("U", Signature::Good),
            ("B", Signature::Bad),
            ("E", Signature::Unchecked),
            ("X", Signature::Unchecked),
            ("Y", Signature::Unchecked),
            ("R", Signature::Unchecked),
        ] {
            assert_eq!(Signature::from_status(status), expected, "{status}");
        }
        assert!(Signature::None.hint().is_none());
        assert!(Signature::Good.hint().is_some());
    }

    #[test]
    fn a_signature_survives_encoding_and_a_bad_one_is_refused() {
        for signature in [
            Signature::None,
            Signature::Good,
            Signature::Unchecked,
            Signature::Bad,
        ] {
            let mut out = Writer::new();
            signature.encode(&mut out);
            let bytes = out.finish();
            assert_eq!(Signature::decode(&mut Reader::new(&bytes)), Some(signature));
        }
        assert_eq!(Signature::decode(&mut Reader::new(&[9])), None);
    }

    #[test]
    fn only_a_settled_answer_is_worth_keeping() {
        assert!(Signature::Good.is_final() && Signature::Bad.is_final());
        assert!(Signature::None.is_final());
        assert!(!Signature::Unchecked.is_final());
    }

    #[test]
    fn unsigned_commits_are_read_as_unsigned_in_one_call() {
        let repo = Repo::new();
        let first: Arc<str> = repo.commit("one").into();
        let second: Arc<str> = repo.commit("two").into();
        let signatures = read(repo.path(), &[first.clone(), second.clone()]).unwrap();
        assert_eq!(signatures.len(), 2);
        assert!(signatures.iter().all(|(_, s)| *s == Signature::None));
        assert!(signatures.iter().any(|(oid, _)| *oid == first));
        assert!(signatures.iter().any(|(oid, _)| *oid == second));
        assert!(read(repo.path(), &[]).unwrap().is_empty());
    }
}
