//! Frame clock used by the animation timer.

use std::time::Instant;

/// Measures frame deltas and clamps them so a suspended process never makes the
/// dock jump.
pub struct FrameClock {
    last: Instant,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameClock {
    /// Creates a clock anchored at now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
        }
    }

    /// Resets the anchor, used when a new animation run starts.
    pub fn reset(&mut self) {
        self.last = Instant::now();
    }

    /// Returns the elapsed seconds since the previous call, capped at 34 ms.
    pub fn delta(&mut self) -> f32 {
        let now = Instant::now();
        let delta = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        delta.clamp(0.0, 0.034)
    }
}

/// Best-effort timer interval for a compositor refresh rate.
///
/// `SetTimer` is not a high-resolution timer, so this is a sampling-density
/// hint only: the animation timelines themselves are absolute-time based.
#[must_use]
pub fn frame_interval_ms(refresh_hz: u32) -> u32 {
    let hz = refresh_hz.clamp(30, 360);
    (1000.0 / f64::from(hz)).round().clamp(6.0, 16.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_intervals_follow_common_refresh_rates() {
        assert_eq!(frame_interval_ms(60), 16);
        assert_eq!(frame_interval_ms(120), 8);
        assert_eq!(frame_interval_ms(144), 7);
        assert_eq!(frame_interval_ms(240), 6);
        assert_eq!(frame_interval_ms(0), 16);
    }
}
