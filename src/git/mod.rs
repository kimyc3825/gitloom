//! Local Git access: reads via libgit2, writes via the `git` CLI.

pub mod blame;
pub mod conflict;
pub mod graph;
pub mod ops;
pub mod patch;
pub mod rebase;
pub mod repo;
