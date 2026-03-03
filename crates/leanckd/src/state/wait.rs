//! Async wait-for-elaboration gate. Port of `leanck-lsp/src/waitFor.ts`.

use std::time::Duration;

use tokio::time::Instant;

use super::docstate::{ElaborationStore, WireDiagnostic, WireRange, overlaps_any, range_overlaps};
use crate::error::{Error, Result};

/// A pinned query cannot silently move to another version or session.
pub async fn wait_for_version(
    store: &ElaborationStore,
    generation: u64,
    uri: &str,
    version: i64,
    range: Option<&WireRange>,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let notify = store.notifier(uri);
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let state = store.get_version(generation, uri, version)?;
        if state.seen_progress
            && state.seen_completion
            && state.version == version
            && !overlaps_any(range, &state.pending)
        {
            return Ok(());
        }
        tokio::select! {_=&mut notified=>{},_=tokio::time::sleep_until(deadline)=>return Err(Error::Timeout)}
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

    fn begin(s: &ElaborationStore) {
        s.begin_document(0, "u", 1).unwrap();
    }
    async fn wait(s: &ElaborationStore, range: Option<&WireRange>) -> Result<()> {
        wait_for_version(s, 0, "u", 1, range, Duration::from_secs(1)).await
    }
    #[tokio::test]
    async fn returns_complete_matching_snapshot() {
        let s = ElaborationStore::new();
        begin(&s);
        s.diagnostics_for(
            0,
            "u",
            Some(1),
            vec![diag(r(0, 0, 0, 2), "in"), diag(r(5, 0, 5, 2), "out")],
        );
        s.progress_for(0, "u", 1, vec![]);
        wait(&s, Some(&r(0, 0, 2, 0))).await.unwrap();
        let result = s.get_version(0, "u", 1).unwrap();
        assert_eq!(result.version, 1);
        assert_eq!(
            filter_by_range(&result.diagnostics, Some(&r(0, 0, 2, 0))).len(),
            1
        );
    }
    #[tokio::test]
    async fn blocks_for_current_completion_and_wakes_all_waiters() {
        let s = std::sync::Arc::new(ElaborationStore::new());
        begin(&s);
        s.progress_for(0, "u", 1, vec![r(0, 0, 10, 0)]);
        let first = s.clone();
        let second = s.clone();
        let a = tokio::spawn(async move { wait(&first, None).await });
        let b = tokio::spawn(async move { wait(&second, None).await });
        tokio::task::yield_now().await;
        assert!(!a.is_finished());
        assert!(!b.is_finished());
        s.progress_for(0, "u", 1, vec![]);
        a.await.unwrap().unwrap();
        b.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn recovery_wakes_and_fails_a_pinned_wait() {
        let s = std::sync::Arc::new(ElaborationStore::new());
        begin(&s);
        let copy = s.clone();
        let waiter = tokio::spawn(async move { wait(&copy, None).await });
        tokio::task::yield_now().await;
        s.begin_generation(1);
        assert!(matches!(waiter.await.unwrap(), Err(Error::ServerDown(_))));
    }
    #[tokio::test]
    async fn replacement_version_fails_instead_of_following_an_edit() {
        let s = ElaborationStore::new();
        begin(&s);
        s.begin_document(0, "u", 2).unwrap();
        s.progress_for(0, "u", 2, vec![]);
        assert!(matches!(wait(&s, None).await, Err(Error::ServerDown(_))));
    }
    #[tokio::test(start_paused = true)]
    async fn timeout_does_not_accept_stale_progress() {
        let s = ElaborationStore::new();
        begin(&s);
        s.progress_for(0, "u", 0, vec![]);
        assert!(matches!(wait(&s, None).await, Err(Error::Timeout)));
    }
}
