//! Synchronous core of the elaboration state machine. Direct port of
//! `leanck-lsp/src/state.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WirePosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRange {
    pub start: WirePosition,
    pub end: WirePosition,
}

/// Diagnostic in our wire format. Mirrors the LSP `Diagnostic` so the Rust MCP
/// client can pass it through unchanged. Port of `state.ts:WireDiagnostic`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireDiagnostic {
    pub range: WireRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<serde_json::Value>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<i32>>,
    #[serde(
        rename = "relatedInformation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub related_information: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Filled by diagnostics enrichment (`$/lean/plainGoal` conclusions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goals: Option<Vec<String>>,
}

impl WireDiagnostic {
    /// Build a wire diagnostic from a raw LSP `Diagnostic` value, flattening a
    /// structured `code` (`{value, target}`) down to its `value`. Port of
    /// `state.ts:diagnosticToWire` (we receive raw LSP shape directly from the
    /// server, so the range/severity pass straight through).
    pub fn from_lsp(v: &serde_json::Value) -> Option<Self> {
        let range: WireRange = serde_json::from_value(v.get("range")?.clone()).ok()?;
        let severity = v.get("severity").and_then(|s| s.as_i64()).map(|n| n as i32);
        let code = match v.get("code") {
            Some(serde_json::Value::Object(o)) if o.contains_key("value") => o.get("value").cloned(),
            Some(other) => Some(other.clone()),
            None => None,
        };
        let message = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let tags = v.get("tags").and_then(|t| {
            t.as_array()
                .map(|a| a.iter().filter_map(|x| x.as_i64().map(|n| n as i32)).collect())
        });
        Some(WireDiagnostic {
            range,
            severity,
            code,
            message,
            tags,
            related_information: v.get("relatedInformation").cloned(),
            data: v.get("data").cloned(),
            goals: None,
        })
    }

    /// True when `code` equals the given string (e.g. Lean's `"hasSorry"`).
    pub fn code_is(&self, s: &str) -> bool {
        matches!(&self.code, Some(serde_json::Value::String(c)) if c == s)
    }
}

/// Per-URI elaboration state. Port of `state.ts:DocState`.
#[derive(Debug, Clone)]
pub struct DocState {
    /// Latest version reported via `$/lean/fileProgress`.
    pub version: i64,
    /// Latest VSCode/disk document version we've observed for this URI.
    pub doc_version: i64,
    /// Diagnostics merged across publishes for this URI.
    pub diagnostics: Vec<WireDiagnostic>,
    /// Ranges currently being elaborated.
    pub pending: Vec<WireRange>,
    /// True once we've seen at least one fileProgress for the current version.
    pub seen_progress: bool,
    /// True once a fileProgress for the current version arrived with empty processing.
    pub seen_completion: bool,
}

impl DocState {
    fn empty() -> Self {
        DocState {
            version: 0,
            doc_version: 0,
            diagnostics: Vec::new(),
            pending: Vec::new(),
            seen_progress: false,
            seen_completion: false,
        }
    }
}

struct Inner {
    docs: HashMap<String, DocState>,
    notifiers: HashMap<String, Arc<Notify>>,
}

/// URI-keyed store of elaboration state plus per-URI wakeups. Port of
/// `state.ts:StateStore`, with waiters expressed as a `tokio::Notify` per URI.
pub struct StateStore {
    inner: Mutex<Inner>,
}

impl Default for StateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StateStore {
    pub fn new() -> Self {
        StateStore {
            inner: Mutex::new(Inner {
                docs: HashMap::new(),
                notifiers: HashMap::new(),
            }),
        }
    }

    /// Snapshot the state for `uri`, if tracked.
    pub fn get(&self, uri: &str) -> Option<DocState> {
        self.inner.lock().unwrap().docs.get(uri).cloned()
    }

    /// The `Notify` a waiter should hold (and `enable()`) before checking the
    /// gate, so a state change between check and await can't be lost.
    pub fn notifier(&self, uri: &str) -> Arc<Notify> {
        let mut g = self.inner.lock().unwrap();
        g.notifiers
            .entry(uri.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    fn wake(g: &Inner, uri: &str) {
        if let Some(n) = g.notifiers.get(uri) {
            n.notify_waiters();
        }
    }

    /// Apply a `textDocument/publishDiagnostics`. Lean sends full sets, so each
    /// publish replaces the URI's diagnostics. A publish carrying a version older
    /// than the latest fileProgress version is a stale leftover from a previous
    /// edit's reporter and is dropped (a missing version always applies).
    pub fn apply_diagnostics(&self, uri: &str, version: Option<i64>, diagnostics: Vec<WireDiagnostic>) {
        let mut g = self.inner.lock().unwrap();
        let s = g.docs.entry(uri.to_string()).or_insert_with(DocState::empty);
        if version.is_some_and(|v| v < s.version) {
            return;
        }
        s.diagnostics = diagnostics;
        Self::wake(&g, uri);
    }

    /// Apply a `$/lean/fileProgress`. `processing` is the list of ranges still
    /// being elaborated; empty means completion. Drops stale lower versions and
    /// resets completion/pending/diagnostics on a higher version.
    pub fn apply_file_progress(&self, uri: &str, version: i64, processing: Vec<WireRange>) {
        let mut g = self.inner.lock().unwrap();
        let s = g.docs.entry(uri.to_string()).or_insert_with(DocState::empty);
        if version < s.version {
            return; // stale notification from a previous edit's reporter
        }
        if version > s.version {
            s.version = version;
            s.seen_completion = false;
            s.diagnostics.clear();
            s.pending.clear();
        }
        // A reported version we hadn't observed yet is still current — keep
        // doc_version in step so the wait gate doesn't deadlock.
        if version > s.doc_version {
            s.doc_version = version;
        }
        s.seen_progress = true;
        let empty = processing.is_empty();
        s.pending = processing;
        if empty {
            s.seen_completion = true;
        }
        Self::wake(&g, uri);
    }

    /// Record the latest observed document version. Only updates URIs we already
    /// track. Returns whether it advanced (and thus notified).
    pub fn observe_doc_version(&self, uri: &str, version: i64) -> bool {
        let mut g = self.inner.lock().unwrap();
        let Some(s) = g.docs.get_mut(uri) else {
            return false;
        };
        if version > s.doc_version {
            s.doc_version = version;
            Self::wake(&g, uri);
            true
        } else {
            false
        }
    }

    /// Mark a URI as known but not yet elaborated.
    pub fn touch(&self, uri: &str) {
        self.inner
            .lock()
            .unwrap()
            .docs
            .entry(uri.to_string())
            .or_insert_with(DocState::empty);
    }

    /// URIs currently tracked (for re-opening after a server respawn).
    pub fn tracked_uris(&self) -> Vec<String> {
        self.inner.lock().unwrap().docs.keys().cloned().collect()
    }

    /// Drop a URI's state entirely (e.g. after `didClose`).
    pub fn forget(&self, uri: &str) {
        let mut g = self.inner.lock().unwrap();
        g.docs.remove(uri);
        // Wake any waiters so they re-check and bail/re-loop rather than hang.
        Self::wake(&g, uri);
        g.notifiers.remove(uri);
    }
}

/// Do two ranges overlap (inclusive at touching endpoints)? Port of
/// `state.ts:rangeOverlaps`.
pub fn range_overlaps(a: &WireRange, b: &WireRange) -> bool {
    if a.end.line < b.start.line {
        return false;
    }
    if a.start.line > b.end.line {
        return false;
    }
    if a.end.line == b.start.line && a.end.character < b.start.character {
        return false;
    }
    if a.start.line == b.end.line && a.start.character > b.end.character {
        return false;
    }
    true
}

/// Does `range` overlap any pending range? A `None` range means "any pending".
/// Port of `state.ts:overlapsAny`.
pub fn overlaps_any(range: Option<&WireRange>, pending: &[WireRange]) -> bool {
    match range {
        None => !pending.is_empty(),
        Some(r) => pending.iter().any(|p| range_overlaps(r, p)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(sl: u32, sc: u32, el: u32, ec: u32) -> WireRange {
        WireRange {
            start: WirePosition { line: sl, character: sc },
            end: WirePosition { line: el, character: ec },
        }
    }

    fn diag(range: WireRange, message: &str) -> WireDiagnostic {
        WireDiagnostic {
            range,
            severity: None,
            code: None,
            message: message.to_string(),
            tags: None,
            related_information: None,
            data: None,
            goals: None,
        }
    }

    // ---- applyFileProgress ----

    #[test]
    fn records_progress_and_completes_on_empty() {
        let s = StateStore::new();
        s.apply_file_progress("u", 1, vec![r(0, 0, 5, 0)]);
        let st = s.get("u").unwrap();
        assert_eq!(st.version, 1);
        assert!(st.seen_progress);
        assert!(!st.seen_completion);
        assert_eq!(st.pending.len(), 1);

        s.apply_file_progress("u", 1, vec![]);
        let st = s.get("u").unwrap();
        assert!(st.seen_completion);
        assert_eq!(st.pending.len(), 0);
    }

    #[test]
    fn drops_stale_lower_version() {
        let s = StateStore::new();
        s.apply_file_progress("u", 5, vec![]);
        s.apply_file_progress("u", 3, vec![r(0, 0, 1, 0)]);
        let st = s.get("u").unwrap();
        assert_eq!(st.version, 5);
        assert!(st.seen_completion);
        assert_eq!(st.pending.len(), 0);
    }

    #[test]
    fn resets_completion_and_clears_diags_on_higher_version() {
        let s = StateStore::new();
        s.apply_file_progress("u", 1, vec![]);
        s.apply_diagnostics("u", Some(1), vec![diag(r(0, 0, 0, 1), "boom")]);
        s.apply_file_progress("u", 2, vec![r(0, 0, 9, 0)]);
        let st = s.get("u").unwrap();
        assert_eq!(st.version, 2);
        assert!(!st.seen_completion);
        assert_eq!(st.diagnostics.len(), 0);
    }

    #[test]
    fn keeps_docversion_in_step_with_reported_version() {
        let s = StateStore::new();
        s.apply_file_progress("u", 4, vec![]);
        assert_eq!(s.get("u").unwrap().doc_version, 4);
    }

    // ---- observeDocVersion ----

    #[test]
    fn observe_ignores_untracked() {
        let s = StateStore::new();
        assert!(!s.observe_doc_version("untracked", 3));
        assert!(s.get("untracked").is_none());
    }

    #[test]
    fn observe_raises_ahead_of_reported_version() {
        let s = StateStore::new();
        s.apply_file_progress("u", 1, vec![]);
        assert!(s.observe_doc_version("u", 2));
        let st = s.get("u").unwrap();
        assert_eq!(st.doc_version, 2);
        assert_eq!(st.version, 1);
    }

    #[test]
    fn observe_never_lowers() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 7);
        s.observe_doc_version("u", 3);
        assert_eq!(s.get("u").unwrap().doc_version, 7);
    }

    #[test]
    fn observe_returns_false_when_unchanged() {
        let s = StateStore::new();
        s.touch("u");
        assert!(s.observe_doc_version("u", 5));
        assert!(!s.observe_doc_version("u", 4));
    }

    // ---- diagnostics ----

    #[test]
    fn replaces_diagnostic_set_each_publish() {
        let s = StateStore::new();
        s.apply_diagnostics("u", None, vec![diag(r(0, 0, 0, 1), "a")]);
        s.apply_diagnostics("u", None, vec![diag(r(1, 0, 1, 1), "b")]);
        let diags = s.get("u").unwrap().diagnostics;
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "b");
    }

    #[test]
    fn drops_stale_versioned_publish() {
        let s = StateStore::new();
        s.apply_file_progress("u", 2, vec![r(0, 0, 9, 0)]);
        s.apply_diagnostics("u", Some(2), vec![diag(r(0, 0, 0, 1), "current")]);
        // Leftover publish from the v1 reporter must not clobber v2's set.
        s.apply_diagnostics("u", Some(1), vec![]);
        let diags = s.get("u").unwrap().diagnostics;
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "current");
        // An unversioned publish still applies.
        s.apply_diagnostics("u", None, vec![]);
        assert!(s.get("u").unwrap().diagnostics.is_empty());
    }

    // ---- overlap helpers ----

    #[test]
    fn range_overlaps_detects_line_and_char_overlap() {
        assert!(range_overlaps(&r(0, 0, 2, 0), &r(1, 0, 3, 0)));
        assert!(!range_overlaps(&r(0, 0, 1, 0), &r(2, 0, 3, 0)));
        assert!(range_overlaps(&r(0, 0, 1, 5), &r(1, 5, 1, 9)));
        assert!(!range_overlaps(&r(0, 0, 1, 4), &r(1, 5, 1, 9)));
    }

    #[test]
    fn overlaps_any_treats_none_as_any_pending() {
        assert!(!overlaps_any(None, &[]));
        assert!(overlaps_any(None, &[r(0, 0, 1, 0)]));
        assert!(!overlaps_any(Some(&r(5, 0, 6, 0)), &[r(0, 0, 1, 0)]));
        assert!(overlaps_any(Some(&r(0, 0, 1, 0)), &[r(9, 0, 9, 0), r(1, 0, 2, 0)]));
    }

    // ---- diagnostic_from_lsp (diagnosticToWire) ----

    #[test]
    fn from_lsp_flattens_nested_code_and_preserves_range() {
        let wire = WireDiagnostic::from_lsp(&serde_json::json!({
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 3, "character": 4}},
            "severity": 1,
            "code": {"value": "hasSorry", "target": "x"},
            "message": "oops",
        }))
        .unwrap();
        assert_eq!(wire.code, Some(serde_json::json!("hasSorry")));
        assert!(wire.code_is("hasSorry"));
        assert_eq!(wire.severity, Some(1));
        assert_eq!(wire.range, r(1, 2, 3, 4));
    }

    #[test]
    fn from_lsp_passes_primitive_code_through() {
        let wire = WireDiagnostic::from_lsp(&serde_json::json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "code": 42,
            "message": "m",
        }))
        .unwrap();
        assert_eq!(wire.code, Some(serde_json::json!(42)));
        assert_eq!(wire.severity, None);
    }
}
