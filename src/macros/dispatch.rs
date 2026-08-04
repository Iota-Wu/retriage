/// Dispatches an error to a concrete error type handler via downcast,
/// without requiring manual `downcast_ref` calls.
///
/// # Type support
///
/// | Error type | Works with `dispatch!` | Note |
/// |---|---|---|
/// | `anyhow::Error` | ✓ | Exposes `.downcast_ref()` directly |
/// | `thiserror` enum | — | Use `match` directly, no downcasting needed |
/// | `dyn std::error::Error` | ✓ | Supported via `impl dyn Error` in `std` |
/// | `Box<dyn std::error::Error>` | ✓ | Derefs to `&dyn Error` automatically |
/// `dispatch!` works with any error representation that exposes or derefs to a `.downcast_ref::<T>()`
/// method (such as `anyhow::Error`, `&dyn std::error::Error`, or `Box<dyn std::error::Error>`).
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
/// use retriage::{handler::{ErrorDecision, ErrorHandler}, dispatch};
///
/// struct MyPolicy;
///
/// impl ErrorHandler for MyPolicy {
///     type Err = anyhow::Error;
///
///     fn handle<'a>(
///         &self,
///         e: &'a Self::Err,
///         _attempt: u32,
///         backoff: std::time::Duration,
///     ) -> ErrorDecision<'a, Self::Err> {
///         dispatch! {
///             e,
///             reqwest::Error => |e| {
///                 if e.is_timeout() {
///                     tracing::warn!("timeout error: {e}");
///                     ErrorDecision::RetryAfter(backoff)
///                 } else {
///                     ErrorDecision::Propagate(anyhow::anyhow!(e.to_string()))
///                 }
///             },
///             sqlx::Error => |e| {
///                 tracing::warn!("sqlx error: {e}");
///                 ErrorDecision::RetryImmediately
///             },
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::{error::Error, fmt};

    #[derive(Debug, PartialEq)]
    struct CustomTimeoutError;

    impl fmt::Display for CustomTimeoutError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "timeout error")
        }
    }
    impl Error for CustomTimeoutError {}

    #[derive(Debug, PartialEq)]
    struct CustomAuthError;

    impl fmt::Display for CustomAuthError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "auth error")
        }
    }
    impl Error for CustomAuthError {}

    #[test]
    fn dispatch_with_dyn_error_trait_object() {
        let err: &dyn Error = &CustomTimeoutError;

        let result = dispatch! {
            err,
            CustomTimeoutError => |_e| "retry_timeout",
            CustomAuthError => |_e| "retry_auth",
            _ => "propagate",
        };

        assert_eq!(result, "retry_timeout");
    }

    #[test]
    fn dispatch_with_boxed_dyn_error() {
        use crate::DynError;

        let err: Box<DynError> = Box::new(CustomAuthError);

        let result = dispatch! {
            err,
            CustomTimeoutError => |_e| "retry_timeout",
            CustomAuthError => |_e| "retry_auth",
            _ => "propagate",
        };

        assert_eq!(result, "retry_auth");
    }

    #[test]
    fn dispatch_fallback_arm() {
        let err: &dyn Error = &CustomTimeoutError;

        let result = dispatch! {
            err,
            CustomAuthError => |_e| "retry_auth",
            _ => "fallback",
        };

        assert_eq!(result, "fallback");
    }

    #[test]
    fn dispatch_with_anyhow_error() {
        let err = anyhow::Error::new(CustomTimeoutError);

        let result = dispatch! {
            err,
            CustomTimeoutError => |_e| "retry_timeout",
            _ => "fallback",
        };

        assert_eq!(result, "retry_timeout");
    }
}
