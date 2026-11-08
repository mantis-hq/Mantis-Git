//! Everything gitclient knows about a repository, with no user interface. This crate must
//! not depend on gpui-kit, so that all of it can be tested without a window.

pub mod branch;
pub mod cache;
pub mod config;
pub mod conflict;
pub mod details;
pub mod history;
pub mod identity;
pub mod log;
pub mod menu;
pub mod ops;
pub mod patch;
pub mod remote;
pub mod ssh;
pub mod state;
pub mod status;
pub mod submodule;
pub mod terminal;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;
pub mod tree;
pub mod working;
pub mod worktree;
