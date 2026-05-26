use std::time::Duration;

/// Controls how randomness is applied to a base delay.
///
/// Implement this trait to provide custom jitter strategies.
/// The input `base` is the delay calculated by the backoff strategy,
/// and the output is the final delay after jitter is applied.
pub trait Jitter: Send + Sync {
    fn apply(&self, base: Duration) -> Duration;
}

// ── Built-in implementations ──────────────────────────────────────────────────

/// No jitter — returns the base delay unchanged.
#[derive(Clone, Copy)]
pub struct NoJitter;

impl Jitter for NoJitter {
    fn apply(&self, base: Duration) -> Duration {
        base
    }
}

/// Full jitter — samples uniformly from [0, base].
///
/// Recommended for most distributed systems workloads as it
/// spreads retry storms most effectively.
#[derive(Clone, Copy)]
pub struct FullJitter;

impl Jitter for FullJitter {
    fn apply(&self, base: Duration) -> Duration {
        use rand::RngExt;

        let secs = rand::rng().random_range(0.0..=base.as_secs_f64());
        Duration::from_secs_f64(secs)
    }
}

/// Bounded jitter — samples from [base * (1 - factor), base * (1 + factor)].
///
/// Keeps the delay close to the base while still adding enough randomness
/// to avoid thundering herd. `factor` is clamped to [0.0, 1.0].
#[derive(Clone, Copy)]
pub struct BoundedJitter {
    factor: f64,
}

impl BoundedJitter {
    /// `factor` controls the spread around the base delay.
    /// e.g. factor = 0.2 means ±20% of base.
    pub fn new(factor: f64) -> Self {
        Self {
            factor: factor.clamp(0.0, 1.0),
        }
    }
}

impl Jitter for BoundedJitter {
    fn apply(&self, base: Duration) -> Duration {
        use rand::RngExt;

        let base_secs = base.as_secs_f64();
        let low = base_secs * (1.0 - self.factor);
        let high = base_secs * (1.0 + self.factor);
        let secs = rand::rng().random_range(low..=high);
        Duration::from_secs_f64(secs)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_jitter_is_identity() {
        let base = Duration::from_millis(500);
        assert_eq!(NoJitter.apply(base), base);
    }

    #[test]
    fn full_jitter_within_bounds() {
        let base = Duration::from_millis(500);
        for _ in 0..1000 {
            let result = FullJitter.apply(base);
            assert!(result <= base, "full jitter exceeded base: {:?}", result);
        }
    }

    #[test]
    fn bounded_jitter_within_range() {
        let base = Duration::from_millis(500);
        let jitter = BoundedJitter::new(0.2);
        let low = Duration::from_millis(400);
        let high = Duration::from_millis(600);
        for _ in 0..1000 {
            let result = jitter.apply(base);
            assert!(
                result >= low && result <= high,
                "bounded jitter out of range: {:?}",
                result
            );
        }
    }

    #[test]
    fn bounded_jitter_clamps_factor() {
        // factor > 1.0 should be clamped to 1.0, not panic
        let base = Duration::from_millis(200);
        let jitter = BoundedJitter::new(5.0);
        let result = jitter.apply(base);
        assert!(result <= Duration::from_millis(400));
    }
}
