//! Everything gitclient knows about a repository, with no user interface. This crate must
//! not depend on gpui-kit, so that all of it can be tested without a window.

pub mod branch;
pub mod config;
pub mod conflict;
pub mod details;
pub mod git;
pub mod graph;
pub mod log;
pub mod menu;
pub mod ops;
pub mod patch;
pub mod selection;
pub mod spotlight;
pub mod status;
pub mod terminal;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;
pub mod working;
