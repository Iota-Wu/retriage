/// Executes an async closure with retry logic defined by a [`RetryConfig`].
///
/// This is a thin wrapper around [`crate::runner::run`] that `.await`s
/// the result for you, keeping call sites clean.
///
/// # Syntax
///
/// ```rust,ignore
/// retry!(closure, config)
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use triage::{retry, RetryConfigBuilder};
/// use triage::backoff::Exponential;
/// use std::time::Duration;
///
/// let config = RetryConfigBuilder::new()
///     .attempts(5)
///     .backoff(Exponential::new(Duration::from_millis(100)))
///     .handler(MyPolicy)
///     .build();
///
/// let result = retry!(|| async {
///     reqwest::get("https://example.com").await?;
///     Ok(())
/// }, config).await;
/// ```
///
/// # With rate limiting (out of scope for triage)
///
/// ```rust,ignore
/// let limiter = Arc::new(RateLimiter::direct(Quota::per_second(10)));
///
/// let result = retry!(|| async {
///     limiter.until_ready().await;
///     do_work().await
/// }, config).await;
/// ```
#[macro_export]
macro_rules! retry {
    ($f:expr, $config:expr) => {
        $crate::runner::run(&$config, $f)
    };
}
