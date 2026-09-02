/// Helper for the cast variant of [`retry!`].
///
/// Explicitly binds the lifetime of the cast result to the input reference,
/// allowing the compiler to correctly infer lifetimes when converting
/// `&E` to `&H::Err` inside the macro.
///
/// This function is an implementation detail of `retry!` and is not intended
/// for direct use.
#[doc(hidden)]
#[inline]
pub fn cast_err<'a, E, Target: ?Sized>(e: &'a E, f: impl Fn(&'a E) -> &'a Target) -> &'a Target {
    f(e)
}

/// Executes a block with retry logic defined by a [`RetryConfig`].
///
/// Expands to an `async` block — `.await` it at the call site. This makes
/// the async nature explicit, as delay management (via `tokio::time::sleep`)
/// is handled asynchronously by the runner based on [`ErrorDecision`].
///
/// The block is re-evaluated on every attempt, which avoids higher-ranked
/// lifetime errors that arise when async closures capture borrows
/// (e.g. `sqlx` query builders, `&str` parameters).
///
/// # Syntax
///
/// ```rust,ignore
/// // Without cast — E must match H::Err exactly
/// let value = retry!({ expr }, config).await?;
///
/// // With cast — convert &E to &H::Err on the error path, zero heap allocation
/// let value = retry!({ expr }, config, |e| e as &DynError).await?;
/// let value = retry!({ expr }, config, |e| e.as_ref()).await?;
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use retriage::{retry, RetryConfigBuilder};
/// use retriage::backoff::Exponential;
/// use std::time::Duration;
///
/// // Simple case — error type matches handler directly
/// let foo = retry!({
///     bar().await
/// }, config).await?;
///
/// // Multiple error sources — cast to unify, zero heap allocation
/// let foo = retry!({
///     bar().await
/// }, config, |e| e as &retriage::DynError).await?;
///
/// // anyhow::Error — use as_ref() to get &dyn Error
/// let foo = retry!({
///     bar().await
/// }, config, |e: &anyhow::Error| e.as_ref()).await?;
/// ```
///
/// # With rate limiting (out of scope for retriage)
///
/// ```rust,ignore
/// retry!({
///     limiter.until_ready().await;
///     foo().await
/// }, config).await;
/// ```
#[macro_export]
macro_rules! retry {
    // Without cast — &e passed directly, zero cost
    ($blk:block, $config:expr) => {
        $crate::retry!($blk, $config, |e| e)
    };

    // With cast — user provides a closure to convert &E to &H::Err
    // Fat pointer built on stack, no heap allocation
    ($blk:block, $config:expr, $cast:expr) => {
        async {
            let config = &$config;
            let mut state = config.create_state();

            loop {
                match $blk {
                    Ok(value) => break Ok(value),
                    Err(e) => {
                        let err_ref = $crate::macros::retry::cast_err(&e, $cast);

                        let std::ops::ControlFlow::Continue((action, duration)) =
                            $crate::runner::next_step(&mut state, config, err_ref)
                        else {
                            break Err(e);
                        };

                        if let Some(action) = action {
                            action().await;
                        }
                        if let Some(duration) = duration {
                            tokio::time::sleep(duration).await;
                        }
                    }
                }
            }
        }
    };
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::{
        backoff::{Exponential, Fixed},
        config::RetryConfigBuilder,
        handler::{ErrorDecision, ErrorHandler},
        types::DynError,
    };
    use std::{
        error::Error as StdError,
        fmt::Display,
        sync::atomic::{AtomicU32, Ordering},
        time::Duration,
    };

    #[derive(Debug, PartialEq)]
    struct CustomError;

    impl Display for CustomError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "CustomError")
        }
    }

    impl StdError for CustomError {}

    struct TestPolicy;

    impl ErrorHandler for TestPolicy {
        type Err = CustomError;

        fn handle<'a>(
            &self,
            e: &'a Self::Err,
            attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<'a, Self::Err> {
            if attempt < 3 {
                ErrorDecision::RetryImmediately
            } else {
                ErrorDecision::Propagate(e)
            }
        }
    }

    struct DynPolicy;

    impl ErrorHandler for DynPolicy {
        type Err = DynError;

        fn handle<'a>(
            &self,
            e: &'a Self::Err,
            attempt: u32,
            _backoff: Duration,
        ) -> ErrorDecision<'a, Self::Err> {
            if attempt < 2 {
                ErrorDecision::RetryImmediately
            } else {
                ErrorDecision::Propagate(e)
            }
        }
    }

    #[tokio::test]
    async fn retry_macro_without_cast_success() {
        let config = RetryConfigBuilder::new()
            .max_retries(3)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(TestPolicy)
            .build();

        let attempts = AtomicU32::new(0);

        let result: Result<&str, CustomError> = retry!(
            {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    Err(CustomError)
                } else {
                    Ok("success")
                }
            },
            config
        )
        .await;

        assert_eq!(result, Ok("success"));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_macro_with_cast_success() {
        let config = RetryConfigBuilder::new()
            .max_retries(3)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(DynPolicy)
            .build();

        let attempts = AtomicU32::new(0);

        let result: Result<&str, CustomError> = retry!(
            {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count < 1 {
                    Err(CustomError)
                } else {
                    Ok("success")
                }
            },
            config,
            |e| e as &DynError
        )
        .await;

        assert_eq!(result, Ok("success"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_macro_propagates_error_on_exhaustion() {
        let config = RetryConfigBuilder::new()
            .max_retries(1)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(TestPolicy)
            .build();

        let attempts = AtomicU32::new(0);

        let result: Result<&str, CustomError> = retry!(
            {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(CustomError)
            },
            config
        )
        .await;

        assert_eq!(result, Err(CustomError));
        // Initial attempt (1) + max 1 retry = 2 attempts total
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_with_executes_action() {
        use std::sync::Arc;

        struct ActionPolicy {
            ran: Arc<AtomicU32>,
        }
        impl ErrorHandler for ActionPolicy {
            type Err = CustomError;
            fn handle<'a>(
                &self,
                _e: &'a Self::Err,
                _attempt: u32,
                backoff: Duration,
            ) -> ErrorDecision<'a, Self::Err> {
                let ran = Arc::clone(&self.ran);
                ErrorDecision::RetryWith(
                    backoff,
                    Box::new(move || {
                        Box::pin(async move {
                            ran.fetch_add(1, Ordering::SeqCst);
                        })
                    }),
                )
            }
        }

        let ran = Arc::new(AtomicU32::new(0));
        let config = RetryConfigBuilder::new()
            .max_retries(1)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(ActionPolicy {
                ran: Arc::clone(&ran),
            })
            .build();

        let _ = retry!({ Err::<i32, CustomError>(CustomError) }, config).await;
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn print_future_size() {
        async fn simple() -> Result<u32, CustomError> {
            Ok(42)
        }
        println!("no future: {}", std::mem::size_of_val(&simple()));

        let config = RetryConfigBuilder::new()
            .max_retries(1)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(TestPolicy)
            .build();

        let future = retry!({ Ok::<u32, CustomError>(42) }, config); // The size of the future last time checked was 232
        println!("retry future: {}", std::mem::size_of_val(&future));

        let mut state = config.create_state();
        let step_future = crate::runner::next_step(&mut state, &config, &CustomError);
        println!("next_step future: {}", std::mem::size_of_val(&step_future));
    }

    #[test]
    fn print_future_size_with_different_policy() {
        let fixed_config = RetryConfigBuilder::new()
            .max_retries(1)
            .backoff(Fixed::new(Duration::ZERO))
            .handler(TestPolicy)
            .build();

        let future = retry!({ Ok::<u32, CustomError>(42) }, fixed_config);
        println!(
            "retry future with fixed policy: {}",
            std::mem::size_of_val(&future)
        );

        let exp_config = RetryConfigBuilder::new()
            .max_retries(1)
            .backoff(Exponential::new(Duration::ZERO, Duration::MAX))
            .handler(TestPolicy)
            .build();

        let future = retry!({ Ok::<u32, CustomError>(42) }, exp_config);
        println!(
            "retry future with exponential policy: {}",
            std::mem::size_of_val(&future)
        );
    }
}
