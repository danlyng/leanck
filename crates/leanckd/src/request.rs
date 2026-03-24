//! One absolute operation budget, shared by admission, query helpers, and LSP.
use crate::error::{Error, Result};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct RequestContext {
    deadline: Instant,
    cancel: CancellationToken,
    wait_override: Option<Duration>,
}
impl RequestContext {
    pub fn new(timeout_ms: Option<u64>, cancel: CancellationToken) -> Result<Self> {
        Self::with_default(timeout_ms, cancel, DEFAULT_REQUEST_TIMEOUT)
    }
    pub fn with_default(
        timeout_ms: Option<u64>,
        cancel: CancellationToken,
        default: Duration,
    ) -> Result<Self> {
        let wait_override = timeout_ms.map(Duration::from_millis);
        let deadline = Instant::now()
            .checked_add(wait_override.unwrap_or(default))
            .ok_or_else(|| Error::BadRequest("timeout_ms is too large".into()))?;
        Ok(Self {
            deadline,
            cancel,
            wait_override,
        })
    }
    pub fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }
    pub fn remaining(&self) -> Result<Duration> {
        self.check()?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
    pub fn wait_budget(&self, default: Duration) -> Result<Duration> {
        Ok(self.remaining()?.min(self.wait_override.unwrap_or(default)))
    }
    pub async fn run<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        self.check()?;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(Error::Cancelled),
            _ = tokio::time::sleep_until(self.deadline) => Err(Error::Timeout),
            result = work => { self.check()?; result }
        }
    }
    /// Optional enrichment may fail locally, but cannot hide operation expiry.
    pub fn optional<T>(&self, value: Result<T>) -> Result<Option<T>> {
        self.check()?;
        match value {
            Err(Error::Cancelled) => Err(Error::Cancelled),
            other => Ok(other.ok()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn successive_steps_share_one_budget() {
        let ctx = RequestContext::new(Some(100), CancellationToken::new()).unwrap();
        ctx.run(async {
            tokio::time::sleep(Duration::from_millis(70)).await;
            Ok(())
        })
        .await
        .unwrap();
        assert!(ctx.remaining().unwrap() <= Duration::from_millis(30));
        assert!(matches!(
            ctx.run(async {
                tokio::time::sleep(Duration::from_millis(40)).await;
                Ok(())
            })
            .await,
            Err(Error::Timeout)
        ));
        assert!(matches!(ctx.optional::<()>(Ok(())), Err(Error::Timeout)));
    }
    #[tokio::test]
    async fn cancellation_interrupts_a_queued_operation() {
        let token = CancellationToken::new();
        let ctx = RequestContext::new(None, token.clone()).unwrap();
        token.cancel();
        assert!(matches!(
            ctx.run(std::future::pending::<Result<()>>()).await,
            Err(Error::Cancelled)
        ));
    }
}
