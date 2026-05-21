use crate::{
    backoff::{BackoffStrategy, Exponential, NoJitter},
    handler::ErrorHandler,
};
use std::time::Duration;

/// The assembled retry configuration passed to the runner.
///
/// Construct via [`RetryConfigBuilder::new`].
pub struct RetryConfig<S, H>
where
    S: BackoffStrategy,
    H: ErrorHandler,
{
    pub(crate) max_attempts: u32,
    pub(crate) strategy: S,
    pub(crate) handler: H,
}

// ── Builder states ────────────────────────────────────────────────────────────
//
// We use a typestate pattern so the compiler enforces that `.handler()`
// is called before `.build()`. No handler = no build.
//
// () = handler not yet provided
// H  = handler provided

pub struct RetryConfigBuilder<S, H> {
    max_attempts: u32,
    strategy: S,
    handler: H,
}

impl RetryConfigBuilder<Exponential<NoJitter>, ()> {
    pub fn new() -> Self {
        Self {
            max_attempts: 3,
            strategy: Exponential::new(Duration::from_millis(100)),
            handler: (),
        }
    }
}

impl<S, H> RetryConfigBuilder<S, H>
where
    S: BackoffStrategy,
{
    /// Maximum number of attempts before giving up.
    /// Includes the first attempt — e.g. `attempts(3)` means
    /// 1 initial try + 2 retries.
    ///
    /// Defaults to 3.
    pub fn attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Replace the backoff strategy.
    ///
    /// Defaults to [`Exponential`] with 100ms base and no jitter.
    pub fn backoff<NS: BackoffStrategy>(self, strategy: NS) -> RetryConfigBuilder<NS, H> {
        RetryConfigBuilder {
            max_attempts: self.max_attempts,
            strategy,
            handler: self.handler,
        }
    }

    /// Attach the error handler.
    ///
    /// Required — calling `.build()` is not possible without this.
    pub fn handler<NH: ErrorHandler>(self, handler: NH) -> RetryConfigBuilder<S, NH> {
        RetryConfigBuilder {
            max_attempts: self.max_attempts,
            strategy: self.strategy,
            handler,
        }
    }
}

/// Only available once a handler has been provided.
impl<S, H> RetryConfigBuilder<S, H>
where
    S: BackoffStrategy,
    H: ErrorHandler,
{
    pub fn build(self) -> RetryConfig<S, H> {
        assert!(self.max_attempts > 0, "max_attempts must be at least 1");
        RetryConfig {
            max_attempts: self.max_attempts,
            strategy: self.strategy,
            handler: self.handler,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backoff::{
        jitter::FullJitter,
        strategy::{Fixed, Linear},
    };
    use crate::handler::{ErrorDecision, ErrorHandler};
    use std::fmt;

    #[derive(Debug)]
    struct DummyError;
    impl fmt::Display for DummyError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "dummy")
        }
    }
    impl std::error::Error for DummyError {}

    struct DummyHandler;
    impl ErrorHandler for DummyHandler {
        type Err = DummyError;
        fn handle(&self, _e: &Self::Err, _attempt: u32) -> ErrorDecision<Self::Err> {
            ErrorDecision::Propagate(DummyError)
        }
    }

    #[test]
    fn builder_defaults() {
        let config = RetryConfigBuilder::new().handler(DummyHandler).build();

        assert_eq!(config.max_attempts, 3);
    }

    #[test]
    fn builder_custom_attempts() {
        let config = RetryConfigBuilder::new()
            .attempts(10)
            .handler(DummyHandler)
            .build();

        assert_eq!(config.max_attempts, 10);
    }

    #[test]
    fn builder_custom_backoff() {
        let config = RetryConfigBuilder::new()
            .backoff(Fixed::new(Duration::from_millis(50)))
            .handler(DummyHandler)
            .build();

        assert_eq!(config.max_attempts, 3);
    }

    #[test]
    fn builder_full_chain() {
        let config = RetryConfigBuilder::new()
            .attempts(5)
            .backoff(
                Linear::with_jitter(Duration::from_millis(200), FullJitter)
                    .max_delay(Duration::from_secs(10)),
            )
            .handler(DummyHandler)
            .build();

        assert_eq!(config.max_attempts, 5);
    }

    #[test]
    #[should_panic(expected = "max_attempts must be at least 1")]
    fn builder_rejects_zero_attempts() {
        RetryConfigBuilder::new()
            .attempts(0)
            .handler(DummyHandler)
            .build();
    }
}
