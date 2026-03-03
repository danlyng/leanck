//! Monotonic idle accounting: queued and running requests keep the workspace busy.
//!
//! Admission closes once, under the same mutex that counts leases, so a
//! conditional shutdown (`close_if_idle`) can never race a request that was
//! just admitted: either the request's lease exists and the close fails, or
//! the close wins and the request is refused.
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

struct State {
    last_active: Instant,
    active: u64,
    closed: bool,
}

pub(crate) struct Activity {
    state: Mutex<State>,
}
impl Activity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                last_active: Instant::now(),
                active: 0,
                closed: false,
            }),
        })
    }
    pub fn touch(&self) {
        self.state.lock().unwrap().last_active = Instant::now();
    }
    /// Admit one request, or `None` once admission is closed.
    pub fn begin(self: &Arc<Self>) -> Option<Lease> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return None;
        }
        state.last_active = Instant::now();
        state.active += 1;
        Some(Lease(self.clone()))
    }
    /// Close admission unconditionally.
    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
    }
    /// Close admission only when nothing is queued or running and admission is
    /// still open. Returns whether this call closed it.
    pub fn close_if_idle(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.active == 0 && !state.closed {
            state.closed = true;
            true
        } else {
            false
        }
    }
    /// Whether admission is closed. Closing is permanent.
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
    pub fn active(&self) -> u64 {
        self.state.lock().unwrap().active
    }
    pub fn idle_for(&self) -> Duration {
        let state = self.state.lock().unwrap();
        if state.active > 0 {
            Duration::ZERO
        } else {
            state.last_active.elapsed()
        }
    }
}
pub(crate) struct Lease(Arc<Activity>);
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.active -= 1;
        state.last_active = Instant::now();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn a_long_operation_is_not_idle_and_completion_resets_the_clock() {
        let activity = Activity::new();
        let lease = activity.begin().unwrap();
        tokio::time::advance(Duration::from_secs(7200)).await;
        assert_eq!(activity.idle_for(), Duration::ZERO);
        drop(lease);
        assert_eq!(activity.active(), 0);
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(activity.idle_for(), Duration::from_secs(7));
    }

    #[test]
    fn close_if_idle_waits_for_leases_and_then_refuses_admission() {
        let activity = Activity::new();
        let lease = activity.begin().unwrap();
        assert!(!activity.close_if_idle());
        // A failed conditional close leaves admission open.
        let second = activity.begin().unwrap();
        drop((lease, second));
        assert!(activity.close_if_idle());
        assert!(activity.begin().is_none());
        assert_eq!(activity.active(), 0);
        // Closing is once only.
        assert!(!activity.close_if_idle());
        assert!(activity.is_closed());
    }

    #[test]
    fn unconditional_close_refuses_admission_while_work_drains() {
        let activity = Activity::new();
        let lease = activity.begin().unwrap();
        activity.close();
        assert!(activity.begin().is_none());
        assert_eq!(activity.active(), 1);
        drop(lease);
        assert_eq!(activity.active(), 0);
        assert!(!activity.close_if_idle());
    }
}
