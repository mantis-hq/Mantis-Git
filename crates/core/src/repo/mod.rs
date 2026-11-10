//! What a repository has besides its history: branches to bring together, remotes,
//! working trees, submodules, a paused operation, and where it stands overall.

pub mod auto_fetch;
pub mod branch;
pub mod clone;
pub mod commit_graph;
pub mod conflict;
pub mod forge;
pub mod remote;
pub mod stash;
pub mod status;
pub mod submodule;
pub mod worktree;
pub mod wsl;
