//! Composition layer: merge independent channels without letting one overwrite
//! another.
//!
//! Scale channels multiply, translation channels add, light channels take the
//! strongest contribution and the status layer is copied through untouched.

use crate::motion::launch::LaunchFrame;
use crate::motion::press::PressFrame;
use crate::motion::status::StatusFrame;
use crate::motion::tuning::{MAX_SCALE, MIN_SCALE};
use crate::motion::{IconMotion, RippleMotion};

/// Inputs for composing one icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ComposeInput {
    /// Hover vertical offset in device pixels.
    pub hover_lift: f32,
    /// Hover tilt in degrees.
    pub tilt_x: f32,
    /// Hover tilt in degrees.
    pub tilt_y: f32,
    /// Hover glow in `0..=1`.
    pub hover_glow: f32,
    /// Neighbour displacement in device pixels.
    pub neighbour_nudge: f32,
    /// Press channel.
    pub press: PressFrame,
    /// Launch channel.
    pub launch: LaunchFrame,
    /// Status channel.
    pub status: StatusFrame,
    /// Offset clamp in device pixels.
    pub max_offset_px: f32,
}

/// Merges the channels into the renderer's per-icon motion.
#[must_use]
pub(crate) fn compose(input: ComposeInput) -> IconMotion {
    // Press and launch are mutually exclusive visual owners. Multiplying both
    // turns a 0.5 launch start and a 0.5 press floor into a 0.25 icon, which
    // looks like a collapsing thumbnail for one frame. The launch layer wins
    // while it is playing.
    let scale = if input.launch.launching {
        input.launch.scale
    } else {
        input.press.scale * input.launch.scale
    }
    .clamp(MIN_SCALE, MAX_SCALE);
    let offset_y = (input.hover_lift + input.press.offset_y + input.launch.offset_y)
        .clamp(-input.max_offset_px, input.max_offset_px);
    let offset_x = input
        .neighbour_nudge
        .clamp(-input.max_offset_px, input.max_offset_px);
    let glow = input
        .hover_glow
        .max(input.press.glow)
        .max(input.launch.glow)
        .clamp(0.0, 1.0);
    let ripple: Option<RippleMotion> = input.launch.ripple.or(input.press.ripple);
    IconMotion {
        scale,
        offset_x,
        offset_y,
        tilt_x_deg: input.tilt_x,
        tilt_y_deg: input.tilt_y,
        glow,
        ripple,
        waiting_opacity: input.status.waiting_opacity,
        running_dot_allowed: input.status.running_dot_allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::press::PressState;

    fn neutral_press() -> PressFrame {
        PressState::default().frame(
            0.0,
            &crate::layout::Metrics::new(
                &crate::models::Appearance::default(),
                &crate::models::Behavior::default(),
                &crate::models::MotionConfig::default(),
                1.0,
            ),
        )
    }

    #[test]
    fn scales_multiply_and_offsets_add() {
        let motion = compose(ComposeInput {
            hover_lift: -3.0,
            tilt_x: 1.0,
            tilt_y: -2.0,
            hover_glow: 0.2,
            neighbour_nudge: 4.0,
            press: PressFrame {
                scale: 0.94,
                offset_y: 0.0,
                ..neutral_press()
            },
            launch: LaunchFrame {
                scale: 1.16,
                offset_y: -8.0,
                glow: 0.5,
                ..LaunchFrame::default()
            },
            status: StatusFrame::default(),
            max_offset_px: 16.0,
        });
        assert!((motion.scale - 0.94 * 1.16).abs() < 0.001);
        assert!((motion.offset_y - -11.0).abs() < 0.001);
        assert!((motion.offset_x - 4.0).abs() < 0.001);
        assert!((motion.glow - 0.5).abs() < 0.001);
    }

    #[test]
    fn launch_does_not_multiply_press_scale() {
        let motion = compose(ComposeInput {
            hover_lift: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            hover_glow: 0.0,
            neighbour_nudge: 0.0,
            press: PressFrame {
                scale: 0.5,
                ..neutral_press()
            },
            launch: LaunchFrame {
                scale: 0.5,
                launching: true,
                active: true,
                ..LaunchFrame::default()
            },
            status: StatusFrame::default(),
            max_offset_px: 16.0,
        });
        assert!((motion.scale - 0.5).abs() < 0.001);
    }

    #[test]
    fn offsets_are_clamped() {
        let motion = compose(ComposeInput {
            hover_lift: -20.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            hover_glow: 0.0,
            neighbour_nudge: 100.0,
            press: neutral_press(),
            launch: LaunchFrame {
                offset_y: -20.0,
                ..LaunchFrame::default()
            },
            status: StatusFrame::default(),
            max_offset_px: 16.0,
        });
        assert!((motion.offset_y - -16.0).abs() < 0.001);
        assert!((motion.offset_x - 16.0).abs() < 0.001);
    }

    #[test]
    fn launch_ripple_wins_over_press_ripple() {
        let ripple_a = RippleMotion {
            radius: 10.0,
            width: 1.0,
            opacity: 0.3,
        };
        let ripple_b = RippleMotion {
            radius: 20.0,
            width: 2.0,
            opacity: 0.6,
        };
        let motion = compose(ComposeInput {
            hover_lift: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            hover_glow: 0.0,
            neighbour_nudge: 0.0,
            press: PressFrame {
                ripple: Some(ripple_a),
                ..neutral_press()
            },
            launch: LaunchFrame {
                ripple: Some(ripple_b),
                ..LaunchFrame::default()
            },
            status: StatusFrame::default(),
            max_offset_px: 16.0,
        });
        assert_eq!(motion.ripple, Some(ripple_b));
    }
}
