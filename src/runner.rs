use crate::{
    config::RetryConfig,
    handler::{ErrorDecision, ErrorHandler},
};
use std::time::Duration;
use tokio::time::sleep;

/// Tracks iteration state for a single retry sequence.
///
/// Created by [`RetryConfig::create_state`] and consumed by the [`retry!`] macro.
/// Each call to `retry!` creates a fresh state, so shared configs are safe.
pub struct RetryState<I> {
    pub(crate) iter: I,
    pub(crate) attempt: u32,
}
 
impl<I> RetryState<I>
where
    I: Iterator<Item = Duration>,
{
    pub(crate) fn new(iter: I) -> Self {
        Self { iter, attempt: 0 }
    }
 
    #[cfg(test)]
    pub(crate) fn next_delay_for_test(&mut self) -> Option<Duration>
    where
        I: Iterator<Item = Duration>,
    {
        self.iter.next()
    }
}
 
/// Processes a single failed attempt and drives the retry decision.
///
/// Called by the [`retry!`] macro after each failure. Advances the backoff
/// iterator, consults the error handler, then either sleeps and returns `Ok(())`
/// (signal to retry) or returns `Err(e)` (signal to stop).
///
/// Returns:
/// - `Ok(())` — caller should retry
/// - `Err(e)` — caller should propagate; either attempts exhausted or handler
///   returned [`ErrorDecision::Propagate`]
///
/// This function is `pub` for advanced use cases. Most users should use
/// the [`retry!`] macro instead.
pub async fn next_step<I, H>(
    state: &mut RetryState<I>,
    config: &RetryConfig<impl Iterator<Item = Duration> + Clone, H>,
    err: H::Err,
) -> Result<(), H::Err>
where
    I: Iterator<Item = Duration>,
    H: ErrorHandler,
{
    match state.iter.next() {
        // Iterator exhausted — all attempts used up
        None => Err(err),
        Some(backoff) => {
            state.attempt += 1;
 
            match config.handler.handle(err, state.attempt, backoff) {
                ErrorDecision::Retry => {
                    sleep(backoff).await;
                    Ok(())
                }
                ErrorDecision::RetryAfter(duration) => {
                    sleep(duration).await;
                    Ok(())
                }
                ErrorDecision::RetryWith(action) => {
                    action().await;
                    sleep(backoff).await;
                    Ok(())
                }
                ErrorDecision::Propagate(err) => Err(err),
            }
        }
    }
}
 
// ── Tests ─────────────────────────────────────────────────────────────────────
 
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backoff::Fixed,
        config::RetryConfigBuilder,
        handler::{ErrorDecision, ErrorHandler},
    };
    use std::{
        fmt,
        sync::Arc,
        sync::atomic::{AtomicU32, Ordering},
    };

    #[derive(Debug, PartialEq)]
    enum TestError {
        Transient,
        Fatal,
    }
 
    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                TestError::Transient => write!(f, "transient"),
                TestError::Fatal => write!(f, "fatal"),
            }
        }
    }
 
    impl std::error::Error for TestError {}
 
    struct TransientPolicy;
    impl ErrorHandler for TransientPolicy {
        type Err = TestError;
        fn handle(
            &self,
            e: Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<Self::Err> {
            match e {
                TestError::Transient => ErrorDecision::Retry,
                TestError::Fatal => ErrorDecision::Propagate(TestError::Fatal),
            }
        }
    }
 
    fn make_config() -> RetryConfig<Fixed, TransientPolicy> {
        RetryConfigBuilder::new()
            .attempts(3)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(TransientPolicy)
            .build()
    }
 
    #[tokio::test]
    async fn transient_returns_ok_to_signal_retry() {
        let config = make_config();
        let mut state = config.create_state();
        let result = next_step(&mut state, &config, TestError::Transient).await;
        assert!(result.is_ok());
    }
 
    #[tokio::test]
    async fn fatal_returns_err_immediately() {
        let config = make_config();
        let mut state = config.create_state();
        let result = next_step(&mut state, &config, TestError::Fatal).await;
        assert_eq!(result.unwrap_err(), TestError::Fatal);
    }
 
    #[tokio::test]
    async fn exhausted_iterator_propagates_error() {
        let config = make_config();
        let mut state = config.create_state();
        // attempts(3) = 2 retries = 2 backoff values
        for _ in 0..2 {
            let _ = next_step(&mut state, &config, TestError::Transient).await;
        }
        // 3rd call — iterator exhausted
        let result = next_step(&mut state, &config, TestError::Transient).await;
        assert!(result.is_err());
    }
 
    #[tokio::test]
    async fn retry_with_executes_action() {
        struct ActionPolicy {
            ran: Arc<AtomicU32>,
        }
        impl ErrorHandler for ActionPolicy {
            type Err = TestError;
            fn handle(
                &self,
                _e: Self::Err,
                _attempt: u32,
                _backoff: Duration,
            ) -> ErrorDecision<Self::Err> {
                let ran = self.ran.clone();
                ErrorDecision::RetryWith(Box::new(move || {
                    Box::pin(async move {
                        ran.fetch_add(1, Ordering::SeqCst);
                    })
                }))
            }
        }
 
        let ran = Arc::new(AtomicU32::new(0));
        let config = RetryConfigBuilder::new()
            .attempts(3)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(ActionPolicy { ran: ran.clone() })
            .build();
 
        let mut state = config.create_state();
        let _ = next_step(&mut state, &config, TestError::Transient).await;
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }
}
