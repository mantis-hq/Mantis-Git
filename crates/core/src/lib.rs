//! Everything gitclient knows about a repository, with no user interface. This crate must
//! not depend on gpui-kit, so that all of it can be tested without a window.

pub mod config;
pub mod conflict;
pub mod details;
pub mod git;
pub mod graph;
pub mod ops;
pub mod patch;
pub mod spotlight;
pub mod status;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;
pub mod working;
