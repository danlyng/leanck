//! Typed Lean sessions, library-backed LSP I/O, and owned process lifecycle.

pub mod extensions;
mod io;
pub mod lifecycle;
pub mod session;

pub use session::LeanSession;

#[cfg(test)]
pub(crate) mod test_peer;
