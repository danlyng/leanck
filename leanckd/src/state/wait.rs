//! Async wait-for-elaboration gate. Port of `leanck-lsp/src/waitFor.ts`.

use std::time::Duration;

use tokio::time::Instant;

use super::docstate::{overlaps_any, range_overlaps, StateStore, WireDiagnostic, WireRange};
use crate::error::{Error, Result};

pub struct WaitResult {
    pub version: i64,
    pub diagnostics: Vec<WireDiagnostic>,
}

/// Block until the requested range (or the whole file, when `range` is `None`)
/// is no longer pending elaboration, then return the merged diagnostics.
///
/// "Done" means: a fileProgress for the current version arrived (`seen_progress`
/// && `seen_completion`), Lean's reported `version` has caught up to the
/// observed `doc_version`, and no remaining `pending` range overlaps `range`.
/// On re-edit, [`StateStore::apply_file_progress`] resets completion for the new
/// version, so waiters re-loop. Returns [`Error::Timeout`] past the deadline.
pub async fn wait_for_range(
    store: &StateStore,
    uri: &str,
    range: Option<&WireRange>,
    timeout: Duration,
) -> Result<WaitResult> {
    let deadline = Instant::now() + timeout;
    let notify = store.notifier(uri);
    loop {
        // Register interest BEFORE checking the gate so a state change between
        // the check and the await is not lost (Notify::notify_waiters only wakes
        // already-enabled waiters).
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(res) = try_complete(store, uri, range) {
            return Ok(res);
        }

        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => return Err(Error::Timeout),
        }
    }
}

/// Check the completion gate once; on success return the version + range-filtered
/// diagnostics. Holds the store lock only synchronously (never across `.await`).
fn try_complete(store: &StateStore, uri: &str, range: Option<&WireRange>) -> Option<WaitResult> {
    let s = store.get(uri)?;
    if s.seen_progress
        && s.seen_completion
        && s.version >= s.doc_version
        && !overlaps_any(range, &s.pending)
    {
        Some(WaitResult {
            version: s.version,
            diagnostics: filter_by_range(&s.diagnostics, range),
        })
    } else {
        None
    }
}

/// Keep only the diagnostics overlapping `range` (`None` = keep all).
pub fn filter_by_range(diags: &[WireDiagnostic], range: Option<&WireRange>) -> Vec<WireDiagnostic> {
    match range {
        None => diags.to_vec(),
        Some(r) => diags
            .iter()
            .filter(|d| range_overlaps(&d.range, r))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::docstate::{WireDiagnostic, WirePosition};

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

    /// Drive a URI to fully-elaborated/complete at `version`.
    fn complete(s: &StateStore, uri: &str, version: i64, diags: Vec<WireDiagnostic>) {
        s.apply_file_progress(uri, version, vec![r(0, 0, 100, 0)]);
        s.apply_diagnostics(uri, Some(version), diags);
        s.apply_file_progress(uri, version, vec![]);
    }

    #[tokio::test]
    async fn returns_immediately_when_already_complete() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 1);
        complete(&s, "u", 1, vec![]);
        let res = wait_for_range(&s, "u", None, Duration::from_secs(1)).await.unwrap();
        assert_eq!(res.version, 1);
        assert!(res.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn blocks_until_completion_then_resolves() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 1);
        let fut = wait_for_range(&s, "u", None, Duration::from_secs(1));
        s.apply_file_progress("u", 1, vec![r(0, 0, 10, 0)]);
        s.apply_file_progress("u", 1, vec![]);
        let res = fut.await.unwrap();
        assert_eq!(res.version, 1);
    }

    #[tokio::test]
    async fn does_not_return_stale_complete_when_docversion_ahead() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 1);
        complete(&s, "u", 1, vec![]);
        s.observe_doc_version("u", 2); // edit landed; Lean still at v1

        // Must still be waiting.
        let early = tokio::time::timeout(
            Duration::from_millis(50),
            wait_for_range(&s, "u", None, Duration::from_secs(10)),
        )
        .await;
        assert!(early.is_err(), "should not have resolved with stale completion");

        // Lean catches up to v2.
        complete(&s, "u", 2, vec![]);
        let res = wait_for_range(&s, "u", None, Duration::from_secs(1)).await.unwrap();
        assert_eq!(res.version, 2);
    }

    #[tokio::test]
    async fn filters_diagnostics_to_overlapping_range() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 1);
        s.apply_file_progress("u", 1, vec![r(0, 0, 100, 0)]);
        s.apply_diagnostics(
            "u",
            Some(1),
            vec![diag(r(1, 0, 1, 5), "in"), diag(r(50, 0, 50, 5), "out")],
        );
        s.apply_file_progress("u", 1, vec![]);

        let res = wait_for_range(&s, "u", Some(&r(0, 0, 3, 0)), Duration::from_secs(1))
            .await
            .unwrap();
        let msgs: Vec<_> = res.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(msgs, vec!["in"]);
    }

    #[tokio::test]
    async fn keeps_waiting_while_pending_overlaps_query() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 1);
        s.apply_file_progress("u", 1, vec![r(2, 0, 4, 0)]);

        let early = tokio::time::timeout(
            Duration::from_millis(50),
            wait_for_range(&s, "u", Some(&r(3, 0, 3, 0)), Duration::from_secs(10)),
        )
        .await;
        assert!(early.is_err());

        s.apply_file_progress("u", 1, vec![]);
        let res = wait_for_range(&s, "u", Some(&r(3, 0, 3, 0)), Duration::from_secs(1)).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn times_out_when_never_completes() {
        let s = StateStore::new();
        s.touch("u");
        s.observe_doc_version("u", 1);
        s.apply_file_progress("u", 1, vec![r(0, 0, 1, 0)]); // never empties
        let res = wait_for_range(&s, "u", None, Duration::from_millis(80)).await;
        assert!(matches!(res, Err(Error::Timeout)));
    }
}
