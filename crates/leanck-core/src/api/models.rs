//! Current private-protocol operation results. Lean's nullable fields and extension
//! metadata retain their external wire representation.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// LSP's numeric representation is retained, including unfamiliar values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "i32", into = "i32")]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
    Other(i32),
}

impl From<i32> for Severity {
    fn from(value: i32) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            4 => Self::Hint,
            other => Self::Other(other),
        }
    }
}

impl From<Severity> for i32 {
    fn from(value: Severity) -> Self {
        match value {
            Severity::Error => 1,
            Severity::Warning => 2,
            Severity::Information => 3,
            Severity::Hint => 4,
            Severity::Other(other) => other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedInformation {
    pub location: Location,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// Lean codes may be strings, numbers, or extension values. Preserve null.
    #[serde(
        default,
        deserialize_with = "super::present_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub code: Option<Value>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<i32>>,
    /// Outer None is omitted; Some(None) is an explicit null.
    #[serde(
        rename = "relatedInformation",
        default,
        deserialize_with = "super::present_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub related_information: Option<Option<Vec<RelatedInformation>>>,
    #[serde(
        default,
        deserialize_with = "super::present_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goals: Option<Vec<String>>,
}

impl Diagnostic {
    pub fn code_is(&self, code: &str) -> bool {
        self.code.as_ref().and_then(Value::as_str) == Some(code)
    }

    pub fn severity_or_error(&self) -> Severity {
        self.severity.unwrap_or(Severity::Error)
    }
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticsResult {
    pub uri: String,
    #[serde(deserialize_with = "required_nullable")]
    pub range: Option<Range>,
    #[serde(deserialize_with = "required_nullable")]
    pub symbol: Option<String>,
    pub version: i64,
    pub total: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub source: String,
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GoalResult {
    NoGoal,
    Goals(Goals),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goals {
    pub goals: Vec<String>,
    /// Lean's plainGoal extension may include additional metadata.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SorryKind {
    Declaration,
    Tactic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SorryEntry {
    pub range: Range,
    pub kind: SorryKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SorryFile {
    pub uri: String,
    pub sorries: Vec<SorryEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SorryDirectory {
    pub uri: String,
    pub total: u64,
    pub files: Vec<SorryFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SorryResult {
    File(SorryFile),
    Directory(SorryDirectory),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedSymbol {
    pub name: String,
    #[serde(rename = "fullName")]
    pub full_name: String,
    pub kind: super::DeclarationKind,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListResult {
    pub uri: String,
    pub symbols: Vec<ListedSymbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferencesResult {
    pub uri: String,
    pub symbol: String,
    pub locations: Vec<Location>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestartFileResult {
    pub uri: String,
    pub restarted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerHealth {
    #[serde(deserialize_with = "required_nullable")]
    pub pid: Option<u32>,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthResult {
    pub ok: bool,
    pub workspace: String,
    pub protocol_version: u32,
    pub server: ServerHealth,
    pub idle_secs: u64,
    pub lifecycle: LifecycleHealth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShutdownResult {
    pub shutting_down: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelResult {
    pub cancelled: String,
}

/// Daemon lifecycle state and queued/running request count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleHealth {
    pub state: LifecycleState,
    pub active_requests: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Ready,
    Recovering,
    ShuttingDown,
}
