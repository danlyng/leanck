//! Shared identities and versioned protocol data, without daemon or frontend dependencies.
//! Optional `wire` support adds async private-socket framing.
pub mod api;
pub mod document;
pub mod error;
pub mod instances;
pub mod paths;
pub mod workspace;

#[cfg(feature = "wire")]
pub mod wire;
