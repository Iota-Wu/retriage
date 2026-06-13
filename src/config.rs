use crate::{
    backoff::{Exponential, NoJitter},
    handler::ErrorHandler,
    runner::RetryState,
};
use std::time::Duration;

/// The assembled retry configuration passed to [`retry!`].
///
/// Construct via [`RetryConfigBuilder::new`]. Once built, a config is
/// cheaply cloneable and can be shared across calls via [`std::sync::LazyLock`].
///
/// # Type parameters
///
/// - `I` — the backoff iterator, must be `Clone` so the runner can reset state
///   between independent retry sequences
/// - `H` — the error handler
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::LazyLock;
/// use triage::{RetryConfigBuilder, backoff::Exponential};
/// use std::time::Duration;
///
/// // Note: `static` requires fully explicit type parameters — `_` is not allowed.
/// static RETRY: LazyLock<RetryConfig<Exponential, MyPolicy>> = LazyLock::new(|| {
///     RetryConfigBuilder::new()
///         .attempts(5)
///         .backoff(Exponential::new(Duration::from_millis(100)))
///         .handler(MyPolicy)
///         .build()
/// });
/// ```
#[derive(Clone, Copy)]
pub struct RetryConfig<I, H>
where
    I: Iterator<Item = Duration> + Clone,
    H: ErrorHandler,
{
    pub(crate) max_attempts: u32,
    pub(crate) strategy: I,
    pub(crate) handler: H,
}

impl<I, H> RetryConfig<I, H>
where
    I: Iterator<Item = Duration> + Clone,
    H: ErrorHandler,
{
    /// Creates a fresh [`RetryState`] for one retry sequence.
    ///
    /// Clones the strategy iterator and caps it at `max_attempts`, so each
    /// call to `retry!` gets its own independent state — shared configs are safe.
    pub fn create_state(&self) -> RetryState<std::iter::Take<I>> {
        RetryState::new(self.strategy.clone().take((self.max_attempts - 1) as usize))
    }
}

// ── Builder states ────────────────────────────────────────────────────────────
//
// Typestate pattern: the compiler enforces that `.handler()` is called before
// `.build()`. No handler = no build.
//
// () = handler not yet provided
// H  = handler provided

/// Builds a [`RetryConfig`] with a fluent API.
///
/// The only required call is `.handler()` — everything else has a default.
///
/// # Defaults
///
/// | Setting | Default |
/// |---|---|
/// | `attempts` | `3` |
/// | `backoff` | [`Exponential`] 100ms base, no jitter |
///
/// # Example
///
/// ```rust,ignore
/// use triage::RetryConfigBuilder;
/// use triage::backoff::{Exponential, FullJitter};
/// use std::time::Duration;
///
/// let config = RetryConfigBuilder::new()
///     .attempts(5)
///     .backoff(
///         Exponential::with_jitter(Duration::from_millis(100), FullJitter)
///             .max_delay(Duration::from_secs(30))
///     )
///     .handler(MyPolicy)
///     .build();
/// ```
pub struct RetryConfigBuilder<S, H> {
    max_attempts: u32,
    strategy: S,
    handler: H,
}

impl RetryConfigBuilder<Exponential<NoJitter>, ()> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_attempts: 3,
            strategy: Exponential::new(Duration::from_millis(100)),
            handler: (),
        }
    }
}

impl Default for RetryConfigBuilder<Exponential<NoJitter>, ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I, H> RetryConfigBuilder<I, H>
where
    I: Iterator<Item = Duration> + Clone,
{
    /// Maximum number of attempts before giving up.
    ///
    /// Includes the first attempt — e.g. `attempts(3)` means
    /// 1 initial try + 2 retries. Must be at least 1.
    ///
    /// Defaults to `3`.
    #[must_use]
    pub fn attempts(mut self, max_attempts: u32) -> Self {
        assert!(self.max_attempts > 0, "max_attempts must be at least 1");
        self.max_attempts = max_attempts;
        self
    }

    /// Replace the backoff strategy.
    ///
    /// Accepts any [`Iterator<Item = Duration>`] — this includes all built-in
    /// strategies ([`Fixed`], [`Linear`], [`Exponential`]) as well as custom
    /// iterators.
    ///
    /// Defaults to [`Exponential`] with 100ms base and no jitter.
    ///
    /// [`Fixed`]: crate::backoff::Fixed
    /// [`Linear`]: crate::backoff::Linear
    /// [`Exponential`]: crate::backoff::Exponential
    pub fn backoff<NS>(self, strategy: NS) -> RetryConfigBuilder<NS, H>
    where
        NS: Iterator<Item = Duration> + Clone,
    {
        RetryConfigBuilder {
            max_attempts: self.max_attempts,
            strategy,
            handler: self.handler,
        }
    }

    /// Attach the error handler.
    ///
    /// Required — `.build()` is only available after this call.
    pub fn handler<NH: ErrorHandler>(self, handler: NH) -> RetryConfigBuilder<I, NH> {
        RetryConfigBuilder {
            max_attempts: self.max_attempts,
            strategy: self.strategy,
            handler,
        }
    }
}

/// Only available once a handler has been provided.
impl<I, H> RetryConfigBuilder<I, H>
where
    I: Iterator<Item = Duration> + Clone,
    H: ErrorHandler,
{
    pub fn build(self) -> RetryConfig<I, H> {
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
        fn handle(
            &self,
            _e: Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<Self::Err> {
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

    #[test]
    fn create_state_is_independent() {
        // The two create_state calls should each start from the beginning, and not be affected by each other.
        let config = RetryConfigBuilder::new()
            .attempts(3)
            .backoff(Fixed::new(Duration::from_millis(100)))
            .handler(DummyHandler)
            .build();

        let mut s1 = config.create_state();
        let mut s2 = config.create_state();

        assert_eq!(s1.next_delay_for_test(), s2.next_delay_for_test());
    }
}
