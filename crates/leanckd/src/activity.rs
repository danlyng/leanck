//! Monotonic idle accounting: queued and running requests keep the workspace busy.
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;
pub(crate) struct Activity {
    state: Mutex<(Instant, u64)>,
}
impl Activity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((Instant::now(), 0)),
        })
    }
    pub fn touch(&self) {
        self.state.lock().unwrap().0 = Instant::now();
    }
    pub fn begin(self: &Arc<Self>) -> Lease {
        let mut state = self.state.lock().unwrap();
        state.0 = Instant::now();
        state.1 += 1;
        Lease(self.clone())
    }
    pub fn active(&self) -> u64 {
        self.state.lock().unwrap().1
    }
    pub fn idle_for(&self) -> Duration {
        let state = self.state.lock().unwrap();
        if state.1 > 0 {
            Duration::ZERO
        } else {
            state.0.elapsed()
        }
    }
}
pub(crate) struct Lease(Arc<Activity>);
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.1 -= 1;
        state.0 = Instant::now();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn a_long_operation_is_not_idle_and_completion_resets_the_clock() {
        let activity = Activity::new();
        let lease = activity.begin();
        tokio::time::advance(Duration::from_secs(7200)).await;
        assert_eq!(activity.idle_for(), Duration::ZERO);
        drop(lease);
        assert_eq!(activity.active(), 0);
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(activity.idle_for(), Duration::from_secs(7));
    }
}
