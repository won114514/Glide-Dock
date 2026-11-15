//! Derived motion parameters.
//!
//! The persisted configuration exposes only a handful of user-facing knobs.
//! Everything the animation engine needs (spring constants, preset durations,
//! DIP-to-pixel conversions) is derived here so no timing constant is scattered
//! across the app or the renderer.

use crate::models::{Behavior, HoverPreset, LaunchPreset, MAX_HOVER_SCALE, MotionConfig};

/// Baseline hover duration used by the settings sliders, in seconds.
pub const BASE_HOVER_S: f32 = 0.180;
/// Press animation: pointer down.
pub const PRESS_DOWN_S: f32 = 0.075;
/// Press animation: release rebound.
pub const PRESS_RELEASE_S: f32 = 0.220;
/// Scale multiplier while the pointer is down.
pub const PRESSED_SCALE: f32 = 0.94;
/// Peak of the release rebound.
pub const RELEASE_PEAK: f32 = 1.02;
/// Recovery after a cancelled press or failed launch, in seconds.
pub const SETTLE_S: f32 = 0.140;
/// Waiting-dot breathing period, in seconds.
pub const WAIT_PERIOD_S: f32 = 1.000;
/// How long an unconfirmed launch may keep a frame timer alive.
pub const WAIT_TIMEOUT_S: f32 = 12.000;
/// Longest accepted frame delta, in seconds.
pub const MAX_FRAME_DT: f32 = 0.034;
/// Hard clamp for composed vertical offsets, in DIP.
pub const MAX_OFFSET_DIP: f32 = 16.0;
/// Hard clamp for the composed render scale multiplier.
pub const MIN_SCALE: f32 = 0.20;
/// Hard clamp for the composed render scale multiplier.
pub const MAX_SCALE: f32 = 3.0;

/// Tuning shared by every channel of the motion engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionTuning {
    /// Master switch after combining configuration and reduced motion.
    pub enabled: bool,
    /// Hover layer is allowed to move.
    pub hover_enabled: bool,
    /// Launch layer is allowed to animate.
    pub launch_enabled: bool,
    /// Reduced-motion mode: status dots stay static and no timeline runs.
    pub reduced: bool,
    /// Currently selected hover preset.
    pub hover_preset: HoverPreset,
    /// Currently selected launch preset.
    pub launch_preset: LaunchPreset,
    /// Peak hover scale (1.0 when disabled).
    pub max_scale: f32,
    /// Attenuation radius in icon slots.
    pub range_slots: f32,
    /// Baseline hover duration in seconds.
    pub hover_s: f32,
    /// Hover exit duration in seconds.
    pub exit_s: f32,
    /// Spring stiffness.
    pub stiffness: f32,
    /// Spring damping ratio.
    pub damping: f32,
    /// Multiplier applied to hover preset timelines (1.0 at 180 ms).
    pub time_scale: f32,
    /// Stiffness used by the Bounce hover spring (~300 ms).
    pub bounce_stiffness: f32,
    /// Stiffness used by the Perspective restore spring (~200 ms).
    pub perspective_stiffness: f32,
}

impl MotionTuning {
    /// Builds tuning from persisted configuration plus the Windows animation
    /// accessibility switch.
    #[must_use]
    pub fn new(behavior: &Behavior, motion: &MotionConfig, system_animations: bool) -> Self {
        let reduced_by_system = motion.respect_reduced_motion && !system_animations;
        let enabled = motion.enabled && !reduced_by_system;
        let hover_s = (motion.hover_ms.max(1) as f32 / 1000.0).clamp(0.080, 0.400);
        let elasticity = motion.elasticity.clamp(0.0, 1.0);
        let damping = (1.0 - 0.4 * elasticity).clamp(0.35, 1.0);
        let stiffness = spring_stiffness(hover_s, damping);
        let bounce_stiffness = spring_stiffness(0.300, damping);
        let perspective_stiffness = spring_stiffness(0.200, damping);
        let max_scale = if enabled && behavior.magnification {
            motion.max_scale.clamp(1.0, MAX_HOVER_SCALE)
        } else {
            1.0
        };
        Self {
            enabled,
            hover_enabled: enabled && behavior.magnification,
            launch_enabled: enabled && behavior.launch_animation,
            reduced: !enabled,
            hover_preset: motion.hover_preset,
            launch_preset: motion.launch_preset,
            max_scale,
            range_slots: motion.range_slots.clamp(0.5, 8.0),
            hover_s,
            exit_s: (hover_s * 0.78).clamp(0.070, 0.320),
            stiffness,
            damping,
            time_scale: (hover_s / BASE_HOVER_S).clamp(0.5, 2.5),
            bounce_stiffness,
            perspective_stiffness,
        }
    }

    /// A tuning where every visual channel is neutral.
    #[must_use]
    pub fn disabled() -> Self {
        let mut tuning = Self::new(&Behavior::default(), &MotionConfig::default(), true);
        tuning.enabled = false;
        tuning.hover_enabled = false;
        tuning.launch_enabled = false;
        tuning.reduced = true;
        tuning.max_scale = 1.0;
        tuning
    }

    /// Duration of the current hover timeline scaled by the user preference.
    #[must_use]
    pub fn scaled(&self, seconds: f32) -> f32 {
        (seconds * self.time_scale).max(0.0)
    }
}

/// Stiffness that settles a spring to 2% within `duration` seconds.
#[must_use]
pub fn spring_stiffness(duration: f32, damping: f32) -> f32 {
    let duration = duration.clamp(0.05, 2.0);
    let damping = damping.clamp(0.2, 1.5);
    let omega = 3.912 / (damping * duration);
    (omega * omega).clamp(80.0, 6000.0)
}

/// Scales a duration relative to the 180 ms baseline.
#[must_use]
pub fn scale_for_time_scale(time_scale: f32, seconds: f32) -> f32 {
    (seconds * time_scale.clamp(0.5, 2.5)).max(0.0)
}

/// Cubic ease-out.
#[must_use]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Cubic ease-in-out.
#[must_use]
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Linear interpolation with clamped `t`.
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_produce_the_spec_timing() {
        let tuning = MotionTuning::new(&Behavior::default(), &MotionConfig::default(), true);
        assert!(tuning.enabled);
        assert!((tuning.max_scale - 1.35).abs() < f32::EPSILON);
        assert!((tuning.hover_s - 0.180).abs() < f32::EPSILON);
        assert!((tuning.exit_s - 0.140).abs() < 0.001);
        assert!((tuning.time_scale - 1.0).abs() < f32::EPSILON);
        assert!(tuning.stiffness > 500.0);
        assert!(tuning.damping < 1.0);
    }

    #[test]
    fn reduced_motion_and_disabled_are_neutral() {
        let behavior = Behavior::default();
        let motion = MotionConfig::default();
        let tuning = MotionTuning::new(&behavior, &motion, false);
        assert!(!tuning.enabled);
        assert!(tuning.reduced);
        assert!((tuning.max_scale - 1.0).abs() < f32::EPSILON);

        let mut disabled = motion.clone();
        disabled.enabled = false;
        let tuning = MotionTuning::new(&behavior, &disabled, true);
        assert!(!tuning.enabled);
    }

    #[test]
    fn durations_are_clamped_and_scaled() {
        let mut motion = MotionConfig {
            hover_ms: 400,
            elasticity: 0.0,
            ..MotionConfig::default()
        };
        motion.normalize();
        let tuning = MotionTuning::new(&Behavior::default(), &motion, true);
        assert!((tuning.hover_s - 0.4).abs() < f32::EPSILON);
        assert!((tuning.exit_s - 0.312).abs() < 0.001);
        assert!((tuning.damping - 1.0).abs() < f32::EPSILON);
        assert!((tuning.time_scale - 2.222).abs() < 0.01);
    }

    #[test]
    fn spring_stiffness_grows_when_duration_shrinks() {
        assert!(spring_stiffness(0.12, 0.7) > spring_stiffness(0.4, 0.7));
        assert!(spring_stiffness(0.18, 0.5) > spring_stiffness(0.18, 1.0));
    }
}
