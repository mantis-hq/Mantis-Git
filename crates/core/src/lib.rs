//! Everything Mantis Git knows about a repository, with no user interface. This crate must
//! not depend on gpui-kit, so that all of it can be tested without a window.

pub mod about;
pub mod askpass;
pub mod cache;
pub mod commit;
pub mod history;
pub mod install;
pub mod log;
pub mod menu;
pub mod ops;
pub mod patch;
pub mod repo;
pub mod settings;
pub mod terminal;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod text;
pub mod tree;
pub mod working;
