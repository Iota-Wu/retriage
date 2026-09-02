use crate::{
    config::RetryConfig,
    handler::{ErrorDecision, ErrorHandler, RetryAction},
};
use std::{ops::ControlFlow, time::Duration};

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
/// iterator, consults the error handler via a shared reference to the error,
/// then either sleeps and returns [`ControlFlow::Continue`] (signal to retry)
/// or returns [`ControlFlow::Break`] (signal to stop).
///
/// The original error remains owned by the caller (`retry!` macro) throughout,
/// so it can be propagated without any cloning or re-allocation on the stop path.
///
/// This function is `pub` for advanced use cases. Most users should use
/// the [`retry!`] macro instead.
#[inline]
pub fn next_step<I, H>(
    state: &mut RetryState<I>,
    config: &RetryConfig<impl Iterator<Item = Duration> + Clone, H>,
    err: &H::Err,
) -> ControlFlow<(), (Option<RetryAction>, Option<Duration>)>
where
    I: Iterator<Item = Duration>,
    H: ErrorHandler,
{
    let Some(backoff) = state.iter.next() else {
        return ControlFlow::Break(());
    };

    state.attempt += 1;

    let decision = match config.handler.handle(err, state.attempt, backoff) {
        ErrorDecision::RetryImmediately => (None, None),
        ErrorDecision::RetryAfter(duration) => (None, Some(duration)),
        ErrorDecision::RetryWithImmediately(action) => (Some(action), None),
        ErrorDecision::RetryWith(duration, action) => (Some(action), Some(duration)),
        ErrorDecision::Propagate(_) => return ControlFlow::Break(()),
    };

    ControlFlow::Continue(decision)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{
        backoff::Fixed,
        config::RetryConfigBuilder,
        handler::{ErrorDecision, ErrorHandler},
    };
    use std::fmt;

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
        fn handle<'a>(
            &self,
            e: &'a Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<'a, Self::Err> {
            match e {
                TestError::Transient => ErrorDecision::RetryImmediately,
                TestError::Fatal => ErrorDecision::Propagate(e),
            }
        }
    }

    fn make_config() -> RetryConfig<Fixed, TransientPolicy> {
        RetryConfigBuilder::new()
            .max_retries(2)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(TransientPolicy)
            .build()
    }

    #[tokio::test]
    async fn transient_returns_continue() {
        let config = make_config();
        let mut state = config.create_state();
        let result = next_step(&mut state, &config, &TestError::Transient);
        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn fatal_returns_break() {
        let config = make_config();
        let mut state = config.create_state();
        let result = next_step(&mut state, &config, &TestError::Fatal);
        assert!(result.is_break());
    }

    #[tokio::test]
    async fn exhausted_iterator_returns_break() {
        let config = make_config();
        let mut state = config.create_state();
        // max_retries(2) = 2 retries = 2 backoff values
        for _ in 0..2 {
            let _ = next_step(&mut state, &config, &TestError::Transient);
        }
        // 3rd call — iterator exhausted
        let result = next_step(&mut state, &config, &TestError::Transient);
        assert!(result.is_break());
    }
}
