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
/// the async nature explicit, since the retry loop uses `tokio::time::sleep`
/// internally.
///
/// The block is re-evaluated on every attempt, which avoids higher-ranked
/// lifetime errors that arise when async closures capture borrows
/// (e.g. `sqlx` query builders, `&str` parameters).
///
/// # Syntax
///
/// ```rust,ignore
/// // Without cast — E must match H::Err exactly
/// retry!({ expr }, config).await
///
/// // With cast — convert &E to &H::Err on the error path, zero heap allocation
/// retry!({ expr }, config, |e| e as &DynError).await
/// retry!({ expr }, config, |e: &anyhow::Error| e.as_ref()).await
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use triage::{retry, RetryConfigBuilder};
/// use triage::backoff::Exponential;
/// use std::time::Duration;
///
/// // Simple case — error type matches handler directly
/// let result = retry!({
///     reqwest::get("https://example.com").await
/// }, config).await;
///
/// // Multiple error sources — cast to unify, zero heap allocation
/// let result = retry!({
///     sqlx::query(...).await
/// }, config, |e| e as &triage::DynError).await;
///
/// // anyhow::Error — use as_ref() to get &dyn Error
/// let result = retry!({
///     do_work().await
/// }, config, |e: &anyhow::Error| e.as_ref()).await;
/// ```
///
/// # With rate limiting (out of scope for triage)
///
/// ```rust,ignore
/// retry!({
///     limiter.until_ready().await;
///     do_work().await
/// }, config).await;
/// ```
#[macro_export]
macro_rules! retry {
    // Without cast — &e passed directly, zero cost
    ($blk:block, $config:expr) => {
        async {
            let config = &$config;
            let mut state = config.create_state();
            loop {
                match $blk {
                    Ok(value) => break Ok(value),
                    Err(e) => {
                        if $crate::runner::next_step(&mut state, config, &e)
                            .await
                            .is_break()
                        {
                            break Err(e);
                        }
                    }
                }
            }
        }
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

                        if $crate::runner::next_step(&mut state, config, err_ref)
                            .await
                            .is_break()
                        {
                            break Err(e);
                        }
                    }
                }
            }
        }
    };
}
