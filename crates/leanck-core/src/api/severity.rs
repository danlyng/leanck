//! Typed diagnostic severity selection for requests.
use super::Severity;
use serde::{Deserialize, Serialize};

/// Named severity levels selectable in a request. Response severities may also
/// contain unfamiliar numeric values, which are included only by `All`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl From<DiagnosticSeverity> for Severity {
    fn from(value: DiagnosticSeverity) -> Self {
        match value {
            DiagnosticSeverity::Error => Self::Error,
            DiagnosticSeverity::Warning => Self::Warning,
            DiagnosticSeverity::Information => Self::Information,
            DiagnosticSeverity::Hint => Self::Hint,
        }
    }
}

/// Omission defaults to error-only. An explicit empty selection matches nothing;
/// repeated levels do not change the selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeverityFilter {
    All,
    Only { severities: Vec<DiagnosticSeverity> },
}

impl<'de> Deserialize<'de> for SeverityFilter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Serde's internally tagged unit variants ignore extra fields even with
        // deny_unknown_fields. An empty struct variant enforces the same strict
        // shape as Only, so All cannot silently discard a severity selection.
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum Input {
            All {},
            Only { severities: Vec<DiagnosticSeverity> },
        }
        struct FilterVisitor;
        impl<'de> serde::de::Visitor<'de> for FilterVisitor {
            type Value = SeverityFilter;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a tagged severity filter object")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<Self::Value, M::Error> {
                Ok(
                    match Input::deserialize(serde::de::value::MapAccessDeserializer::new(map))? {
                        Input::All {} => SeverityFilter::All,
                        Input::Only { severities } => SeverityFilter::Only { severities },
                    },
                )
            }
        }
        deserializer.deserialize_map(FilterVisitor)
    }
}

impl Default for SeverityFilter {
    fn default() -> Self {
        Self::Only {
            severities: vec![DiagnosticSeverity::Error],
        }
    }
}

impl SeverityFilter {
    pub fn includes(&self, severity: Severity) -> bool {
        match self {
            Self::All => true,
            Self::Only { severities } => severities.iter().any(|s| Severity::from(*s) == severity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_match_exactly() {
        assert!(SeverityFilter::All.includes(Severity::Other(99)));
        assert!(SeverityFilter::default().includes(Severity::Error));
        assert!(!SeverityFilter::default().includes(Severity::Other(99)));
        let empty = SeverityFilter::Only { severities: vec![] };
        assert!(!empty.includes(Severity::Error));
        let warnings = SeverityFilter::Only {
            severities: vec![DiagnosticSeverity::Warning, DiagnosticSeverity::Warning],
        };
        assert!(warnings.includes(Severity::Warning));
        assert!(!warnings.includes(Severity::Error));
        assert!(!warnings.includes(Severity::Other(2)));
        for level in [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Information,
            DiagnosticSeverity::Hint,
        ] {
            assert!(
                SeverityFilter::Only {
                    severities: vec![level]
                }
                .includes(level.into())
            );
        }
    }
}
