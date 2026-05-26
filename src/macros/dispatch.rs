/// Dispatches an error to a concrete error type handler via downcast,
/// without requiring manual `downcast_ref` calls.
///
/// # Type support
///
/// | Error type | Works with `dispatch!` |
/// |---|---|
/// | `anyhow::Error` | ✓ — recommended |
/// | `thiserror` enum | use `match` directly, no downcast needed |
/// | `Box<dyn Error>` | ✗ — `dyn Error` has no `downcast_ref`; use `anyhow` instead |
///
/// `dispatch!` requires `downcast_ref`, which is only available on
/// [`anyhow::Error`]. This is a limitation of the standard library —
/// `dyn std::error::Error` does not expose downcasting. If this changes
/// in a future version of Rust, broader support may become possible.
///
/// For `Box<dyn Error>` use cases, wrapping with `anyhow::Error` via
/// `anyhow::Error::from(e)` is the recommended migration path.
///
/// # Usage
///
/// Each arm specifies a concrete type to attempt downcasting to,
/// followed by a closure that receives `&T` directly — no manual
/// `downcast_ref` needed.
///
/// A trailing `_ => expr` catch-all arm is required.
///
/// # Example
///
/// ```rust,ignore
/// use triage::handler::{ErrorDecision, ErrorHandler};
/// use triage::dispatch;
///
/// struct MyPolicy;
///
/// impl ErrorHandler for MyPolicy {
///     type Err = anyhow::Error;
///
///     fn handle(&self, e: &Self::Err, attempt: u32) -> ErrorDecision<Self::Err> {
///         dispatch! {
///             e,
///             reqwest::Error => |e| {
///                 if e.is_timeout() {
///                     ErrorDecision::Retry
///                 } else {
///                     ErrorDecision::Propagate(anyhow::anyhow!(e.to_string()))
///                 }
///             },
///             sqlx::Error => |e| ErrorDecision::Retry,
///             _ => ErrorDecision::Propagate(anyhow::anyhow!("unknown error")),
///         }
///     }
/// }
/// ```
#[macro_export]
macro_rules! dispatch {
    // Base case: only catch-all remains
    (
        $err:expr,
        _ => $fallback:expr $(,)?
    ) => {
        $fallback
    };

    // Recursive case: match closure syntax |var| body directly
    // This lets us bind `var` as &$type without going through closure type inference
    (
        $err:expr,
        $type:ty => |$var:ident| $body:expr,
        $($rest:tt)*
    ) => {
        if let Some($var) = $err.downcast_ref::<$type>() {
            $body
        } else {
            $crate::dispatch!($err, $($rest)*)
        }
    };

    // Recursive case: block body variant |var| { ... }
    (
        $err:expr,
        $type:ty => |$var:ident| $body:block,
        $($rest:tt)*
    ) => {
        if let Some($var) = $err.downcast_ref::<$type>() {
            $body
        } else {
            $crate::dispatch!($err, $($rest)*)
        }
    };
}
