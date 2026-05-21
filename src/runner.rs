use crate::{
    backoff::strategy::BackoffStrategy,
    config::RetryConfig,
    handler::{ErrorDecision, ErrorHandler},
};
use std::future::Future;
use tokio::time::sleep;

/// Executes an async closure with the retry logic defined in [`RetryConfig`].
///
/// # Type parameters
///
/// - `S` — the backoff strategy
/// - `H` — the error handler
/// - `F` — the closure to retry, must return `Result<T, H::Err>`
/// - `Fut` — the future returned by `F`
/// - `T` — the success type
///
/// # Example
///
/// ```rust,ignore
/// use triage::config::RetryConfigBuilder;
/// use triage::runner::run;
/// use triage::backoff::Exponential;
/// use std::time::Duration;
///
/// let config = RetryConfigBuilder::new()
///     .attempts(5)
///     .backoff(Exponential::new(Duration::from_millis(100)))
///     .handler(MyPolicy)
///     .build();
///
/// let result = run(&config, || async {
///     reqwest::get("https://example.com").await?;
///     Ok(())
/// }).await;
/// ```
pub async fn run<S, H, F, Fut, T>(config: &RetryConfig<S, H>, mut f: F) -> Result<T, H::Err>
where
    S: BackoffStrategy,
    H: ErrorHandler,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, H::Err>>,
{
    let mut attempt = 0;

    loop {
        attempt += 1;

        match f().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                // Exhausted all attempts — propagate regardless of handler decision
                if attempt >= config.max_attempts {
                    return Err(e);
                }

                match config.handler.handle(&e, attempt) {
                    ErrorDecision::Retry => {
                        let delay = config.strategy.next_delay(attempt);
                        sleep(delay).await;
                    }

                    ErrorDecision::RetryAfter(duration) => {
                        sleep(duration).await;
                    }

                    ErrorDecision::RetryWith(action) => {
                        action().await;
                        let delay = config.strategy.next_delay(attempt);
                        sleep(delay).await;
                    }

                    ErrorDecision::Propagate(e) => return Err(e),
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backoff::Fixed;
    use crate::config::RetryConfigBuilder;
    use crate::handler::{ErrorDecision, ErrorHandler};
    use std::fmt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

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
        fn handle(&self, e: &Self::Err, _attempt: u32) -> ErrorDecision<Self::Err> {
            match e {
                TestError::Transient => ErrorDecision::Retry,
                TestError::Fatal => ErrorDecision::Propagate(TestError::Fatal),
            }
        }
    }

    fn instant_config() -> RetryConfig<Fixed, TransientPolicy> {
        RetryConfigBuilder::new()
            .attempts(3)
            .backoff(Fixed::new(Duration::ZERO)) // Test with zero delay for simplicity
            .handler(TransientPolicy)
            .build()
    }

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let config = instant_config();
        let result = run(&config, || async { Ok::<_, TestError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retries_and_eventually_succeeds() {
        let config = instant_config();
        let attempts = Arc::new(AtomicU32::new(0));

        let result = run(&config, || {
            let attempts = attempts.clone();
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(TestError::Transient)
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn propagates_fatal_error_immediately() {
        let config = instant_config();
        let attempts = Arc::new(AtomicU32::new(0));

        let result = run(&config, || {
            let attempts = attempts.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(TestError::Fatal)
            }
        })
        .await;

        assert_eq!(result.unwrap_err(), TestError::Fatal);
        // Do not retry while fatal, just run 1 time
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exhausts_attempts_and_returns_last_error() {
        let config = instant_config();
        let attempts = Arc::new(AtomicU32::new(0));

        let result = run(&config, || {
            let attempts = attempts.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(TestError::Transient)
            }
        })
        .await;

        assert!(result.is_err());
        // max_attempts = 3, run 3 times
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_with_executes_action_before_retry() {
        struct WithActionPolicy {
            action_ran: Arc<AtomicU32>,
        }

        impl ErrorHandler for WithActionPolicy {
            type Err = TestError;
            fn handle(&self, _e: &Self::Err, _attempt: u32) -> ErrorDecision<Self::Err> {
                let counter = self.action_ran.clone();
                ErrorDecision::RetryWith(Box::new(move || {
                    Box::pin(async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                    })
                }))
            }
        }

        let action_ran = Arc::new(AtomicU32::new(0));
        let config = RetryConfigBuilder::new()
            .attempts(3)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(WithActionPolicy {
                action_ran: action_ran.clone(),
            })
            .build();

        let call_count = Arc::new(AtomicU32::new(0));
        let _ = run(&config, || {
            let call_count = call_count.clone();
            async move {
                call_count.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(TestError::Transient)
            }
        })
        .await;

        // 3 attempts = 2 retries = action ran 2 times
        assert_eq!(action_ran.load(Ordering::SeqCst), 2);
    }
}
