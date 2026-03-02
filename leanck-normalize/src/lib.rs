//! Pure Lean source normalization, with an optional shared command adapter.
mod attributes;
mod configured;
mod frontmatter;
pub mod history;
mod prose;
mod reflow;
mod spelling;
mod syntax;
pub use configured::{Context, normalize_with_context};
pub use leanck_config::NormalizeRules;
pub use spelling::{Dictionary, builtin_json, configured_map};

#[cfg(feature = "command")]
pub mod command;
pub use frontmatter::Options as FrontmatterOptions;
pub use reflow::{NormalizeResult, Options, SpellingDirection, Stats};

pub const DEFAULT_WIDTH: usize = 100;

/// The comment delimiters recognized by normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// A `/-! ... -/` doc comment, including module docstrings and headings.
    Doc,
    /// A `/-- ... -/` block comment.
    Block,
}

/// A source normalization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeError {
    /// The requested target width is zero.
    InvalidWidth,
    InvalidConfiguration(String),
    /// A recognized opening delimiter has no closing `-/`.
    UnmatchedOpeningDelimiter {
        kind: CommentKind,
        /// One-based line in the source after frontmatter, attribute, and heading normalization.
        line: usize,
    },
    /// The complete error message returned by the Markdown reflow dependency.
    Reflow(String),
}

impl std::fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => f.write_str(message),
            Self::InvalidWidth => f.write_str("width must be greater than zero"),
            Self::UnmatchedOpeningDelimiter { kind, line } => {
                let tag = match kind {
                    CommentKind::Doc => "/-!",
                    CommentKind::Block => "/--",
                };
                write!(
                    f,
                    "unmatched opening delimiter '{tag}' at line {line} (no closing '-/')"
                )
            }
            Self::Reflow(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for NormalizeError {}

/// Transform source without filesystem, environment, or clock access.
/// The legacy year argument is retained for source compatibility; attribution
/// now requires file history through `normalize_with_context` and never uses it.
///
/// # Errors
///
/// Returns [`NormalizeError::InvalidWidth`] for a zero target width,
/// [`NormalizeError::UnmatchedOpeningDelimiter`] for an unclosed recognized
/// comment, or [`NormalizeError::Reflow`] with the dependency's error message.
pub fn normalize(
    source: &str,
    target_width: usize,
    options: Options,
    copyright_year: i32,
) -> Result<NormalizeResult, NormalizeError> {
    if target_width == 0 {
        return Err(NormalizeError::InvalidWidth);
    }
    reflow::normalize_with_stats(source, target_width, options, copyright_year)
}
