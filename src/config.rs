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
/// use retriage::{RetryConfigBuilder, backoff::Exponential};
/// use std::time::Duration;
///
/// // Note: `static` requires fully explicit type parameters — `_` is not allowed.
/// static RETRY_CONFIG: LazyLock<ExponentialConfig<MyPolicy, FullJitter>> =
///     LazyLock::new(|| {
///         let backoff = Exponential::with_jitter(
///             Duration::from_millis(100),
///             Duration::from_millis(1650),
///             FullJitter,
///         );
///
///         RetryConfigBuilder::new()
///             .max_retries(4)
///             .backoff(backoff)
///             .handler(MyPolicy)
///             .build()
///     });
/// ```
#[derive(Clone, Copy)]
pub struct RetryConfig<I, H>
where
    I: Iterator<Item = Duration> + Clone,
    H: ErrorHandler,
{
    pub(crate) retry_attempts: u32,
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
    /// Clones the strategy iterator and caps it at `retry_attempts`, so each
    /// call to `retry!` gets its own independent state — shared configs are safe.
    pub fn create_state(&self) -> RetryState<std::iter::Take<I>> {
        RetryState::new(self.strategy.clone().take(self.retry_attempts as usize))
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
/// | `max_retries` | `2` |
/// | `backoff` | [`Exponential`] 100ms base, Duration::MAX max limit, and no jitter |
///
/// # Example
///
/// ```rust,ignore
/// static RETRY_CONFIG: LazyLock<ExponentialConfig<MyPolicy, FullJitter>> =
///     LazyLock::new(|| {
///         let backoff = Exponential::with_jitter(
///             Duration::from_millis(100),
///             Duration::from_millis(1650),
///             FullJitter,
///         );
///
///         RetryConfigBuilder::new()
///             .max_retries(4)
///             .backoff(backoff)
///             .handler(MyPolicy)
///             .build()
///     });
/// ```
pub struct RetryConfigBuilder<S, H> {
    // Number of retries before giving up.
    // Does not include the initial attempt.
    retry_attempts: u32,
    strategy: S,
    handler: H,
}

#[allow(clippy::new_without_default)]
impl RetryConfigBuilder<Exponential<NoJitter>, ()> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            retry_attempts: 2,
            strategy: Exponential::new(Duration::from_millis(100), Duration::MAX),
            handler: (),
        }
    }
}

impl<I, H> RetryConfigBuilder<I, H>
where
    I: Iterator<Item = Duration> + Clone,
{
    /// Maximum number of retries before giving up.
    ///
    /// Defaults to `2`.
    #[must_use]
    pub const fn max_retries(mut self, attempts: u32) -> Self {
        self.retry_attempts = attempts;
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
            retry_attempts: self.retry_attempts,
            strategy,
            handler: self.handler,
        }
    }

    /// Attach the error handler.
    ///
    /// Required — `.build()` is only available after this call.
    pub fn handler<NH: ErrorHandler>(self, handler: NH) -> RetryConfigBuilder<I, NH> {
        RetryConfigBuilder {
            retry_attempts: self.retry_attempts,
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
            retry_attempts: self.retry_attempts,
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
        fn handle<'a>(
            &self,
            _e: &'a Self::Err,
            _attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<'a, Self::Err> {
            ErrorDecision::Propagate(&DummyError)
        }
    }

    #[test]
    fn builder_defaults() {
        let config = RetryConfigBuilder::new().handler(DummyHandler).build();
        assert_eq!(config.retry_attempts, 2);
    }

    #[test]
    fn builder_custom_attempts() {
        let config = RetryConfigBuilder::new()
            .max_retries(9)
            .handler(DummyHandler)
            .build();
        assert_eq!(config.retry_attempts, 9);
    }

    #[test]
    fn builder_custom_backoff() {
        let config = RetryConfigBuilder::new()
            .backoff(Fixed::new(Duration::from_millis(50)))
            .handler(DummyHandler)
            .build();
        assert_eq!(config.retry_attempts, 2);
    }

    #[test]
    fn builder_full_chain() {
        let config = RetryConfigBuilder::new()
            .max_retries(4)
            .backoff(Linear::with_jitter(
                Duration::from_millis(200),
                Duration::from_secs(10),
                FullJitter,
            ))
            .handler(DummyHandler)
            .build();
        assert_eq!(config.retry_attempts, 4);
    }

    #[test]
    fn create_state_is_independent() {
        // The two create_state calls should each start from the beginning, and not be affected by each other.
        let config = RetryConfigBuilder::new()
            .max_retries(4)
            .backoff(Fixed::new(Duration::from_millis(100)))
            .handler(DummyHandler)
            .build();

        let mut s1 = config.create_state();
        let mut s2 = config.create_state();

        assert_eq!(s1.next_delay_for_test(), s2.next_delay_for_test());
    }
}
