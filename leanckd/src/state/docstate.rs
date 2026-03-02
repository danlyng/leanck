//! Generation- and version-bound elaboration snapshots and waiter notifications.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

pub use leanck_core::api::{
    Diagnostic as WireDiagnostic, Position as WirePosition, Range as WireRange,
};

/// Elaboration snapshot for one registered URI and document version.
#[derive(Debug, Clone)]
pub struct DocState {
    /// Latest version reported via `$/lean/fileProgress`.
    pub version: i64,
    /// Version registered by DocumentRegistry for this URI.
    pub doc_version: i64,
    /// Full diagnostic set from the latest matching publish.
    pub diagnostics: Vec<WireDiagnostic>,
    /// Ranges currently being elaborated.
    pub pending: Vec<WireRange>,
    /// True once we've seen at least one fileProgress for the current version.
    pub seen_progress: bool,
    /// True once a fileProgress for the current version arrived with empty processing.
    pub seen_completion: bool,
    /// Unversioned publishes cannot be matched to the pinned source.
    pub unversioned_diagnostics: bool,
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
            unversioned_diagnostics: false,
        }
    }
}

struct Inner {
    generation: u64,
    docs: HashMap<String, DocState>,
    notifiers: HashMap<String, Arc<Notify>>,
}

/// URI-keyed elaboration snapshots, isolated by Lean session and document version.
pub struct ElaborationStore {
    inner: Mutex<Inner>,
}

impl Default for ElaborationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ElaborationStore {
    pub fn new() -> Self {
        ElaborationStore {
            inner: Mutex::new(Inner {
                generation: 0,
                docs: HashMap::new(),
                notifiers: HashMap::new(),
            }),
        }
    }

    pub fn generation(&self) -> u64 {
        self.inner.lock().unwrap().generation
    }
    pub fn begin_generation(&self, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.generation = generation;
        inner.docs.clear();
        for notify in inner.notifiers.values() {
            notify.notify_waiters();
        }
    }
    pub fn begin_document(&self, generation: u64, uri: &str, version: i64) -> crate::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return Err(crate::Error::ServerDown(
                "Lean session changed; retry the query".into(),
            ));
        }
        let mut state = DocState::empty();
        state.doc_version = version;
        inner.docs.insert(uri.into(), state);
        Self::wake(&inner, uri);
        Ok(())
    }
    pub fn get_version(&self, generation: u64, uri: &str, version: i64) -> crate::Result<DocState> {
        Self::matching(&self.inner.lock().unwrap(), generation, uri, version).cloned()
    }
    pub fn verify_version(&self, generation: u64, uri: &str, version: i64) -> crate::Result<()> {
        Self::matching(&self.inner.lock().unwrap(), generation, uri, version).map(|_| ())
    }
    fn matching<'a>(
        inner: &'a Inner,
        generation: u64,
        uri: &str,
        version: i64,
    ) -> crate::Result<&'a DocState> {
        if inner.generation != generation {
            return Err(crate::Error::ServerDown(
                "Lean session changed; retry the query".into(),
            ));
        }
        match inner.docs.get(uri) {
            Some(state) if state.doc_version == version => Ok(state),
            _ => Err(crate::Error::ServerDown(
                "document version changed; retry the query".into(),
            )),
        }
    }
    pub fn diagnostics_for(
        &self,
        generation: u64,
        uri: &str,
        version: Option<i64>,
        diagnostics: Vec<WireDiagnostic>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return;
        }
        let Some(state) = inner.docs.get_mut(uri) else {
            return;
        };
        if version.is_none() {
            state.unversioned_diagnostics = true;
            Self::wake(&inner, uri);
            return;
        }
        if version != Some(state.doc_version) {
            return;
        }
        state.diagnostics = diagnostics;
        Self::wake(&inner, uri);
    }
    pub fn progress_for(
        &self,
        generation: u64,
        uri: &str,
        version: i64,
        processing: Vec<WireRange>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != generation {
            return;
        }
        let Some(state) = inner.docs.get_mut(uri) else {
            return;
        };
        if state.doc_version != version {
            return;
        }
        state.version = version;
        state.seen_progress = true;
        state.seen_completion = processing.is_empty();
        state.pending = processing;
        Self::wake(&inner, uri);
    }

    /// Snapshot the state for `uri`, if tracked.
    #[cfg(test)]
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
            start: WirePosition {
                line: sl,
                character: sc,
            },
            end: WirePosition {
                line: el,
                character: ec,
            },
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

    #[test]
    fn resets_state_before_notifications_can_arrive() {
        let s = ElaborationStore::new();
        s.begin_document(0, "u", 1).unwrap();
        s.diagnostics_for(0, "u", Some(1), vec![diag(r(0, 0, 0, 1), "first")]);
        s.progress_for(0, "u", 1, vec![]);
        assert_eq!(s.get_version(0, "u", 1).unwrap().diagnostics.len(), 1);
        s.begin_document(0, "u", 2).unwrap();
        let st = s.get_version(0, "u", 2).unwrap();
        assert!(!st.seen_completion);
        assert!(st.diagnostics.is_empty());
        assert!(s.get_version(0, "u", 1).is_err());
    }
    #[test]
    fn only_the_registered_version_and_generation_can_publish() {
        let s = ElaborationStore::new();
        s.begin_document(0, "u", 5).unwrap();
        for (g, v) in [(0, 4), (0, 6), (1, 5)] {
            s.diagnostics_for(g, "u", Some(v), vec![diag(r(0, 0, 0, 1), "stale")]);
            s.progress_for(g, "u", v, vec![]);
        }
        assert!(s.get("u").unwrap().diagnostics.is_empty());
        assert!(!s.get("u").unwrap().seen_progress);
        s.diagnostics_for(0, "u", Some(5), vec![diag(r(0, 0, 0, 1), "current")]);
        s.progress_for(0, "u", 5, vec![]);
        assert_eq!(s.get("u").unwrap().diagnostics[0].message, "current");
        assert!(s.get("u").unwrap().seen_completion);
        s.begin_generation(1);
        s.begin_document(1, "u", 5).unwrap();
        s.progress_for(0, "u", 5, vec![]);
        s.diagnostics_for(0, "u", Some(5), vec![diag(r(0, 0, 0, 1), "old process")]);
        assert!(!s.get("u").unwrap().seen_progress);
        assert!(s.get("u").unwrap().diagnostics.is_empty());
        assert!(s.get_version(0, "u", 5).is_err());
    }
    #[test]
    fn evicted_uris_are_not_recreated_by_late_notifications() {
        let s = ElaborationStore::new();
        s.begin_document(0, "u", 1).unwrap();
        s.forget("u");
        s.progress_for(0, "u", 1, vec![]);
        s.diagnostics_for(0, "u", Some(1), vec![]);
        assert!(s.get("u").is_none());
    }
    #[test]
    fn unversioned_diagnostics_are_an_explicit_quality_loss() {
        let s = ElaborationStore::new();
        s.begin_document(0, "u", 1).unwrap();
        s.diagnostics_for(0, "u", None, vec![diag(r(0, 0, 0, 1), "unknown version")]);
        let st = s.get("u").unwrap();
        assert!(st.unversioned_diagnostics);
        assert!(st.diagnostics.is_empty());
    }
    #[test]
    fn current_progress_can_become_pending_again() {
        let s = ElaborationStore::new();
        s.begin_document(0, "u", 1).unwrap();
        s.progress_for(0, "u", 1, vec![]);
        s.progress_for(0, "u", 1, vec![r(0, 0, 2, 0)]);
        assert!(!s.get("u").unwrap().seen_completion);
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
        assert!(overlaps_any(
            Some(&r(0, 0, 1, 0)),
            &[r(9, 0, 9, 0), r(1, 0, 2, 0)]
        ));
    }
}
