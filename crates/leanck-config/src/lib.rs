//! Shared configuration schema and optional filesystem/CLI loader.
mod model;
mod rules;
mod warnings;
pub use model::*;
pub use rules::*;
pub use warnings::*;
#[cfg(feature = "loader")]
mod loader;
#[cfg(feature = "wizard")]
mod wizard;
#[cfg(feature = "loader")]
pub use loader::*;
#[cfg(all(test, feature = "loader"))]
mod tests;
