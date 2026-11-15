//! Real-time exponential smoothing.

/// One exponential smoothing step.
///
/// `dt` is in seconds and must come from the frame clock. `tau` is the time
/// constant in seconds. The result never overshoots `target`.
#[must_use]
pub fn exponential_step(current: f32, target: f32, dt: f32, tau: f32) -> f32 {
    if !current.is_finite() || !target.is_finite() {
        return target;
    }
    if !dt.is_finite() || dt <= 0.0 || !tau.is_finite() || tau <= 0.0 {
        return current;
    }
    let alpha = 1.0 - (-dt / tau).exp();
    current + alpha.clamp(0.0, 1.0) * (target - current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approaches_without_overshoot() {
        let mut value = 1.0;
        for _ in 0..120 {
            value = exponential_step(value, 1.35, 1.0 / 60.0, 0.035);
            assert!(value <= 1.35);
        }
        assert!(value > 1.349);
    }

    #[test]
    fn is_time_based_not_frame_based() {
        let sixty = (0..10).fold(1.0, |value, _| {
            exponential_step(value, 1.35, 1.0 / 60.0, 0.035)
        });
        let high = (0..24).fold(1.0, |value, _| {
            exponential_step(value, 1.35, 1.0 / 144.0, 0.035)
        });
        assert!((sixty - high).abs() < 1e-3, "{sixty} vs {high}");
    }
}
