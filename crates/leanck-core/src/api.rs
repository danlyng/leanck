//! Private daemon protocol data, shared by all clients.

use crate::error::WireError;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

mod kinds;
pub use kinds::{DeclarationKind, KindSelector, LeanDeclarationKind, LspSymbolKind};
mod models;
pub use models::*;
mod severity;
pub use severity::{DiagnosticSeverity, SeverityFilter};
mod command;
pub use command::{Command, Operation, RequestError};

/// Change only when the private wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 4;

/// A request from a client. `op` selects the handler; `params` is the
/// op-specific payload (deserialized per-op by the daemon).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub op: String,
    /// Per-request deadline. `None` means use the daemon default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// A response to a [`Request`], correlated by `id`.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
}

// Preserve explicit result:null as Some(Null), distinct from a missing field.
fn present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            id: String,
            ok: bool,
            #[serde(default, deserialize_with = "present_option")]
            result: Option<serde_json::Value>,
            #[serde(default)]
            error: Option<WireError>,
        }
        let fields = Fields::deserialize(deserializer)?;
        match (fields.ok, fields.result.is_some(), fields.error.is_some()) {
            (true, true, false) | (false, false, true) => Ok(Self {
                id: fields.id,
                ok: fields.ok,
                result: fields.result,
                error: fields.error,
            }),
            _ => Err(D::Error::custom(
                "response must contain exactly a success result or a failure error",
            )),
        }
    }
}

impl Response {
    pub fn ok(id: String, result: serde_json::Value) -> Self {
        Response {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn into_result(
        self,
        expected_id: &str,
    ) -> Result<Result<serde_json::Value, WireError>, crate::error::ProtocolError> {
        if self.id != expected_id {
            return Err(crate::error::ProtocolError::MismatchedId {
                expected: expected_id.into(),
                actual: self.id,
            });
        }
        match (self.ok, self.result, self.error) {
            (true, Some(result), None) => Ok(Ok(result)),
            (false, None, Some(error)) => Ok(Err(error)),
            _ => Err(crate::error::ProtocolError::InconsistentEnvelope),
        }
    }

    pub fn err(id: String, error: WireError) -> Self {
        Response {
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

// ---- op parameter payloads ------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosticsParams {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ident: Option<String>,
    /// Typed severity selection. Omission defaults to error-only; an explicit
    /// empty selection matches nothing.
    #[serde(default)]
    pub severity: SeverityFilter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalParams {
    pub file: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SorryParams {
    /// Either a file or a directory (recursive scan).
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListParams {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Omitted or empty selects all; otherwise selectors are combined with OR.
    pub kind: Option<Vec<KindSelector>>,
}

impl ListParams {
    pub fn includes_kind(&self, kind: DeclarationKind) -> bool {
        self.kind.as_ref().is_none_or(|selectors| {
            selectors.is_empty() || selectors.iter().any(|selector| selector.matches(kind))
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RestartFileParams {
    pub file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferencesParams {
    /// File containing the declaration whose references are wanted.
    pub file: String,
    /// The declaration identifier (e.g. `Foo.bar` or `bar`).
    pub ident: String,
    /// Optional namespace to qualify a bare `ident` against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CancelParams {
    pub target_id: String,
}

/// Op names. Kept as `&str` constants so both sides agree on the wire string.
pub mod op {
    pub const HEALTH: &str = "health";
    pub const DIAGNOSTICS: &str = "diagnostics";
    pub const GOAL: &str = "goal";
    pub const SORRY: &str = "sorry";
    pub const LIST: &str = "list";
    pub const REFERENCES: &str = "references";
    pub const RESTART_FILE: &str = "restart_file";
    pub const CANCEL: &str = "cancel";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
}
