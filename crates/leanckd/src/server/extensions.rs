//! Typed Lean extensions and the extension-aware diagnostic notification.
use crate::state::{WireDiagnostic, WireRange};
use leanck_core::api::GoalResult;
use lsp_types::{
    TextDocumentItem, TextDocumentPositionParams, Url, notification::Notification, request::Request,
};
use serde::{Deserialize, Serialize};

pub enum PlainGoal {}
impl Request for PlainGoal {
    type Params = TextDocumentPositionParams;
    type Result = GoalResult;
    const METHOD: &'static str = "$/lean/plainGoal";
}
pub enum WaitForDiagnostics {}
impl Request for WaitForDiagnostics {
    type Params = DiagnosticsVersion;
    type Result = Option<DiagnosticsReady>;
    const METHOD: &'static str = "textDocument/waitForDiagnostics";
}
#[derive(Debug, Serialize, Deserialize)]
pub struct DiagnosticsVersion {
    pub uri: Url,
    pub version: i64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct DiagnosticsReady {}

pub enum OpenDocument {}
impl Notification for OpenDocument {
    type Params = OpenParams;
    const METHOD: &'static str = "textDocument/didOpen";
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenParams {
    pub text_document: TextDocumentItem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_build_mode: Option<DependencyBuildMode>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyBuildMode {
    Once,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub uri: Url,
    pub version: Option<i64>,
    pub diagnostics: Vec<WireDiagnostic>,
}
impl Diagnostics {
    pub fn normalize(mut self) -> Self {
        for diagnostic in &mut self.diagnostics {
            // Lean sometimes wraps its diagnostic code in {value: ...}.
            if let Some(serde_json::Value::Object(code)) = &diagnostic.code {
                if let Some(value) = code.get("value") {
                    diagnostic.code = Some(value.clone());
                }
            }
            diagnostic.goals = None;
        }
        self
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProgress {
    pub text_document: ProgressDocument,
    #[serde(default)]
    pub processing: Vec<Processing>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ProgressDocument {
    pub uri: Url,
    #[serde(default)]
    pub version: i64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Processing {
    pub range: WireRange,
}

pub enum ProgressNotification {}
impl Notification for ProgressNotification {
    type Params = FileProgress;
    const METHOD: &'static str = "$/lean/fileProgress";
}

/// Lean versions may omit selectionRange; retain the established range fallback.
pub enum DocumentSymbols {}
impl Request for DocumentSymbols {
    type Params = lsp_types::DocumentSymbolParams;
    type Result = Option<SymbolResponse>;
    const METHOD: &'static str = <lsp_types::request::DocumentSymbolRequest as Request>::METHOD;
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SymbolResponse {
    Flat(Vec<lsp_types::SymbolInformation>),
    Nested(Vec<DocumentSymbol>),
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: lsp_types::SymbolKind,
    pub range: lsp_types::Range,
    pub selection_range: Option<lsp_types::Range>,
    pub children: Option<Vec<DocumentSymbol>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_diagnostics_preserve_codes_ranges_nullable_fields_and_extension_data() {
        for code in [
            serde_json::json!({"value":"hasSorry"}),
            serde_json::json!(42),
        ] {
            let params: Diagnostics = serde_json::from_value(serde_json::json!({"uri":"file:///tmp/test.lean", "version":3, "diagnostics":[{"range":{"start":{"line":1,"character":2},"end":{"line":4,"character":5}}, "severity":25, "code":code, "message":"test", "tags":[1,2], "relatedInformation":null, "data":{"lean":true}}]})).unwrap();
            let params = params.normalize();
            let diagnostic = &params.diagnostics[0];
            assert_eq!(diagnostic.range.start.line, 1);
            assert_eq!(diagnostic.range.end.character, 5);
            assert_eq!(diagnostic.related_information, Some(None));
            assert_eq!(diagnostic.data, Some(serde_json::json!({"lean":true})));
            assert_eq!(i32::from(diagnostic.severity.unwrap()), 25);
            assert_eq!(
                diagnostic.code,
                Some(code.get("value").unwrap_or(&code).clone())
            );
        }
    }
}
