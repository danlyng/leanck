//! Structured Loogle search and endpoint provenance shared by CLI and MCP.
mod client;
mod models;
mod query;
mod render;
pub use client::{
    Client, Endpoint, EndpointError, EndpointFailure, EndpointFailures, EndpointProvenance,
    EndpointSource, SearchError, SearchOptions, SearchOutcome,
};
pub use models::{LoogleHit, LoogleResult};
pub use query::LoogleQuery;
pub use render::{format_human, format_results};

pub const DEFAULT_LIMIT: usize = 10;
pub const MULTI_TERM_EMPTY_NOTE: &str = "Loogle joins multiple terms with `and` semantics. Try separate searches if you want broader coverage.";

#[cfg(test)]
mod tests;
