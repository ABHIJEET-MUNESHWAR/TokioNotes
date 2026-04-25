//! Reusable resilience combinators: timeout, retry with exponential
//! backoff, and a tiny circuit breaker. Generic over future output.

use crate::error::{AppError, AppResult};
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::Retry;

pub async fn with_timeout<F, T>(d: Duration, fut: F) -> AppResult<T>
where
    F: Future<Output = AppResult<T>>,
{
    match tokio::time::timeout(d, fut).await {
        Ok(r) => r,
        Err(_) => Err(AppError::Timeout(format!("after {:?}", d))),
    }
}

pub async fn with_retry<F, Fut, T>(max_attempts: usize, op: F) -> AppResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = AppResult<T>>,
{
    let strategy = ExponentialBackoff::from_millis(20)
        .factor(2)
        .max_delay(Duration::from_millis(500))
        .map(jitter)
        .take(max_attempts.saturating_sub(1));
    Retry::spawn(strategy, op).await
}

#[derive(Clone)]
pub struct CircuitBreaker {
    state: Arc<AtomicU8>, // 0 closed, 1 open, 2 half-open
    failures: Arc<AtomicU64>,
    threshold: u64,
    cooldown_ms: u64,
    opened_at: Arc<AtomicU64>,
}

impl CircuitBreaker {
    pub fn new(threshold: u64, cooldown: Duration) -> Self {
        Self {
            state: Arc::new(AtomicU8::new(0)),
            failures: Arc::new(AtomicU64::new(0)),
            threshold,
            cooldown_ms: cooldown.as_millis() as u64,
            opened_at: Arc::new(AtomicU64::new(0)),
        }
    }
    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    pub async fn call<F, Fut, T>(&self, op: F) -> AppResult<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = AppResult<T>>,
    {
        let st = self.state.load(Ordering::SeqCst);
        if st == 1 {
            let opened = self.opened_at.load(Ordering::SeqCst);
            if Self::now_ms().saturating_sub(opened) < self.cooldown_ms {
                return Err(AppError::Upstream("circuit open".into()));
            }
            self.state.store(2, Ordering::SeqCst);
        }
        match op().await {
            Ok(v) => {
                self.failures.store(0, Ordering::SeqCst);
                self.state.store(0, Ordering::SeqCst);
                Ok(v)
            }
            Err(e) => {
                let f = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
                if f >= self.threshold {
                    self.state.store(1, Ordering::SeqCst);
                    self.opened_at.store(Self::now_ms(), Ordering::SeqCst);
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn timeout_triggers() {
        let r: AppResult<()> = with_timeout(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
        .await;
        assert!(matches!(r, Err(AppError::Timeout(_))));
    }

    #[tokio::test]
    async fn retry_eventually_succeeds() {
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        let r: AppResult<u32> = with_retry(5, move || {
            let c = c2.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(AppError::Upstream("nope".into()))
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), 42);
    }

    #[tokio::test]
    async fn breaker_opens() {
        let cb = CircuitBreaker::new(2, Duration::from_secs(60));
        for _ in 0..2 {
            let _: AppResult<()> = cb
                .call(|| async { Err(AppError::Upstream("x".into())) })
                .await;
        }
        let r: AppResult<()> = cb.call(|| async { Ok(()) }).await;
        assert!(matches!(r, Err(AppError::Upstream(_))));
    }
}
