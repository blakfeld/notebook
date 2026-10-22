//! Exponential backoff with full jitter.

use std::time::Duration;

use crate::crypto::fill_random;

/// Delays for consecutive failed attempts: attempt `n` waits a uniformly
/// random time in `[0, min(cap, base * 2^n)]`.
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
    jitter: bool,
}

impl Backoff {
    /// `jitter: false` always waits the full ceiling, for deterministic tests.
    pub(crate) fn new(base: Duration, cap: Duration, jitter: bool) -> Self {
        Self {
            base,
            cap,
            attempt: 0,
            jitter,
        }
    }

    /// The upper bound of the next delay.
    pub(crate) fn ceiling(&self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap)
    }

    /// The delay before the next attempt, and counts the failure.
    pub(crate) fn next_delay(&mut self) -> Duration {
        let ceiling = self.ceiling();
        self.attempt = self.attempt.saturating_add(1);
        if !self.jitter {
            return ceiling;
        }
        let mut bytes = [0; 8];
        if fill_random(&mut bytes).is_err() {
            return ceiling;
        }
        let max = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(u64::from_le_bytes(bytes) % max.saturating_add(1))
    }

    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_to_the_cap_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(60), false);
        let delays: Vec<u64> = (0..9).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn jitter_stays_under_the_ceiling() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(60), true);
        for _ in 0..100 {
            let ceiling = b.ceiling();
            assert!(b.next_delay() <= ceiling);
        }
        // Far past any shift width, still capped.
        assert_eq!(b.ceiling(), Duration::from_secs(60));
    }
}
