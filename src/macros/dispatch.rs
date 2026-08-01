/// Dispatches an error to a concrete error type handler via downcast,
/// without requiring manual `downcast_ref` calls.
///
/// # Type support
///
/// | Error type | Works with `dispatch!` |
/// |---|---|
/// | `anyhow::Error` | ✓ |
/// | `dyn std::error::Error` | ✓ — via `impl dyn Error` in std |
/// | `thiserror` enum | use `match` directly, no downcast needed |
/// | `Box<dyn Error>` | ✗ — use `anyhow` or `dyn Error` instead |
///
/// Both `anyhow::Error` and `&dyn std::error::Error` expose `downcast_ref`,
/// so `dispatch!` works with either. `Box<dyn Error>` is not supported
/// as it does not implement `std::error::Error` itself.
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
