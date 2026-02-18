use std::sync::Arc;
use std::time::Instant;

/// Microsecond-resolution timestamp relative to session start.
pub type TimestampUs = u64;

/// High-resolution monotonic clock for synchronizing capture streams.
///
/// All timestamps are relative to when the clock was created, expressed
/// in microseconds. This ensures monotonic, comparable timestamps across
/// video, input, and audio streams.
///
/// Cloning a `SyncClock` shares the same epoch, so all clones produce
/// timestamps relative to the same origin.
#[derive(Clone)]
pub struct SyncClock {
    epoch: Arc<Instant>,
}

impl SyncClock {
    pub fn new() -> Self {
        Self {
            epoch: Arc::new(Instant::now()),
        }
    }

    /// Returns the current timestamp in microseconds since clock creation.
    pub fn now_us(&self) -> TimestampUs {
        self.epoch.elapsed().as_micros() as u64
    }
}

impl Default for SyncClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn timestamps_are_monotonic() {
        let clock = SyncClock::new();
        let t1 = clock.now_us();
        thread::sleep(Duration::from_millis(1));
        let t2 = clock.now_us();
        thread::sleep(Duration::from_millis(1));
        let t3 = clock.now_us();

        assert!(t2 > t1, "t2={t2} should be > t1={t1}");
        assert!(t3 > t2, "t3={t3} should be > t2={t2}");
    }

    #[test]
    fn timestamps_start_near_zero() {
        let clock = SyncClock::new();
        let t = clock.now_us();
        assert!(t < 1_000_000, "first timestamp should be <1s, got {t}us");
    }

    #[test]
    fn clones_share_epoch() {
        let clock1 = SyncClock::new();
        let clock2 = clock1.clone();

        let t1 = clock1.now_us();
        let t2 = clock2.now_us();

        // Both clones should report similar timestamps (within 1ms)
        let diff = if t2 > t1 { t2 - t1 } else { t1 - t2 };
        assert!(diff < 1_000, "clones should share epoch, diff={diff}us");
    }

    #[test]
    fn timestamps_reflect_elapsed_time() {
        let clock = SyncClock::new();
        thread::sleep(Duration::from_millis(50));
        let t = clock.now_us();
        assert!(
            t >= 40_000 && t < 200_000,
            "after 50ms sleep, expected ~50000us, got {t}us"
        );
    }
}
