<!-- cargo-rdme start -->

# retriage

Ergonomic recoverable error handling with retry strategies for Rust.

## Scope

`retriage` handles **retry logic and error classification only**.
The following concerns are intentionally out of scope:

- **Rate limiting / traffic shaping** — use [`governor`] or similar
- **Circuit breaking** — use [`failsafe`] or [`tower`]'s circuit breaker
- **Timeout management** — wrap your future with [`tokio::time::timeout`](https://docs.rs/tokio/latest/tokio/time/timeout/fn.timeout.html)
- **Logging** — use [`tracing`] or similar

If you need rate limiting alongside retries, manage it inside your closure:

```rust
let limiter = Arc::new(RateLimiter::direct(Quota::per_second(10)));

retry!(
    {
        limiter.until_ready().await;  // retriage `does not` manage rate limiting
        foo().await
    },
    policy
).await
```
For logging, emit log events directly within `ErrorHandler::handle`:

```rust
impl ErrorHandler for MyPolicy {
    type Err = anyhow::Error;

    fn handle<'a>(
        &self,
        e: &'a Self::Err,
        attempt: u32,
        backoff: Duration,
    ) -> ErrorDecision<'a, Self::Err> {
        tracing::warn!(attempt, ?backoff, %e, "Retrying operation"); // retriage `does not` manage logging
        ErrorDecision::RetryAfter(backoff)
    }
}
```

[`governor`]: https://docs.rs/governor
[`failsafe`]: https://docs.rs/failsafe
[`tower`]: https://docs.rs/tower
[`tracing`]: https://docs.rs/tracing

## Runtime requirement

`retriage` uses [`tokio::time::sleep`](https://docs.rs/tokio/latest/tokio/time/sleep/fn.sleep.html) internally and requires a Tokio runtime.

```toml
[dependencies]
tokio = { version = "1", features = ["time"] }
```

## Error types

[`ErrorHandler::Err`](https://docs.rs/retriage/latest/retriage/handler/trait.ErrorHandler.html#associatedtype.Err) accepts any `Send + 'static` type.

| Error type | `handle` | [`dispatch!`](https://docs.rs/retriage/latest/retriage/macro.dispatch.html) |
|---|---|---|
| `anyhow::Error` | ✓ | ✓ |
| `thiserror` enum | ✓ use `match` directly | — not needed |
| `Box<dyn std::error::Error>` | ✓ | ✓ |
| `dyn std::error::Error` (trait object) | ✓ (due to `?Sized`) | ✓ |

`std::error::Error` is intentionally absent from the bound so that non-`std::error::Error`
types like `anyhow::Error` can be used directly. The `?Sized` bound enables handling
unsized trait objects (dyn `std::error::Error`) transparently.


[`RetryConfigBuilder`](https://docs.rs/retriage/latest/retriage/config/struct.RetryConfigBuilder.html) is not `const`, so if you need a single config
shared across your application, use [`std::sync::LazyLock`](https://doc.rust-lang.org/stable/std/sync/lazy_lock/struct.LazyLock.html) to avoid
rebuilding it on every call:

```rust
use std::sync::LazyLock;
use retriage::{
    ExponentialConfig, RetryConfigBuilder,
    backoff::{Exponential, FullJitter},
    retry,
};
use std::time::Duration;

// Note: `static` requires fully explicit type parameters — `_` is not allowed.
static RETRY_CONFIG: LazyLock<ExponentialConfig<SqlitePolicy, FullJitter>> =
    LazyLock::new(|| {
        let backoff = Exponential::with_jitter(
            Duration::from_millis(100),
            Duration::from_millis(1650),
            FullJitter,
        );

        RetryConfigBuilder::new()
            .max_retries(4)
            .backoff(backoff)
            .handler(SqlitePolicy)
            .build()
        });

// elsewhere
let foo = retry!({ bar().await }, &*RETRY_CONFIG).await?;
```

## Known limitations

**Automatic error type coercion** — when a block returns a different error
type than `H::Err`, use the cast parameter to unify them on the stack
without heap allocation:

```rust
retry!({ foo().await }, config, |e| e as &DynError).await?;
retry!({ foo().await }, config, |e| e.as_ref()).await?;
```

Full automatic coercion (without the cast parameter) is a planned feature.

<!-- cargo-rdme end -->