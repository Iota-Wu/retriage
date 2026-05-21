use super::{Jitter, NoJitter};
use std::time::Duration;

/// Calculates the delay before the next retry attempt.
///
/// Implement this trait to provide custom backoff strategies.
/// `attempt` is 1-indexed — the first retry is attempt 1.
pub trait BackoffStrategy: Send + Sync {
    fn next_delay(&self, attempt: u32) -> Duration;
}

// ── Fixed ─────────────────────────────────────────────────────────────────────

/// Returns the same delay for every attempt.
///
/// ```
/// use triage::backoff::strategy::Fixed;
/// use std::time::Duration;
///
/// let backoff = Fixed::new(Duration::from_millis(500));
/// ```
pub struct Fixed<J: Jitter = NoJitter> {
    delay: Duration,
    jitter: J,
}

impl Fixed {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            jitter: NoJitter,
        }
    }
}

impl<J: Jitter> Fixed<J> {
    pub fn with_jitter(delay: Duration, jitter: J) -> Self {
        Self { delay, jitter }
    }
}

impl<J: Jitter> BackoffStrategy for Fixed<J> {
    fn next_delay(&self, _attempt: u32) -> Duration {
        self.jitter.apply(self.delay)
    }
}

// ── Linear ────────────────────────────────────────────────────────────────────

/// Increases the delay linearly: base * attempt.
///
/// attempt 1 → base, attempt 2 → base * 2, attempt 3 → base * 3 ...
///
/// ```
/// use triage::backoff::strategy::Linear;
/// use std::time::Duration;
///
/// let backoff = Linear::new(Duration::from_millis(200));
/// ```
pub struct Linear<J: Jitter = NoJitter> {
    base: Duration,
    max: Option<Duration>,
    jitter: J,
}

impl Linear {
    pub fn new(base: Duration) -> Self {
        Self {
            base,
            max: None,
            jitter: NoJitter,
        }
    }
}

impl<J: Jitter> Linear<J> {
    pub fn with_jitter(base: Duration, jitter: J) -> Self {
        Self {
            base,
            max: None,
            jitter,
        }
    }

    /// Caps the delay at `max` before jitter is applied.
    pub fn max_delay(mut self, max: Duration) -> Self {
        self.max = Some(max);
        self
    }
}

impl<J: Jitter> BackoffStrategy for Linear<J> {
    fn next_delay(&self, attempt: u32) -> Duration {
        let delay = self.base * attempt;
        let capped = match self.max {
            Some(max) => delay.min(max),
            None => delay,
        };
        self.jitter.apply(capped)
    }
}

// ── Exponential ───────────────────────────────────────────────────────────────

/// Increases the delay exponentially: base * multiplier ^ (attempt - 1).
///
/// attempt 1 → base, attempt 2 → base * m, attempt 3 → base * m² ...
///
/// Defaults to multiplier = 2.0 (classic exponential backoff).
///
/// ```
/// use triage::backoff::strategy::Exponential;
/// use std::time::Duration;
///
/// let backoff = Exponential::new(Duration::from_millis(100));
/// let backoff_custom = Exponential::new(Duration::from_millis(100))
///     .multiplier(1.5)
///     .max_delay(Duration::from_secs(30));
/// ```
pub struct Exponential<J: Jitter = NoJitter> {
    base: Duration,
    multiplier: f64,
    max: Option<Duration>,
    jitter: J,
}

impl Exponential {
    pub fn new(base: Duration) -> Self {
        Self {
            base,
            multiplier: 2.0,
            max: None,
            jitter: NoJitter,
        }
    }
}

impl<J: Jitter> Exponential<J> {
    pub fn with_jitter(base: Duration, jitter: J) -> Self {
        Self {
            base,
            multiplier: 2.0,
            max: None,
            jitter,
        }
    }

    pub fn multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }

    /// Caps the delay at `max` before jitter is applied.
    pub fn max_delay(mut self, max: Duration) -> Self {
        self.max = Some(max);
        self
    }
}

impl<J: Jitter> BackoffStrategy for Exponential<J> {
    fn next_delay(&self, attempt: u32) -> Duration {
        let factor = self.multiplier.powi(attempt as i32 - 1);
        let delay = Duration::from_secs_f64(self.base.as_secs_f64() * factor);
        let capped = match self.max {
            Some(max) => delay.min(max),
            None => delay,
        };
        self.jitter.apply(capped)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Fixed

    #[test]
    fn fixed_always_same() {
        let b = Fixed::new(Duration::from_millis(300));
        for i in 1..=10 {
            assert_eq!(b.next_delay(i), Duration::from_millis(300));
        }
    }

    // Linear

    #[test]
    fn linear_scales_with_attempt() {
        let b = Linear::new(Duration::from_millis(100));
        assert_eq!(b.next_delay(1), Duration::from_millis(100));
        assert_eq!(b.next_delay(2), Duration::from_millis(200));
        assert_eq!(b.next_delay(5), Duration::from_millis(500));
    }

    #[test]
    fn linear_respects_max() {
        let b = Linear::new(Duration::from_millis(100)).max_delay(Duration::from_millis(250));
        assert_eq!(b.next_delay(1), Duration::from_millis(100));
        assert_eq!(b.next_delay(2), Duration::from_millis(200));
        assert_eq!(b.next_delay(3), Duration::from_millis(250)); // capped
        assert_eq!(b.next_delay(9), Duration::from_millis(250)); // still capped
    }

    // Exponential

    #[test]
    fn exponential_doubles_by_default() {
        let b = Exponential::new(Duration::from_millis(100));
        assert_eq!(b.next_delay(1), Duration::from_millis(100));
        assert_eq!(b.next_delay(2), Duration::from_millis(200));
        assert_eq!(b.next_delay(3), Duration::from_millis(400));
        assert_eq!(b.next_delay(4), Duration::from_millis(800));
    }

    #[test]
    fn exponential_custom_multiplier() {
        let b = Exponential::new(Duration::from_millis(100)).multiplier(1.5);
        assert_eq!(b.next_delay(1), Duration::from_millis(100));
        // 100 * 1.5^1 = 150
        assert_eq!(b.next_delay(2), Duration::from_millis(150));
    }

    #[test]
    fn exponential_respects_max() {
        let b = Exponential::new(Duration::from_millis(100)).max_delay(Duration::from_millis(300));
        assert_eq!(b.next_delay(1), Duration::from_millis(100));
        assert_eq!(b.next_delay(2), Duration::from_millis(200));
        assert_eq!(b.next_delay(3), Duration::from_millis(300)); // capped
        assert_eq!(b.next_delay(4), Duration::from_millis(300)); // still capped
    }

    // Jitter integration

    #[test]
    fn fixed_with_full_jitter_within_bounds() {
        use crate::backoff::jitter::FullJitter;
        let base = Duration::from_millis(500);
        let b = Fixed::with_jitter(base, FullJitter);
        for _ in 0..500 {
            assert!(b.next_delay(1) <= base);
        }
    }

    #[test]
    fn exponential_with_bounded_jitter() {
        use crate::backoff::jitter::BoundedJitter;
        let b = Exponential::with_jitter(Duration::from_millis(100), BoundedJitter::new(0.2))
            .max_delay(Duration::from_secs(30));

        // attempt 2: base delay = 200ms, jitter ±20% → [160ms, 240ms]
        let delay = b.next_delay(2);
        assert!(delay >= Duration::from_millis(160));
        assert!(delay <= Duration::from_millis(240));
    }
}
