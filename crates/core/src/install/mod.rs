//! How this copy of the app got onto the machine, and what it may do about that.
//!
//! A copy unpacked from a release archive looks after itself: it can add a launcher to
//! the application menu and replace itself with a newer release. A copy a package manager
//! installed does neither, since that would fight the package manager over the same files.
//! Everything here works without a window, so it is tested without one.

mod channel;
pub mod menu_entry;
pub mod update;

pub use channel::{Channel, exe_path};
