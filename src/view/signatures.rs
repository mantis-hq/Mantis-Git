//! The shield on a signed commit. Git checks each signature by running gpg or ssh-keygen,
//! which takes seconds in a repository of signed commits, so the history is read without
//! them and the rows in view ask for theirs afterwards, off the UI thread. What git said is
//! kept in the cache (see `Caches::read_signatures`), so a commit seen before shows its
//! shield at once on the next run.

use super::GraphView;
use gpui_kit::prelude::*;
use mantisgit_core::history::Signature;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

/// The most commits one call to git checks, so a long scroll is read in pieces and the
/// first shields arrive before the last.
const PER_CALL: usize = 64;

/// What the view knows about signatures in the repository on show.
#[derive(Default)]
pub(super) struct State {
    /// What git said, by commit. A commit missing here shows no shield yet.
    known: HashMap<Arc<str>, Signature>,
    /// Commits already asked about, answered or not, so a frame never asks twice.
    asked: HashSet<Arc<str>>,
}

impl GraphView {
    /// The signature of the commit `oid`, `None` until git has been asked.
    pub(super) fn signature_of(&self, oid: &str) -> Signature {
        self.signatures.known.get(oid).copied().unwrap_or_default()
    }

    /// Asks for the signatures of the rows in `range` that haven't been asked about. Called
    /// before a batch of rows is built, so that building them only looks things up.
    pub(super) fn prepare_signatures(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let end = range.end.min(self.entries.len());
        let wanted: Vec<Arc<str>> = self.entries[range.start.min(end)..end]
            .iter()
            .filter(|entry| !entry.working && !entry.commit.oid.is_empty())
            .map(|entry| entry.commit.oid.clone())
            .filter(|oid| !self.signatures.asked.contains(oid))
            .collect();
        for oids in wanted.chunks(PER_CALL) {
            let oids: Vec<Arc<str>> = oids.to_vec();
            self.signatures.asked.extend(oids.iter().cloned());
            let caches = self.caches.clone();
            let (read_in, shown) = (repo.clone(), repo.clone());
            Self::spawn_blocking(
                move || caches.read_signatures(&read_in, &oids),
                move |this, read, cx| {
                    // The tab may have moved to another repository meanwhile.
                    if this.repo.as_deref() != Some(shown.as_path()) {
                        return;
                    }
                    // A failure shows no shield and isn't retried on every repaint; the
                    // Log has what git said.
                    if let Ok(read) = read {
                        this.signatures.known.extend(
                            read.into_iter()
                                .filter(|(_, signature)| *signature != Signature::None),
                        );
                        cx.notify();
                    }
                },
                cx,
            );
        }
    }
}
