//! leanckd — a standalone daemon that owns one warm `lake serve` per Lean
//! worktree and answers proof-state queries (diagnostics, goal, sorry, list,
//! restart_file) over a Unix socket.
//!
//! CLI and MCP reach this service through `leanck-client`, which owns discovery,
//! startup, and transport policy. Shared identity and protocol data live in
//! `leanck-core`; client-side rendering lives in `leanck-client`. Independent
//! Lake build/cache utilities live in `leanck-lake`.

mod activity;
mod daemon;
mod declarations;
mod decls;
mod documents;
mod error;
mod folding;
mod query;
mod request;
mod resolve;
mod server;
mod sorries;
mod state;
mod workspace;

pub use error::{Error, Result};

pub use daemon::{run, run_with_runtime_directory, run_with_settings, run_with_version};

#[cfg(test)]
mod live_smoke;
