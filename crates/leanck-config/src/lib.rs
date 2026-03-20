//! Shared configuration schema and optional filesystem loader.
mod model;
mod rules;
mod warnings;
mod width;
pub use model::*;
pub use rules::*;
pub use warnings::*;
pub use width::NormalizeWidth;
#[cfg(feature = "loader")]
mod loader;
#[cfg(feature = "wizard")]
pub mod wizard;
#[cfg(feature = "loader")]
pub use loader::*;
#[cfg(all(test, feature = "loader"))]
mod tests;
