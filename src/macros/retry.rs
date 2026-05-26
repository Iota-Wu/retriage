/// Executes a block with retry logic defined by a [`RetryConfig`].
///
/// Expands to an `async` block that loops until the inner block succeeds,
/// all attempts are exhausted, or the error handler returns
/// [`ErrorDecision::Propagate`].
///
/// The block is re-evaluated on every attempt, so any setup inside it
/// runs each time. This is intentional — it allows borrows and temporaries
/// (e.g. `sqlx` query builders, `&str` parameters) to be created fresh
/// each attempt without hitting higher-ranked lifetime errors.
///
/// # Syntax
///
/// ```rust,ignore
/// retry!({ expr }, config)
/// ```
///
/// The result is an `async` block — use it directly in an `async` context
/// or `.await` it at the call site.
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
/// // Simple async expression
/// let result = retry!({
///     reqwest::get("https://example.com").await
/// }, config).await;
///
/// // Works with sqlx — no clone needed
/// let row = retry!({
///     sqlx::query_as::<_, MyRow>("SELECT * FROM t WHERE id = $1")
///         .bind(id)
///         .fetch_one(&pool)
///         .await
/// }, config).await?;
/// ```
///
/// # With rate limiting (out of scope for triage)
///
/// ```rust,ignore
/// let result = retry!({
///     limiter.until_ready().await;
///     do_work().await
/// }, config).await;
/// ```
#[macro_export]
macro_rules! retry {
    ($blk:block, $config:expr) => {
        async {
            let config = &$config;

            let mut state = config.create_state();

            loop {
                match $blk {
                    Ok(value) => break Ok(value),
                    Err(e) => {
                        if let Err(final_err) =
                            $crate::runner::next_step(&mut state, config, e).await
                        {
                            break Err(final_err);
                        }
                    }
                }
            }
        }
    };
}
