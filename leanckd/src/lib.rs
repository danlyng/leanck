//! leanckd — a standalone daemon that owns one warm `lake serve` per Lean
//! worktree and answers proof-state queries (diagnostics, goal, sorry, list,
//! restart_file) over a Unix socket.
//!
//! It replaces the `leanck-lsp` VSCode extension + HTTP bridge: `leanck-mcp`
//! connects to this daemon as a client and auto-spawns it. The behavior and
//! JSON shapes are ported from `leanck-lsp`; the daemon scaffolding (socket,
//! single-instance, idle reaper, supervision) is new.

pub mod build;
pub mod cache;
pub mod client;
pub mod daemon;
pub mod decls;
pub mod error;
pub mod folding;
pub mod format;
pub mod paths;
pub mod proto;
pub mod query;
pub mod resolve;
pub mod server;
pub mod sorries;
pub mod state;
pub mod workspace;

pub use error::{Error, Result};
