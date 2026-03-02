//! The Lean server side: stdio framing, JSON-RPC client, process lifecycle,
//! and supervision.

pub mod jsonrpc;
pub mod lifecycle;
pub mod transport;

pub use jsonrpc::{JsonRpc, NotifySink};
pub use lifecycle::LeanServer;
