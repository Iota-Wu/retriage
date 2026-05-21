//! # triage
//!
//! Ergonomic recoverable error handling with retry strategies for Rust.
//!
//! ## Scope
//!
//! `triage` handles **retry logic and error classification only**.
//! The following concerns are intentionally out of scope:
//!
//! - **Rate limiting / traffic shaping** — use [`governor`] or similar
//! - **Circuit breaking** — use [`failsafe`] or [`tower`]'s circuit breaker
//! - **Timeout management** — wrap your future with [`tokio::time::timeout`]
//!
//! If you need rate limiting alongside retries, manage it inside your closure:
//!
//! ```rust,ignore
//! let limiter = Arc::new(RateLimiter::direct(Quota::per_second(10)));
//!
//! retry!(
//!     || async {
//!         limiter.until_ready().await;  // 流量管理在這裡，triage 不管
//!         reqwest::get(&url).await
//!     },
//!     policy
//! )
//! ```
//!
//! [`governor`]: https://docs.rs/governor
//! [`failsafe`]: https://docs.rs/failsafe
//! [`tower`]: https://docs.rs/tower
//!
//! ## Runtime requirement
//!
//! `triage` uses [`tokio::time::sleep`] internally and requires a Tokio runtime.
//!
//! ```toml
//! [dependencies]
//! tokio = { version = "1", features = ["time"] }
//! ```
//!
//! ## Error types
//!
//! [`ErrorHandler::Err`] accepts any `Send + 'static` type.
//!
//! | Error type | `handle` | [`dispatch!`] |
//! |---|---|---|
//! | `anyhow::Error` | ✓ | ✓ |
//! | `thiserror` enum | ✓ use `match` directly | — not needed |
//! | `Box<dyn Error>` | ✗ | ✗ |
//!
//! `std::error::Error` is intentionally absent from the bound because
//! `anyhow::Error` does not implement it. `Box<dyn Error>` remains
//! unsupported as it does not satisfy `Sized` — use `anyhow::Error::from(e)`
//! as the migration path.
//!
//!
//! [`RetryConfigBuilder`] is not `const`, so if you need a single config
//! shared across your application, use [`std::sync::LazyLock`] to avoid
//! rebuilding it on every call:
//!
//! ```rust,ignore
//! use std::sync::LazyLock;
//! use triage::{RetryConfigBuilder, backoff::Exponential};
//! use std::time::Duration;
//!
//! // Note: `static` requires fully explicit type parameters — `_` is not allowed.
//! static RETRY: LazyLock<RetryConfig<Exponential, MyPolicy>> = LazyLock::new(|| {
//!     RetryConfigBuilder::new()
//!         .attempts(5)
//!         .backoff(Exponential::new(Duration::from_millis(100)))
//!         .handler(MyPolicy)
//!         .build()
//! });
//!
//! // elsewhere
//! let result = retry!(|| async { do_work().await }, *RETRY).await;
//! ```

pub mod backoff;
pub mod config;
pub mod handler;
pub mod runner;
#[macro_use]
pub mod macros;

pub use config::RetryConfigBuilder;
pub use handler::{ErrorDecision, ErrorHandler};
