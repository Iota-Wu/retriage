#![allow(clippy::unwrap_used)]
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use triage::{
    RetryConfigBuilder,
    backoff::{Exponential, Fixed, FullJitter, Linear},
    dispatch,
    handler::{ErrorDecision, ErrorHandler},
    retry,
};

// ── Shared test error types ───────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum ApiError {
    Timeout,
    RateLimited,
    NotFound,
    ServerError,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Timeout => write!(f, "request timed out"),
            ApiError::RateLimited => write!(f, "rate limited"),
            ApiError::NotFound => write!(f, "not found"),
            ApiError::ServerError => write!(f, "internal server error"),
        }
    }
}

impl std::error::Error for ApiError {}

// ── Policies ──────────────────────────────────────────────────────────────────

/// Retries transient errors, propagates permanent ones.
struct ApiPolicy;

impl ErrorHandler for ApiPolicy {
    type Err = ApiError;

    fn handle(&self, e: Self::Err, _attempt: u32, _backoff: Duration) -> ErrorDecision<Self::Err> {
        match e {
            ApiError::Timeout | ApiError::ServerError => ErrorDecision::Retry,
            ApiError::RateLimited => ErrorDecision::RetryAfter(Duration::from_millis(10)),
            ApiError::NotFound => ErrorDecision::Propagate(ApiError::NotFound),
        }
    }
}

/// Retries up to attempt 2, then propagates.
struct AttemptAwarePolicy;

impl ErrorHandler for AttemptAwarePolicy {
    type Err = ApiError;

    fn handle(&self, e: Self::Err, attempt: u32, _backoff: Duration) -> ErrorDecision<Self::Err> {
        match (e, attempt) {
            (ApiError::Timeout, 1..=2) => ErrorDecision::Retry,
            (ApiError::Timeout, _) => ErrorDecision::Propagate(ApiError::Timeout),
            _ => ErrorDecision::Propagate(ApiError::ServerError),
        }
    }
}

// ── Basic retry behaviour ─────────────────────────────────────────────────────

#[tokio::test]
async fn succeeds_without_any_retry() {
    let config = RetryConfigBuilder::new().handler(ApiPolicy).build();

    let result = retry!({ Ok::<&str, ApiError>("hello") }, config).await;
    assert_eq!(result.unwrap(), "hello");
}

#[tokio::test]
async fn retries_until_success() {
    let config = RetryConfigBuilder::new()
        .attempts(5)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(ApiPolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 3 {
                Err(ApiError::Timeout)
            } else {
                Ok("recovered")
            }
        },
        config
    )
    .await;

    assert_eq!(result.unwrap(), "recovered");
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn propagates_permanent_error_immediately() {
    let config = RetryConfigBuilder::new()
        .attempts(5)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(ApiPolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(ApiError::NotFound)
        },
        config
    )
    .await;

    assert_eq!(result.unwrap_err(), ApiError::NotFound);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exhausts_all_attempts() {
    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(ApiPolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(ApiError::Timeout)
        },
        config
    )
    .await;

    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

// ── Backoff strategies ────────────────────────────────────────────────────────

#[tokio::test]
async fn works_with_exponential_backoff() {
    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Exponential::new(Duration::ZERO))
        .handler(ApiPolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(ApiError::Timeout)
            } else {
                Ok(())
            }
        },
        config
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn works_with_linear_backoff_and_jitter() {
    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Linear::with_jitter(Duration::ZERO, FullJitter))
        .handler(ApiPolicy)
        .build();

    let result = retry!({ Ok::<_, ApiError>(42) }, config).await;
    assert_eq!(result.unwrap(), 42);
}

// ── RetryAfter ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_after_overrides_backoff_for_that_attempt() {
    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(ApiPolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 1 {
                Err(ApiError::RateLimited)
            } else {
                Ok("ok")
            }
        },
        config
    )
    .await;

    assert_eq!(result.unwrap(), "ok");
}

// ── RetryWith ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_with_executes_action_then_retries() {
    struct RotationPolicy {
        rotated: Arc<AtomicU32>,
    }

    impl ErrorHandler for RotationPolicy {
        type Err = ApiError;

        fn handle(
            &self,
            _e: Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<Self::Err> {
            let rotated = Arc::clone(&self.rotated);
            ErrorDecision::RetryWith(Box::new(move || {
                Box::pin(async move {
                    rotated.fetch_add(1, Ordering::SeqCst);
                })
            }))
        }
    }

    let rotated = Arc::new(AtomicU32::new(0));
    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(RotationPolicy {
            rotated: Arc::clone(&rotated),
        })
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let _ = retry!(
        {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(ApiError::Timeout)
        },
        config
    )
    .await;

    // 3 attempts → 2 retries → action ran 2 times
    assert_eq!(rotated.load(Ordering::SeqCst), 2);
}

// ── dispatch! macro ───────────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_downcasts_anyhow_to_concrete_type() {
    #[derive(Debug)]
    struct DbError;
    impl fmt::Display for DbError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "db error")
        }
    }
    impl std::error::Error for DbError {}

    #[derive(Debug)]
    struct NetError;
    impl fmt::Display for NetError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "net error")
        }
    }
    impl std::error::Error for NetError {}

    struct DispatchPolicy;
    impl ErrorHandler for DispatchPolicy {
        type Err = anyhow::Error;

        fn handle(
            &self,
            e: Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<Self::Err> {
            dispatch! {
                e,
                DbError  => |_e| ErrorDecision::Retry,
                NetError => |_e| ErrorDecision::Retry,
                _ => ErrorDecision::Propagate(anyhow::anyhow!("unknown error")),
            }
        }
    }

    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(DispatchPolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(anyhow::Error::new(DbError))
            } else {
                Ok("dispatched")
            }
        },
        config
    )
    .await;

    assert_eq!(result.unwrap(), "dispatched");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn dispatch_falls_through_to_catchall() {
    #[derive(Debug)]
    struct UnknownError;
    impl fmt::Display for UnknownError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "unknown")
        }
    }
    impl std::error::Error for UnknownError {}

    #[derive(Debug)]
    struct KnownError;
    impl fmt::Display for KnownError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "known")
        }
    }
    impl std::error::Error for KnownError {}

    struct StrictPolicy;
    impl ErrorHandler for StrictPolicy {
        type Err = anyhow::Error;

        fn handle(
            &self,
            e: Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<Self::Err> {
            dispatch! {
                e,
                KnownError => |_e| ErrorDecision::Retry,
                _ => ErrorDecision::Propagate(anyhow::anyhow!("unhandled error")),
            }
        }
    }

    let config = RetryConfigBuilder::new()
        .attempts(3)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(StrictPolicy)
        .build();

    let result = retry!({ Err::<(), _>(anyhow::Error::new(UnknownError)) }, config).await;

    assert!(result.is_err());
}

// ── Attempt-aware policy ──────────────────────────────────────────────────────

#[tokio::test]
async fn attempt_aware_policy_gives_up_after_threshold() {
    let config = RetryConfigBuilder::new()
        .attempts(5)
        .backoff(Fixed::new(Duration::ZERO))
        .handler(AttemptAwarePolicy)
        .build();

    let attempts = Arc::new(AtomicU32::new(0));
    let result = retry!(
        {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(ApiError::Timeout)
        },
        config
    )
    .await;

    assert_eq!(result.unwrap_err(), ApiError::Timeout);
    // attempt 1 → Retry, attempt 2 → Retry, attempt 3 → Propagate
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}
