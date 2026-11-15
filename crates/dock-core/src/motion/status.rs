//! Status layer: the waiting breathing dot and the running indicator.

use crate::motion::launch::{LaunchPhase, LaunchState};
use crate::motion::tuning::WAIT_PERIOD_S;

/// Opacity of the waiting dot for an elapsed waiting time.
///
/// The dot breathes between 0.55 and 1.00 once per second.
#[must_use]
pub fn waiting_opacity(elapsed: f32) -> f32 {
    let phase = (elapsed.max(0.0) % WAIT_PERIOD_S) / WAIT_PERIOD_S;
    let wave = 0.5 - 0.5 * (std::f32::consts::TAU * phase).cos();
    0.55 + 0.45 * wave
}

/// Visual status output for one icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StatusFrame {
    /// Waiting dot opacity, when the icon is waiting for a window.
    pub waiting_opacity: Option<f32>,
    /// Whether the fixed running dot may be drawn.
    pub running_dot_allowed: bool,
}

impl Default for StatusFrame {
    fn default() -> Self {
        Self {
            waiting_opacity: None,
            running_dot_allowed: true,
        }
    }
}

/// Samples the status layer for one icon.
///
/// `press_active` suppresses the running dot while the pointer is down.
#[must_use]
pub(crate) fn status_frame(
    launch: &LaunchState,
    press_active: bool,
    reduced: bool,
    now: f32,
) -> StatusFrame {
    match launch.phase() {
        LaunchPhase::Idle if !press_active => StatusFrame {
            waiting_opacity: None,
            running_dot_allowed: true,
        },
        LaunchPhase::Waiting => StatusFrame {
            waiting_opacity: Some(if reduced {
                0.75
            } else {
                waiting_opacity(launch.waiting_elapsed(now))
            }),
            running_dot_allowed: false,
        },
        _ => StatusFrame {
            waiting_opacity: None,
            running_dot_allowed: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_dot_breathes() {
        assert!((waiting_opacity(0.0) - 0.55).abs() < 0.001);
        assert!((waiting_opacity(0.5) - 1.0).abs() < 0.001);
        assert!((waiting_opacity(1.0) - 0.55).abs() < 0.001);
    }
}
