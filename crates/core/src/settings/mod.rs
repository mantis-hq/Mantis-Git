//! What the user sets up outside any one repository: git config worth changing, the
//! identities they commit as, SSH suggestions for them, and what the app itself remembers.

pub mod config;
pub mod identity;
pub mod ssh;
pub mod state;
pub mod theme;
pub mod themes;
