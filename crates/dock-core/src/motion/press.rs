//! Interaction layer: Fluid Press plus the Bounce/Glow click accents.

use crate::layout::Metrics;
use crate::motion::RippleMotion;
use crate::motion::tuning::{
    PRESS_DOWN_S, PRESS_RELEASE_S, PRESSED_SCALE, RELEASE_PEAK, SETTLE_S, ease_in_out_cubic,
    ease_out_cubic, lerp,
};

/// Optional extra feedback attached to a hover preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PressAccent {
    /// Two decreasing vertical bounces (Bounce hover preset).
    BounceDouble,
    /// Blue-violet ring expanding from the icon (Glow hover preset).
    GlowWave,
}

impl PressAccent {
    fn duration(self) -> f32 {
        match self {
            Self::BounceDouble => 0.520,
            Self::GlowWave => 0.420,
        }
    }
}

/// Where the press scale currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Down,
    Release,
    Settling,
}

/// Sampled press output for one icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PressFrame {
    /// Scale multiplier relative to the layout base.
    pub scale: f32,
    /// Vertical offset in device pixels (negative is up).
    pub offset_y: f32,
    /// Accent glow value in `0..=1`.
    pub glow: f32,
    /// Optional expanding ring.
    pub ripple: Option<RippleMotion>,
    /// Whether the press channel still needs frames.
    pub active: bool,
}

impl Default for PressFrame {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset_y: 0.0,
            glow: 0.0,
            ripple: None,
            active: false,
        }
    }
}

/// State of one icon's press interaction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PressState {
    phase: Phase,
    started: f32,
    from_scale: f32,
    accent: Option<PressAccent>,
    accent_started: f32,
}

impl Default for PressState {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            started: 0.0,
            from_scale: 1.0,
            accent: None,
            accent_started: 0.0,
        }
    }
}

impl PressState {
    /// Starts a pointer-down press from the currently sampled scale.
    pub fn pointer_down(&mut self, now: f32, current_scale: f32) {
        self.phase = Phase::Down;
        self.started = now;
        self.from_scale = current_scale.clamp(0.5, 1.5);
        self.accent = None;
    }

    /// Starts the release rebound. The accent is supplied by the hover preset.
    pub fn pointer_up(&mut self, now: f32, accent: Option<PressAccent>) {
        if self.phase == Phase::Down {
            let current = self.sample_scale(now);
            self.from_scale = current;
            self.phase = Phase::Release;
            self.started = now;
        }
        if let Some(accent) = accent {
            self.accent = Some(accent);
            self.accent_started = now;
        }
    }

    /// Cancels a held press and eases back without a rebound. Completed
    /// clicks keep their release motion and accent when the pointer leaves.
    pub fn cancel(&mut self, now: f32) {
        if self.phase != Phase::Down {
            return;
        }
        let current = self.sample_scale(now);
        self.from_scale = current;
        self.phase = Phase::Settling;
        self.started = now;
        self.accent = None;
    }

    /// Stops the press scale because the launch layer now owns the icon.
    ///
    /// The accent, if any, keeps running: it is an independent visual channel.
    pub fn hand_over_to_launch(&mut self) {
        self.phase = Phase::Idle;
        self.from_scale = 1.0;
    }

    /// Clears the accent and resets to rest.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `true` while the press or an accent still needs frames.
    #[must_use]
    pub fn is_active(&self, now: f32) -> bool {
        let scale_active = match self.phase {
            Phase::Idle => false,
            Phase::Down => now - self.started < PRESS_DOWN_S,
            Phase::Release => now - self.started < PRESS_RELEASE_S,
            Phase::Settling => now - self.started < SETTLE_S,
        };
        scale_active
            || self
                .accent
                .is_some_and(|accent| now - self.accent_started < accent.duration())
    }

    /// The held pose still suppresses the status dot after it stops animating.
    pub fn holds_status(&self, now: f32) -> bool {
        self.phase == Phase::Down || self.is_active(now)
    }

    /// Current press scale without the accent.
    #[must_use]
    pub fn sample_scale(&self, now: f32) -> f32 {
        match self.phase {
            Phase::Idle => 1.0,
            Phase::Down => {
                let t = ((now - self.started) / PRESS_DOWN_S).clamp(0.0, 1.0);
                lerp(self.from_scale, PRESSED_SCALE, ease_out_cubic(t))
            }
            Phase::Release => {
                let t = ((now - self.started) / PRESS_RELEASE_S).clamp(0.0, 1.0);
                if t < 0.42 {
                    lerp(self.from_scale, RELEASE_PEAK, ease_out_cubic(t / 0.42))
                } else {
                    lerp(RELEASE_PEAK, 1.0, ease_in_out_cubic((t - 0.42) / 0.58))
                }
            }
            Phase::Settling => {
                let t = ((now - self.started) / SETTLE_S).clamp(0.0, 1.0);
                lerp(self.from_scale, 1.0, ease_out_cubic(t))
            }
        }
    }

    /// Samples the full press channel for one frame.
    #[must_use]
    pub fn frame(&self, now: f32, metrics: &Metrics) -> PressFrame {
        let mut frame = PressFrame {
            scale: self.sample_scale(now),
            active: self.is_active(now),
            ..PressFrame::default()
        };
        let Some(accent) = self.accent else {
            return frame;
        };
        let elapsed = (now - self.accent_started).max(0.0);
        if elapsed >= accent.duration() {
            return frame;
        }
        match accent {
            PressAccent::BounceDouble => {
                frame.offset_y -= double_bounce_px(elapsed) * metrics.scale;
            }
            PressAccent::GlowWave => {
                let t = elapsed / accent.duration();
                frame.ripple = Some(RippleMotion {
                    radius: metrics.icon * lerp(0.5, 1.7, t),
                    width: lerp(2.0, 0.5, t) * metrics.scale,
                    opacity: lerp(0.45, 0.0, ease_out_cubic(t)),
                });
                frame.glow = (1.0 - t) * 0.35;
            }
        }
        frame
    }
}

/// Two decreasing bounces: 0 / -6 / 0 / -2 / 0 DIP at 0/100/230/350/520 ms.
fn double_bounce_px(elapsed: f32) -> f32 {
    if elapsed < 0.100 {
        lerp(0.0, 6.0, ease_out_cubic(elapsed / 0.100))
    } else if elapsed < 0.230 {
        lerp(6.0, 0.0, ease_in_out_cubic((elapsed - 0.100) / 0.130))
    } else if elapsed < 0.350 {
        lerp(0.0, 2.0, ease_in_out_cubic((elapsed - 0.230) / 0.120))
    } else {
        lerp(2.0, 0.0, ease_out_cubic((elapsed - 0.350) / 0.170))
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
    fn fluid_press_hits_down_and_release_keyframes() {
        let metrics = metrics();
        let mut press = PressState::default();
        press.pointer_down(1.0, 1.0);
        assert!(close(press.frame(1.0, &metrics).scale, 1.0));
        assert!(close(press.frame(1.075, &metrics).scale, PRESSED_SCALE));
        assert!(!press.is_active(1.080));
        assert!(press.holds_status(1.080));
        press.pointer_up(1.075, None);
        let peak = press.frame(1.075 + 0.090, &metrics).scale;
        assert!((peak - RELEASE_PEAK).abs() < 0.01);
        assert!(close(press.frame(1.075 + 0.220, &metrics).scale, 1.0));
        assert!(!press.is_active(1.075 + 0.220));
    }

    #[test]
    fn cancel_returns_to_rest_without_rebound() {
        let metrics = metrics();
        let mut press = PressState::default();
        press.pointer_down(0.0, 1.0);
        press.cancel(0.040);
        let before = press.frame(0.040, &metrics).scale;
        let at_cancel = press.frame(0.040, &metrics).scale;
        assert!(close(before, at_cancel));
        assert!(close(press.frame(0.040 + SETTLE_S, &metrics).scale, 1.0));
    }

    #[test]
    fn bounce_accent_has_two_decreasing_peaks() {
        let metrics = metrics();
        let mut press = PressState::default();
        press.pointer_down(0.0, 1.0);
        press.pointer_up(0.070, Some(PressAccent::BounceDouble));
        let first = press.frame(0.070 + 0.100, &metrics).offset_y;
        let second = press.frame(0.070 + 0.350, &metrics).offset_y;
        assert!(first < -5.0);
        assert!(second < -1.0);
        assert!(first < second);
        assert!(press.is_active(0.070 + 0.510));
        assert!(!press.is_active(0.070 + 0.521));
        assert!(close(press.frame(0.070 + 0.520, &metrics).offset_y, 0.0));
    }

    #[test]
    fn glow_accent_expands_and_fades() {
        let metrics = metrics();
        let mut press = PressState::default();
        press.pointer_down(0.0, 1.0);
        press.pointer_up(0.070, Some(PressAccent::GlowWave));
        let early = press.frame(0.070 + 0.020, &metrics).ripple.expect("ripple");
        let late = press.frame(0.070 + 0.380, &metrics).ripple.expect("ripple");
        assert!(late.radius > early.radius);
        assert!(late.opacity < early.opacity);
        assert!(press.is_active(0.070 + 0.410));
        assert!(!press.is_active(0.070 + 0.421));
        assert!(press.frame(0.070 + 0.420, &metrics).ripple.is_none());
    }
}
