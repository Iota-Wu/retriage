use super::{Jitter, NoJitter};
use std::time::Duration;

// ── Fixed ─────────────────────────────────────────────────────────────────────

/// Returns the same delay for every attempt.
///
/// Implements [`Iterator`] — each call to `.next()` yields the same duration
/// with jitter applied. Because the delay never changes, `Fixed` is suitable
/// for simple polling loops or when you want predictable retry intervals.
///
/// # Example
///
/// ```rust,ignore
/// use triage::backoff::strategy::Fixed;
/// use triage::backoff::FullJitter;
/// use std::time::Duration;
///
/// // No jitter — always 500ms
/// let backoff = Fixed::new(Duration::from_millis(500));
///
/// // With jitter — samples uniformly from [0, 500ms]
/// let backoff = Fixed::with_jitter(Duration::from_millis(500), FullJitter);
/// ```
#[derive(Clone, Copy)]
pub struct Fixed<J: Jitter = NoJitter> {
    delay: Duration,
    jitter: J,
}

impl Fixed {
    #[must_use]
    pub const fn new(delay: Duration) -> Self {
        Self {
            delay,
            jitter: NoJitter,
        }
    }
}

impl<J: Jitter> Fixed<J> {
    #[must_use]
    pub const fn with_jitter(delay: Duration, jitter: J) -> Self {
        Self { delay, jitter }
    }
}

impl<J: Jitter> Iterator for Fixed<J> {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.jitter.apply(self.delay))
    }
}

// ── Linear ────────────────────────────────────────────────────────────────────

/// Increases the delay linearly with each attempt: `base * attempt`.
///
/// Implements [`Iterator`] — each call to `.next()` advances the attempt
/// counter and yields the next duration.
///
/// | Attempt | Delay (base = 200ms) |
/// |---------|----------------------|
/// | 1       | 200ms                |
/// | 2       | 400ms                |
/// | 3       | 600ms                |
///
///
/// # Example
///
/// ```rust,ignore
/// use triage::backoff::strategy::Linear;
/// use triage::backoff::BoundedJitter;
/// use std::time::Duration;
///
/// let backoff = Linear::new(Duration::from_millis(200), Duration::from_secs(10));
///
/// // With ±20% jitter
/// let backoff = Linear::with_jitter(Duration::from_millis(200), Duration::from_secs(10), BoundedJitter::new(0.2));
/// ```
#[derive(Clone, Copy)]
pub struct Linear<J: Jitter = NoJitter> {
    base: Duration,
    max: Duration,
    jitter: J,
    attempt: u32,
}

impl Linear {
    #[must_use]
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            jitter: NoJitter,
            attempt: 1,
        }
    }
}

impl<J: Jitter> Linear<J> {
    #[must_use]
    pub const fn with_jitter(base: Duration, max: Duration, jitter: J) -> Self {
        Self {
            base,
            max,
            jitter,
            attempt: 1,
        }
    }
}

impl<J: Jitter> Iterator for Linear<J> {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        let delay = self.base.saturating_mul(self.attempt);
        let capped = delay.min(self.max);

        self.attempt += 1;

        Some(self.jitter.apply(capped))
    }
}

// ── Exponential ───────────────────────────────────────────────────────────────

/// Increases the delay exponentially: `base * multiplier ^ (attempt - 1)`.
///
/// Implements [`Iterator`] — each call to `.next()` advances the attempt
/// counter and yields the next duration.
///
/// Defaults to `multiplier = 2.0` (classic exponential backoff).
///
/// | Attempt | Delay (base = 100ms, multiplier = 2.0) |
/// |---------|----------------------------------------|
/// | 1       | 100ms                                  |
/// | 2       | 200ms                                  |
/// | 3       | 400ms                                  |
/// | 4       | 800ms                                  |
///
/// Use [`Exponential::multiplier`] to tune the growth rate.
///
/// # Example
///
/// ```rust,ignore
/// use triage::backoff::strategy::Exponential;
/// use triage::backoff::FullJitter;
/// use std::time::Duration;
///
/// // Classic exponential backoff, capped at 30s
/// let backoff = Exponential::new(Duration::from_millis(100), Duration::from_secs(30));
///
/// // Gentler growth with full jitter — recommended for distributed systems
/// let backoff = Exponential::with_jitter(
///         Duration::from_millis(100),
///         Duration::from_secs(30),
///         FullJitter,
///     )
///     .multiplier(1.5);
/// ```
#[derive(Clone, Copy)]
pub struct Exponential<J: Jitter = NoJitter> {
    base: Duration,
    multiplier: f64,
    max: Duration,
    jitter: J,
    attempt: u32,
}

impl Exponential {
    #[must_use]
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            multiplier: 2.0,
            max,
            jitter: NoJitter,
            attempt: 1,
        }
    }
}

impl<J: Jitter> Exponential<J> {
    pub const fn with_jitter(base: Duration, max: Duration, jitter: J) -> Self {
        Self {
            base,
            multiplier: 2.0,
            max,
            jitter,
            attempt: 1,
        }
    }
    /// Sets the growth multiplier. Defaults to `2.0`.
    ///
    /// Values between `1.0` and `2.0` give gentler growth.
    /// Values below `1.0` will cause the delay to shrink — not recommended.
    #[must_use]
    pub const fn multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }
}

impl<J: Jitter> Iterator for Exponential<J> {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        let exponent =
            i32::try_from(self.attempt - 1).expect("Attempt should not bigger than i32::MAX");
        let factor = self.multiplier.powi(exponent);
        let raw = self.base.as_secs_f64() * factor;
        let capped = if raw.is_finite() {
            Duration::from_secs_f64(raw).min(self.max)
        } else {
            self.max
        };

        self.attempt += 1;

        Some(self.jitter.apply(capped))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    // Fixed

    #[test]
    fn fixed_always_same() {
        let mut b = Fixed::new(Duration::from_millis(300));
        for _ in 1..=10 {
            assert_eq!(b.next().unwrap(), Duration::from_millis(300));
        }
    }

    // Linear

    #[test]
    fn linear_scales_with_attempt() {
        let mut b = Linear::new(Duration::from_millis(100), Duration::MAX);
        assert_eq!(b.next().unwrap(), Duration::from_millis(100));
        assert_eq!(b.next().unwrap(), Duration::from_millis(200));
        assert_eq!(b.next().unwrap(), Duration::from_millis(300));
    }

    #[test]
    fn linear_respects_max() {
        let mut b = Linear::new(Duration::from_millis(100), Duration::from_millis(250));
        assert_eq!(b.next().unwrap(), Duration::from_millis(100)); // 100
        assert_eq!(b.next().unwrap(), Duration::from_millis(200)); // 200
        assert_eq!(b.next().unwrap(), Duration::from_millis(250)); // 300 → capped
        assert_eq!(b.next().unwrap(), Duration::from_millis(250)); // 400 → capped
    }

    // Exponential

    #[test]
    fn exponential_doubles_by_default() {
        let mut b = Exponential::new(Duration::from_millis(100), Duration::MAX);
        assert_eq!(b.next().unwrap(), Duration::from_millis(100));
        assert_eq!(b.next().unwrap(), Duration::from_millis(200));
        assert_eq!(b.next().unwrap(), Duration::from_millis(400));
        assert_eq!(b.next().unwrap(), Duration::from_millis(800));
    }

    #[test]
    fn exponential_custom_multiplier() {
        let mut b = Exponential::new(Duration::from_millis(100), Duration::MAX).multiplier(1.5);
        assert_eq!(b.next().unwrap(), Duration::from_millis(100));
        assert_eq!(b.next().unwrap(), Duration::from_millis(150)); // 100 * 1.5
        assert_eq!(b.next().unwrap(), Duration::from_millis(225)); // 100 * 1.5^2
    }

    #[test]
    fn exponential_respects_max() {
        let mut b = Exponential::new(Duration::from_millis(100), Duration::from_millis(300));
        assert_eq!(b.next().unwrap(), Duration::from_millis(100));
        assert_eq!(b.next().unwrap(), Duration::from_millis(200));
        assert_eq!(b.next().unwrap(), Duration::from_millis(300)); // capped
        assert_eq!(b.next().unwrap(), Duration::from_millis(300)); // still capped
    }

    // Jitter integration

    #[test]
    fn fixed_with_full_jitter_within_bounds() {
        use crate::backoff::jitter::FullJitter;
        let base = Duration::from_millis(500);
        let mut b = Fixed::with_jitter(base, FullJitter);
        for _ in 0..500 {
            assert!(b.next().unwrap() <= base);
        }
    }

    #[test]
    fn exponential_with_bounded_jitter() {
        use crate::backoff::jitter::BoundedJitter;
        let mut b = Exponential::with_jitter(
            Duration::from_millis(100),
            Duration::MAX,
            BoundedJitter::new(0.2),
        );

        b.next(); // attempt 1 = 100ms, skip
        // attempt 2: 200ms ±20% → [160ms, 240ms]
        let delay = b.next().unwrap();
        assert!(delay >= Duration::from_millis(160));
        assert!(delay <= Duration::from_millis(240));
    }
}
