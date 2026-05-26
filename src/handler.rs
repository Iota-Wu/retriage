use std::{future::Future, pin::Pin, time::Duration};

/// A boxed async action to execute before the next retry attempt.
///
/// Commonly used for side effects such as rotating IPs, refreshing
/// auth tokens, or resetting connection state.
pub type RetryAction = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// The outcome of [`ErrorHandler::handle`] — decides what happens next.
pub enum ErrorDecision<E> {
    /// Retry immediately without any delay.
    Retry,

    /// Wait for the specified duration, then retry.
    /// Overrides the configured backoff for this attempt only.
    RetryAfter(Duration),

    /// Execute an async action (e.g. rotate IP, refresh token),
    /// then retry. Backoff still applies after the action completes.
    RetryWith(RetryAction),

    /// Do not retry — propagate the error to the caller as-is.
    Propagate(E),
}

/// Decides how an error should be handled for a given attempt.
///
/// # Example
///
/// ```rust,ignore
/// use triage::handler::{ErrorHandler, ErrorDecision};
///
/// struct MyPolicy;
///
/// impl ErrorHandler for MyPolicy {
///     type Err = anyhow::Error;
///
///     fn handle(&self, e: &Self::Err, attempt: u32) -> ErrorDecision<Self::Err> {
///         dispatch! {
///             e,
///             reqwest::Error => |e| {
///                 if e.is_timeout() {
///                     ErrorDecision::Retry
///                 } else {
///                     ErrorDecision::Propagate(anyhow::anyhow!(e.to_string()))
///                 }
///             },
///             _ => ErrorDecision::Propagate(anyhow::anyhow!("unknown error")),
///         }
///     }
/// }
/// ```
pub trait ErrorHandler: Send + Sync {
    /// The error type this handler operates on.
    ///
    /// | Error type | `handle` | [`dispatch!`] |
    /// |---|---|---|
    /// | `anyhow::Error` | ✓ | ✓ |
    /// | `thiserror` enum | ✓ use `match` directly | — not needed |
    /// | `Box<dyn Error>` | ✗ | ✗ |
    ///
    /// # Why not `std::error::Error`?
    ///
    /// `anyhow::Error` intentionally does not implement `std::error::Error`,
    /// so that bound is deliberately absent here. Only `Send + 'static` is
    /// required, which is sufficient for safe use across async tasks.
    ///
    /// `Box<dyn Error>` is unsupported because it does not satisfy `Sized`.
    /// This is a standard library limitation — migrate with `anyhow::Error::from(e)`.
    ///
    /// Note: while `Display` is not enforced by the bound, implementors are
    /// strongly encouraged to ensure their error type implements it.
    type Err: Send + 'static;

    /// Inspect the error and decide what the runner should do next.
    ///
    /// `attempt` is 1-indexed: the first failure is attempt 1.
    /// `backoff` is the delay the runner has calculated for this attempt —
    /// useful for logging or conditional logic. Ignore with `_` if not needed.
    fn handle(&self, e: Self::Err, attempt: u32, backoff: Duration) -> ErrorDecision<Self::Err>;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    // Minimal error type for testing
    #[derive(Debug, PartialEq)]
    enum TestError {
        Transient,
        Fatal,
    }

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                TestError::Transient => write!(f, "transient error"),
                TestError::Fatal => write!(f, "fatal error"),
            }
        }
    }

    impl std::error::Error for TestError {}

    struct TestPolicy;

    impl ErrorHandler for TestPolicy {
        type Err = TestError;

        fn handle(
            &self,
            e: Self::Err,
            attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<Self::Err> {
            match e {
                TestError::Transient if attempt < 3 => ErrorDecision::Retry,
                TestError::Transient => ErrorDecision::RetryAfter(Duration::from_millis(100)),
                TestError::Fatal => ErrorDecision::Propagate(TestError::Fatal),
            }
        }
    }

    #[test]
    fn transient_retries_on_early_attempts() {
        let policy = TestPolicy;
        assert!(matches!(
            policy.handle(TestError::Transient, 1, Duration::ZERO),
            ErrorDecision::Retry
        ));
        assert!(matches!(
            policy.handle(TestError::Transient, 2, Duration::ZERO),
            ErrorDecision::Retry
        ));
    }

    #[test]
    fn transient_retry_after_on_late_attempts() {
        let policy = TestPolicy;
        assert!(matches!(
            policy.handle(TestError::Transient, 3, Duration::ZERO),
            ErrorDecision::RetryAfter(_)
        ));
    }

    #[test]
    fn fatal_always_propagates() {
        let policy = TestPolicy;
        assert!(matches!(
            policy.handle(TestError::Fatal, 1, Duration::ZERO),
            ErrorDecision::Propagate(TestError::Fatal)
        ));
    }

    #[test]
    fn retry_with_carries_action() {
        // Verify RetryWith can hold an async action
        let decision: ErrorDecision<TestError> =
            ErrorDecision::RetryWith(Box::new(|| Box::pin(async { /* rotate IP */ })));
        assert!(matches!(decision, ErrorDecision::RetryWith(_)));
    }
}
