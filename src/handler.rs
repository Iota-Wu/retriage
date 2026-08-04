use std::{
    fmt::{Debug, Formatter, Result},
    future::Future,
    pin::Pin,
    time::Duration,
};

/// A boxed async action to execute before the next retry attempt.
///
/// Commonly used for side effects such as rotating IPs, refreshing
/// auth tokens, or resetting connection state.
pub type RetryAction = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// The outcome of [`ErrorHandler::handle`] — decides what happens next.
pub enum ErrorDecision<'a, E: ?Sized + 'static> {
    /// Retry immediately without any delay.
    RetryImmediately,

    /// Wait for the specified duration, then retry.
    /// Overrides the configured backoff for this attempt only.
    RetryAfter(Duration),

    /// Execute an async action (e.g. rotate IP, refresh token),
    /// then retry. Backoff still applies after the action completes.
    RetryWith(RetryAction),

    /// Execute an async action (e.g. rotate IP, refresh token),
    /// then retry. Backoff does not apply after the action completes.
    RetryWithImmediately(RetryAction),

    /// Do not retry — propagate the error to the caller as-is.
    Propagate(&'a E),
}

impl<E: Debug> Debug for ErrorDecision<'_, E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::RetryImmediately => f.write_str("RetryImmediately"),
            Self::RetryAfter(duration) => f.debug_tuple("RetryAfter").field(duration).finish(),
            // RetryWith is an async closure, so we can't implement Debug for it directly.
            Self::RetryWith(_) => f.write_str("RetryWith(<async closure>)"),
            // RetryWithImmediately is an async closure, so we can't implement Debug for it directly.
            Self::RetryWithImmediately(_) => f.write_str("RetryWithImmediately(<async closure>)"),
            Self::Propagate(err) => f.debug_tuple("Propagate").field(err).finish(),
        }
    }
}

/// Decides how an error should be handled for a given attempt.
///
/// # Example
///
/// ```rust,ignore
/// use retriage::handler::{ErrorHandler, ErrorDecision};
///
/// struct MyPolicy;
///
/// impl ErrorHandler for MyPolicy {
///     type Err = anyhow::Error;
///
///     fn handle<'a>(
///         &self,
///         e: &'a Self::Err,
///         _attempt: u32,
///         backoff: Duration,
///     ) -> ErrorDecision<'a, Self::Err> {
///         dispatch! {
///             e,
///             reqwest::Error => |e| {
///                 if e.is_timeout() {
///                     tracing::warn!("request timed out, retrying in {}ms", backoff.as_millis());
///                     ErrorDecision::RetryAfter(backoff)
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
    /// | `Box<dyn std::error::Error>` | ✓ | ✓ |
    /// | `dyn std::error::Error` (trait object) | ✓ (due to `?Sized`) | ✓ |
    ///
    /// # Trait Bounds Design
    ///
    /// `anyhow::Error` intentionally does not implement `std::error::Error`,
    /// so that bound is deliberately absent from `type Err`. Only `Send + ?Sized + 'static`
    /// is required, allowing both custom error types, `anyhow::Error`, and unsized
    /// trait objects (`dyn std::error::Error`) to be handled transparently.
    ///
    /// Note: while `Display` is not enforced by the bound, implementors are
    /// strongly encouraged to ensure their error type implements it.
    type Err: Send + ?Sized + 'static;

    /// Inspect the error and decide what the runner should do next.
    ///
    /// `attempt` is 1-indexed: the first failure is attempt 1.
    /// `backoff` is the delay the runner has calculated for this attempt —
    /// useful for logging or conditional logic. Ignore with `_` if not needed.
    fn handle<'a>(
        &self,
        e: &'a Self::Err,
        attempt: u32,
        backoff: Duration,
    ) -> ErrorDecision<'a, Self::Err>;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::{assert_matches, fmt};

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

        fn handle<'a>(
            &self,
            e: &'a Self::Err,
            attempt: u32,
            backoff: Duration,
        ) -> ErrorDecision<'a, Self::Err> {
            match e {
                TestError::Transient if attempt == 1 => ErrorDecision::RetryImmediately,
                TestError::Transient if attempt == 2 => ErrorDecision::RetryAfter(backoff),
                TestError::Transient => ErrorDecision::RetryWithImmediately(Box::new(|| {
                    Box::pin(async { /* refresh token or rotate IP */ })
                })),
                TestError::Fatal => ErrorDecision::Propagate(e),
            }
        }
    }

    #[test]
    fn test_retry_immediately_on_first_attempt() {
        let policy = TestPolicy;
        assert_matches!(
            policy.handle(&TestError::Transient, 1, Duration::from_millis(100)),
            ErrorDecision::RetryImmediately
        );
    }

    #[test]
    fn test_retry_after_on_second_attempt() {
        let policy = TestPolicy;
        let delay = Duration::from_millis(100);
        assert_matches!(
            policy.handle(&TestError::Transient, 2, delay),
            ErrorDecision::RetryAfter(d) if d == delay
        );
    }

    #[test]
    fn test_retry_with_immediately_on_later_attempts() {
        let policy = TestPolicy;
        assert_matches!(
            policy.handle(&TestError::Transient, 3, Duration::from_millis(100)),
            ErrorDecision::RetryWithImmediately(_)
        );
    }

    #[test]
    fn fatal_always_propagates() {
        let policy = TestPolicy;
        assert_matches!(
            policy.handle(&TestError::Fatal, 1, Duration::ZERO),
            ErrorDecision::Propagate(TestError::Fatal)
        );
    }

    #[test]
    fn test_retry_with_carries_action() {
        let decision: ErrorDecision<TestError> =
            ErrorDecision::RetryWith(Box::new(|| Box::pin(async { /* rotate IP */ })));
        assert_matches!(decision, ErrorDecision::RetryWith(_));
    }

    #[test]
    fn test_retry_with_immediately_carries_action() {
        let decision: ErrorDecision<TestError> =
            ErrorDecision::RetryWithImmediately(Box::new(|| Box::pin(async { /* rotate IP */ })));
        assert_matches!(decision, ErrorDecision::RetryWithImmediately(_));
    }

    #[test]
    fn test_debug_formatting() {
        let decision_immediately: ErrorDecision<TestError> = ErrorDecision::RetryImmediately;
        assert_eq!(format!("{decision_immediately:?}"), "RetryImmediately");

        let decision_after: ErrorDecision<TestError> =
            ErrorDecision::RetryAfter(Duration::from_millis(50));
        assert_eq!(format!("{decision_after:?}"), "RetryAfter(50ms)");

        let decision_with: ErrorDecision<TestError> =
            ErrorDecision::RetryWith(Box::new(|| Box::pin(async {})));
        assert_eq!(format!("{decision_with:?}"), "RetryWith(<async closure>)");

        let decision_with_imm: ErrorDecision<TestError> =
            ErrorDecision::RetryWithImmediately(Box::new(|| Box::pin(async {})));
        assert_eq!(
            format!("{decision_with_imm:?}"),
            "RetryWithImmediately(<async closure>)"
        );

        let err = TestError::Fatal;
        let decision_propagate: ErrorDecision<TestError> = ErrorDecision::Propagate(&err);
        assert_eq!(format!("{decision_propagate:?}"), "Propagate(Fatal)");
    }
}
