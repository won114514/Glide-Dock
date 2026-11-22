//! Launch layer: four launch timelines and the window-confirmation state
//! machine.
//!
//! The launch timeline never writes `running`. It only reports "waiting for a
//! window"; the window tracker is the single source of truth for the running
//! dot.

use crate::layout::Metrics;
use crate::models::LaunchPreset;
use crate::motion::RippleMotion;
use crate::motion::tuning::{SETTLE_S, WAIT_TIMEOUT_S, ease_in_out_cubic, ease_out_cubic, lerp};

/// Visual phase of one icon's launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchPhase {
    /// Nothing is happening.
    Idle,
    /// A launch timeline is running.
    Running,
    /// The timeline finished but no window has been confirmed yet.
    Waiting,
    /// Recovering after a failed/cancelled launch.
    Settling,
}

/// Sampled launch output for one icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LaunchFrame {
    /// Scale multiplier relative to the layout base.
    pub scale: f32,
    /// Vertical offset in device pixels (negative is up).
    pub offset_y: f32,
    /// Extra glow in `0..=1`.
    pub glow: f32,
    /// Optional expanding ring.
    pub ripple: Option<RippleMotion>,
    /// Waiting-dot opacity when the icon waits for a window.
    pub waiting_opacity: Option<f32>,
    /// Whether the launch timeline is currently playing.
    pub launching: bool,
    /// Whether the launch layer still needs frames.
    pub active: bool,
}

impl Default for LaunchFrame {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset_y: 0.0,
            glow: 0.0,
            ripple: None,
            waiting_opacity: None,
            launching: false,
            active: false,
        }
    }
}

/// The launch state of one dock item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LaunchState {
    phase: LaunchPhase,
    started: f32,
    preset: LaunchPreset,
    from_scale: f32,
    from_offset: f32,
    confirmed: bool,
    waiting_started: f32,
}

impl Default for LaunchState {
    fn default() -> Self {
        Self {
            phase: LaunchPhase::Idle,
            started: 0.0,
            preset: LaunchPreset::GlideLaunch,
            from_scale: 1.0,
            from_offset: 0.0,
            confirmed: false,
            waiting_started: 0.0,
        }
    }
}

impl LaunchState {
    /// Current phase.
    #[must_use]
    pub fn phase(&self) -> LaunchPhase {
        self.phase
    }

    /// Preset captured when this launch started.
    #[must_use]
    pub fn preset(&self) -> LaunchPreset {
        self.preset
    }

    /// Starts the visual launch timeline, or jumps straight to the logical
    /// waiting/idle state when animation is disabled.
    pub fn begin(
        &mut self,
        now: f32,
        preset: LaunchPreset,
        animate: bool,
        from_scale: f32,
        from_offset: f32,
        confirmed: bool,
    ) {
        self.confirmed = confirmed;
        self.preset = preset;
        self.from_scale = from_scale.clamp(0.5, 1.5);
        self.from_offset = from_offset;
        if animate {
            self.phase = LaunchPhase::Running;
            self.started = now;
            self.waiting_started = 0.0;
        } else if confirmed {
            self.phase = LaunchPhase::Idle;
        } else {
            self.phase = LaunchPhase::Waiting;
            self.waiting_started = now;
        }
    }

    /// Advances the phase timers. Returns `true` when the phase changed.
    pub fn advance(&mut self, now: f32, running: bool) -> bool {
        if running {
            self.confirmed = true;
        }
        match self.phase {
            LaunchPhase::Running if now - self.started + 1e-4 >= launch_duration(self.preset) => {
                if self.confirmed || running {
                    self.phase = LaunchPhase::Idle;
                } else {
                    self.phase = LaunchPhase::Waiting;
                    self.waiting_started = self.started + launch_duration(self.preset);
                }
                true
            }
            LaunchPhase::Waiting if self.confirmed || running => {
                self.phase = LaunchPhase::Idle;
                true
            }
            LaunchPhase::Waiting if now - self.waiting_started + 1e-4 >= WAIT_TIMEOUT_S => {
                self.phase = LaunchPhase::Idle;
                true
            }
            LaunchPhase::Settling if now - self.started + 1e-4 >= SETTLE_S => {
                self.phase = LaunchPhase::Idle;
                true
            }
            _ => false,
        }
    }

    /// Jumps a running timeline to its logical end when animation is turned
    /// off mid-flight.
    pub fn skip_animation(&mut self, now: f32) {
        if self.phase == LaunchPhase::Running {
            if self.confirmed {
                self.phase = LaunchPhase::Idle;
            } else {
                self.phase = LaunchPhase::Waiting;
                self.waiting_started = now;
            }
        }
    }

    /// Window tracker confirmed a visible window.
    pub fn window_detected(&mut self) {
        self.confirmed = true;
        if self.phase == LaunchPhase::Waiting {
            self.phase = LaunchPhase::Idle;
        }
    }

    /// All known windows for this item disappeared.
    pub fn window_closed(&mut self) {
        let was_confirmed = self.confirmed;
        self.confirmed = false;
        if was_confirmed && matches!(self.phase, LaunchPhase::Running | LaunchPhase::Waiting) {
            self.phase = LaunchPhase::Idle;
        }
    }

    /// Sets tracker confirmation without changing the phase.
    pub fn set_confirmed(&mut self, confirmed: bool) {
        self.confirmed = confirmed;
        if confirmed && self.phase == LaunchPhase::Waiting {
            self.phase = LaunchPhase::Idle;
        }
    }

    /// Launch failed: clear waiting state and ease the icon back.
    pub fn fail(&mut self, now: f32, metrics: &Metrics) {
        self.confirmed = false;
        match self.phase {
            LaunchPhase::Idle => {}
            LaunchPhase::Running => {
                let current = self.frame(now, false, metrics);
                self.from_scale = current.scale;
                self.from_offset = current.offset_y;
                self.phase = LaunchPhase::Settling;
                self.started = now;
            }
            LaunchPhase::Waiting => {
                self.phase = LaunchPhase::Idle;
            }
            LaunchPhase::Settling => {}
        }
    }

    /// Elapsed waiting time, used by the status layer.
    #[must_use]
    pub fn waiting_elapsed(&self, now: f32) -> f32 {
        (now - self.waiting_started).max(0.0)
    }

    /// Whether this icon is currently in the waiting phase.
    #[must_use]
    #[cfg(test)]
    pub fn is_waiting(&self) -> bool {
        self.phase == LaunchPhase::Waiting
    }

    /// `true` while the launch layer needs frames.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase != LaunchPhase::Idle
    }

    /// Neighbour displacement factor in `0..=1`.
    #[must_use]
    pub fn nudge_factor(&self, now: f32) -> f32 {
        if self.phase != LaunchPhase::Running {
            return 0.0;
        }
        let t = now - self.started;
        match self.preset {
            LaunchPreset::GlideLaunch => {
                if t < 0.100 {
                    ease_out_cubic(t / 0.100)
                } else {
                    1.0 - ease_in_out_cubic((t - 0.100) / 0.240)
                }
            }
            LaunchPreset::TripleBounce => {
                if t < 0.100 {
                    ease_out_cubic(t / 0.100)
                } else {
                    1.0 - ease_in_out_cubic((t - 0.100) / 0.200)
                }
            }
            LaunchPreset::RippleLaunch | LaunchPreset::PulseLaunch => 0.0,
        }
        .clamp(0.0, 1.0)
    }

    /// Samples the launch layer.
    #[must_use]
    pub fn frame(&self, now: f32, reduced: bool, metrics: &Metrics) -> LaunchFrame {
        match self.phase {
            LaunchPhase::Idle => LaunchFrame::default(),
            LaunchPhase::Running => {
                let elapsed = (now - self.started).max(0.0);
                let (scale, offset_y, glow, ripple) = sample_launch(
                    self.preset,
                    elapsed,
                    self.from_scale,
                    self.from_offset,
                    metrics,
                );
                LaunchFrame {
                    scale,
                    offset_y,
                    glow,
                    ripple,
                    waiting_opacity: None,
                    launching: true,
                    active: elapsed < launch_duration(self.preset),
                }
            }
            LaunchPhase::Waiting => LaunchFrame {
                waiting_opacity: Some(if reduced {
                    0.75
                } else {
                    crate::motion::status::waiting_opacity(self.waiting_elapsed(now))
                }),
                active: !reduced,
                ..LaunchFrame::default()
            },
            LaunchPhase::Settling => {
                let t = ((now - self.started) / SETTLE_S).clamp(0.0, 1.0);
                let e = ease_out_cubic(t);
                LaunchFrame {
                    scale: lerp(self.from_scale, 1.0, e),
                    offset_y: lerp(self.from_offset, 0.0, e),
                    active: t < 1.0,
                    ..LaunchFrame::default()
                }
            }
        }
    }
}

/// Duration of a launch preset in seconds.
#[must_use]
pub fn launch_duration(preset: LaunchPreset) -> f32 {
    match preset {
        LaunchPreset::GlideLaunch => 0.340,
        LaunchPreset::TripleBounce => 0.520,
        LaunchPreset::RippleLaunch => 0.420,
        LaunchPreset::PulseLaunch => 0.400,
    }
}

/// Neighbour displacement in base-size units for a launch preset.
#[must_use]
pub fn nudge_distance(preset: LaunchPreset, slot: usize) -> f32 {
    match (preset, slot) {
        (LaunchPreset::GlideLaunch, 1) => 0.025,
        (LaunchPreset::GlideLaunch, 2) => 0.010,
        (LaunchPreset::TripleBounce, 1) => 0.018,
        (LaunchPreset::TripleBounce, 2) => 0.007,
        _ => 0.0,
    }
}

fn sample_launch(
    preset: LaunchPreset,
    elapsed: f32,
    from_scale: f32,
    from_offset: f32,
    metrics: &Metrics,
) -> (f32, f32, f32, Option<RippleMotion>) {
    match preset {
        LaunchPreset::GlideLaunch => {
            let up = -8.0 * metrics.scale;
            let down = -2.0 * metrics.scale;
            if elapsed < 0.100 {
                let e = ease_out_cubic(elapsed / 0.100);
                (
                    lerp(from_scale, 1.16, e),
                    lerp(from_offset, up, e),
                    0.0,
                    None,
                )
            } else if elapsed < 0.210 {
                let e = ease_in_out_cubic((elapsed - 0.100) / 0.110);
                (lerp(1.16, 1.04, e), lerp(up, down, e), 0.0, None)
            } else if elapsed < 0.340 {
                let e = ease_out_cubic((elapsed - 0.210) / 0.130);
                (lerp(1.04, 1.0, e), lerp(down, 0.0, e), 0.0, None)
            } else {
                (1.0, 0.0, 0.0, None)
            }
        }
        LaunchPreset::TripleBounce => {
            if elapsed < 0.100 {
                let e = ease_out_cubic(elapsed / 0.100);
                (lerp(from_scale, 1.15, e), 0.0, 0.0, None)
            } else if elapsed < 0.230 {
                let e = ease_in_out_cubic((elapsed - 0.100) / 0.130);
                (lerp(1.15, 0.97, e), 0.0, 0.0, None)
            } else if elapsed < 0.350 {
                let e = ease_in_out_cubic((elapsed - 0.230) / 0.120);
                (lerp(0.97, 1.04, e), 0.0, 0.0, None)
            } else if elapsed < 0.520 {
                let e = ease_out_cubic((elapsed - 0.350) / 0.170);
                (lerp(1.04, 1.0, e), 0.0, 0.0, None)
            } else {
                (1.0, 0.0, 0.0, None)
            }
        }
        LaunchPreset::RippleLaunch => {
            let t = (elapsed / 0.420).clamp(0.0, 1.0);
            let scale = if t < 0.286 {
                lerp(from_scale, 1.06, ease_out_cubic(t / 0.286))
            } else {
                lerp(1.06, 1.0, ease_in_out_cubic((t - 0.286) / 0.714))
            };
            let ripple = if t < 1.0 {
                Some(RippleMotion {
                    radius: metrics.icon * lerp(0.5, 1.8, ease_out_cubic(t)),
                    width: lerp(2.5, 0.5, t) * metrics.scale,
                    opacity: lerp(0.55, 0.0, ease_out_cubic(t)),
                })
            } else {
                None
            };
            (scale, 0.0, (1.0 - t) * 0.20, ripple)
        }
        LaunchPreset::PulseLaunch => {
            let t = (elapsed / 0.400).clamp(0.0, 1.0);
            let (scale, glow) = if t < 0.300 {
                (
                    lerp(from_scale, 1.10, ease_out_cubic(t / 0.300)),
                    lerp(0.0, 0.90, ease_out_cubic(t / 0.300)),
                )
            } else if t < 0.600 {
                (
                    lerp(1.10, 1.03, ease_in_out_cubic((t - 0.300) / 0.300)),
                    lerp(0.90, 0.45, ease_in_out_cubic((t - 0.300) / 0.300)),
                )
            } else {
                (
                    lerp(1.03, 1.0, ease_out_cubic((t - 0.600) / 0.400)),
                    lerp(0.45, 0.0, ease_out_cubic((t - 0.600) / 0.400)),
                )
            };
            (scale, 0.0, glow, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Appearance, Behavior, MotionConfig};

    fn metrics() -> Metrics {
        Metrics::new(
            &Appearance::default(),
            &Behavior::default(),
            &MotionConfig::default(),
            1.0,
        )
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.003
    }

    #[test]
    fn glide_launch_hits_keyframes() {
        let metrics = metrics();
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::GlideLaunch, true, 1.0, 0.0, false);
        for (time, scale, offset) in [(0.100, 1.16, -8.0), (0.210, 1.04, -2.0), (0.340, 1.0, 0.0)] {
            let frame = launch.frame(time, false, &metrics);
            assert!(close(frame.scale, scale), "scale at {time}");
            assert!(close(frame.offset_y, offset), "offset at {time}");
        }
        assert!(launch.advance(0.340, false));
        assert_eq!(launch.phase(), LaunchPhase::Waiting);
    }

    #[test]
    fn triple_bounce_hits_keyframes() {
        let metrics = metrics();
        let presets = [(0.100, 1.15), (0.230, 0.97), (0.350, 1.04), (0.520, 1.0)];
        for (time, scale) in presets {
            let mut launch = LaunchState::default();
            launch.begin(0.0, LaunchPreset::TripleBounce, true, 1.0, 0.0, true);
            assert!(close(launch.frame(time, false, &metrics).scale, scale));
        }
    }

    #[test]
    fn ripple_launch_expands_ring() {
        let metrics = metrics();
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::RippleLaunch, true, 1.0, 0.0, true);
        let early = launch.frame(0.020, false, &metrics).ripple.expect("ripple");
        let late = launch.frame(0.380, false, &metrics).ripple.expect("ripple");
        assert!(late.radius > early.radius);
        assert!(late.opacity < early.opacity);
        assert!(launch.frame(0.420, false, &metrics).ripple.is_none());
    }

    #[test]
    fn pulse_launch_scales_and_glows() {
        let metrics = metrics();
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::PulseLaunch, true, 1.0, 0.0, true);
        let peak = launch.frame(0.120, false, &metrics);
        assert!(peak.scale > 1.09);
        assert!(peak.glow > 0.5);
        assert!(launch.frame(0.400, false, &metrics).scale <= 1.001);
    }

    #[test]
    fn launch_end_waits_until_window_confirmation() {
        let metrics = metrics();
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::GlideLaunch, true, 1.0, 0.0, false);
        assert!(launch.advance(0.340, false));
        assert_eq!(launch.phase(), LaunchPhase::Waiting);
        assert!(
            launch
                .frame(0.500, false, &metrics)
                .waiting_opacity
                .is_some()
        );
        launch.window_detected();
        assert_eq!(launch.phase(), LaunchPhase::Idle);
        assert!(!launch.is_waiting());
    }

    #[test]
    fn failure_continues_from_the_current_pose_then_settles() {
        let metrics = metrics();
        let now = 0.08;
        let mut state = LaunchState::default();
        state.begin(0.0, LaunchPreset::GlideLaunch, true, 0.9, 0.0, false);
        let before = state.frame(now, false, &metrics);
        state.fail(now, &metrics);
        let after = state.frame(now, false, &metrics);
        assert_eq!(before.scale, after.scale);
        assert_eq!(before.offset_y, after.offset_y);
        assert!(state.frame(now + 0.13, false, &metrics).active);
        state.advance(now + 0.14, false);
        assert_eq!(
            state.frame(now + 0.14, false, &metrics),
            LaunchFrame::default()
        );
    }

    #[test]
    fn success_does_not_imply_running() {
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::GlideLaunch, true, 1.0, 0.0, false);
        launch.advance(0.340, false);
        assert_eq!(launch.phase(), LaunchPhase::Waiting);
        // The app only calls window_detected after the tracker sees a window.
        launch.advance(0.500, false);
        assert_eq!(launch.phase(), LaunchPhase::Waiting);
    }

    #[test]
    fn waiting_times_out() {
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::GlideLaunch, true, 1.0, 0.0, false);
        launch.advance(0.340, false);
        assert_eq!(launch.phase(), LaunchPhase::Waiting);
        assert!(launch.advance(0.340 + WAIT_TIMEOUT_S + 0.1, false));
        assert_eq!(launch.phase(), LaunchPhase::Idle);
    }

    #[test]
    fn disabled_animation_goes_to_waiting_immediately() {
        let mut launch = LaunchState::default();
        launch.begin(0.0, LaunchPreset::GlideLaunch, false, 1.0, 0.0, false);
        assert_eq!(launch.phase(), LaunchPhase::Waiting);
    }

    #[test]
    fn neighbours_are_pushed_for_bounce_launches_only() {
        let mut glide = LaunchState::default();
        glide.begin(0.0, LaunchPreset::GlideLaunch, true, 1.0, 0.0, true);
        assert!(glide.nudge_factor(0.100) > 0.9);
        assert!(glide.nudge_factor(0.340) < 0.01);
        let mut ripple = LaunchState::default();
        ripple.begin(0.0, LaunchPreset::RippleLaunch, true, 1.0, 0.0, true);
        assert_eq!(ripple.nudge_factor(0.100), 0.0);
        assert!(nudge_distance(LaunchPreset::GlideLaunch, 1) > 0.0);
    }
}
