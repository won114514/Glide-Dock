//! Hover layer maths: Glide Zoom targets, Wave propagation, Bounce lift,
//! Glow breathing and Perspective tilt.
//!
//! The engine owns the springs; this module only computes targets and pure
//! envelopes so every preset can be tested without a window.

use crate::geometry::{Point, clamp01, lerp, smoothstep};
use crate::layout::{DockGeometry, Metrics};
use crate::motion::tuning::{ease_in_out_cubic, ease_out_cubic};

/// Maximum Perspective tilt in degrees.
pub const MAX_TILT_DEG: f32 = 5.0;
/// Bounce hover lift as a multiple of `(max_scale - 1)` DIP.
pub const BOUNCE_LIFT_DIP: f32 = 10.0;

/// A time-limited two-way envelope used by the hover backdrop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TimedEnvelope {
    started: f32,
    from: f32,
    target: f32,
    duration: f32,
    active: bool,
}

impl Default for TimedEnvelope {
    fn default() -> Self {
        Self {
            started: 0.0,
            from: 0.0,
            target: 0.0,
            duration: 0.14,
            active: false,
        }
    }
}

impl TimedEnvelope {
    /// Starts a transition from the current value to `target`.
    pub fn retarget(&mut self, now: f32, target: f32, duration: f32) {
        let target = target.clamp(0.0, 1.0);
        if self.active && (self.target - target).abs() < f32::EPSILON {
            return;
        }
        self.from = self.value(now);
        self.target = target;
        self.duration = duration.max(0.001);
        self.started = now;
        self.active = true;
    }

    /// Current envelope value.
    #[must_use]
    pub fn value(&self, now: f32) -> f32 {
        if !self.active {
            return self.target;
        }
        let t = ((now - self.started) / self.duration).clamp(0.0, 1.0);
        lerp(self.from, self.target, ease_out_cubic(t))
    }

    /// Whether frames are still required.
    #[must_use]
    pub fn is_active(&self, now: f32) -> bool {
        self.active && now - self.started < self.duration - 1e-4
    }

    /// Snaps to the target.
    pub fn settle(&mut self) {
        self.from = self.target;
        self.active = false;
    }
}

/// State of the travelling Wave preset.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WaveState {
    started: f32,
    anchor: Option<usize>,
    leaving_at: Option<f32>,
    weights: Vec<f32>,
}

impl Default for WaveState {
    fn default() -> Self {
        Self {
            started: 0.0,
            anchor: None,
            leaving_at: None,
            weights: Vec::new(),
        }
    }
}

impl WaveState {
    /// Starts a new wave at `anchor`.
    pub fn trigger(&mut self, now: f32, anchor: usize, weights: Vec<f32>) {
        self.started = now;
        self.anchor = Some(anchor);
        self.leaving_at = None;
        self.weights = weights;
    }

    /// Starts the decay of the current wave.
    pub fn leave(&mut self, now: f32) {
        if self.anchor.is_some() && self.leaving_at.is_none() {
            self.leaving_at = Some(now);
        }
    }

    /// Resets to rest.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Ensures the cached weights match the current anchor and item count.
    ///
    /// When the pointer leaves, the old weights are deliberately kept so the
    /// decay still starts from the last wave shape.
    pub fn sync_weights(&mut self, anchor: Option<usize>, count: usize, range_slots: f32) {
        if let Some(anchor) = anchor
            && (self.anchor != Some(anchor) || self.weights.len() != count)
        {
            self.weights = wave_weights(anchor, count, range_slots);
        }
    }

    /// `true` while the wave still needs frames.
    #[must_use]
    pub fn is_active(&self, now: f32, time_scale: f32) -> bool {
        if self.anchor.is_none() {
            return false;
        }
        let max_delay = self.max_delay(time_scale);
        let rise = 0.120 * time_scale;
        let decay = 0.220 * time_scale;
        now - self.started < max_delay + rise
            || self
                .leaving_at
                .is_some_and(|at| now - at < max_delay + decay)
    }

    /// Layout scales for every icon.
    #[must_use]
    pub fn sample(&self, now: f32, max_scale: f32, time_scale: f32, count: usize) -> Vec<f32> {
        let mut scales = vec![1.0; count];
        let Some(anchor) = self.anchor else {
            return scales;
        };
        let slot_delay = self.slot_delay(time_scale);
        let rise = (0.120 * time_scale).max(0.001);
        let decay = (0.220 * time_scale).max(0.001);
        for (index, slot_scale) in scales.iter_mut().enumerate().take(count) {
            let slot = index.abs_diff(anchor);
            let delay = slot as f32 * slot_delay;
            let weight = self
                .weights
                .get(index)
                .copied()
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            let mut envelope =
                ease_out_cubic(((now - self.started - delay) / rise).clamp(0.0, 1.0));
            if let Some(leaving_at) = self.leaving_at {
                let decay_at = leaving_at + delay * 0.75;
                let t = ((now - decay_at) / decay).clamp(0.0, 1.0);
                envelope *= 1.0 - ease_in_out_cubic(t);
            }
            *slot_scale = 1.0 + (max_scale - 1.0) * weight * envelope;
        }
        scales
    }

    fn slot_delay(&self, time_scale: f32) -> f32 {
        let count = self.weights.len().max(1);
        let total = (0.260 * time_scale).max(0.001);
        (total / (count.saturating_sub(1)).max(1) as f32).min(0.040 * time_scale.max(0.5))
    }

    fn max_delay(&self, time_scale: f32) -> f32 {
        self.slot_delay(time_scale) * (self.weights.len().saturating_sub(1)) as f32
    }
}

/// Distance weights used by the Wave preset.
#[must_use]
pub fn wave_weights(anchor: usize, count: usize, range_slots: f32) -> Vec<f32> {
    (0..count)
        .map(|index| {
            let distance = index.abs_diff(anchor) as f32;
            smoothstep(clamp01(1.0 - distance / range_slots.max(0.5)))
        })
        .collect()
}

/// Glide Zoom / Wave distance weights around the pointer.
#[must_use]
pub fn cursor_weights(cursor: Point, geometry: &DockGeometry, metrics: &Metrics) -> Vec<f32> {
    let range = metrics.range_slots.max(0.5) * (metrics.icon + metrics.gap);
    geometry
        .slot_centers()
        .into_iter()
        .map(|center| smoothstep(clamp01(1.0 - (center - cursor.x).abs() / range)))
        .collect()
}

/// Bounce hover lift in device pixels (negative is up).
#[must_use]
pub fn bounce_lift(metrics: &Metrics, max_scale: f32) -> f32 {
    -((max_scale - 1.0).max(0.0) * BOUNCE_LIFT_DIP) * metrics.scale
}

/// Fluid Press hover scale.
#[must_use]
pub fn fluid_hover_scale(max_scale: f32) -> f32 {
    1.0 + 0.04 * (max_scale - 1.0).max(0.0)
}

/// Gentle Glow breathing multiplier in `0.86..=1.0`.
#[must_use]
pub fn glow_breathing(elapsed: f32) -> f32 {
    let phase = (elapsed.max(0.0) % 2.4) / 2.4;
    0.93 + 0.07 * (std::f32::consts::TAU * phase).cos()
}

/// Per-icon Perspective tilt targets in degrees.
#[must_use]
pub fn perspective_targets(
    cursor: Point,
    geometry: &DockGeometry,
    metrics: &Metrics,
) -> Vec<(f32, f32)> {
    let centers = geometry.slot_centers();
    let range = metrics.range_slots.max(0.5) * (metrics.icon + metrics.gap);
    let icon_center_y = geometry.content_base.center().y;
    centers
        .into_iter()
        .map(|center| {
            let weight = smoothstep(clamp01(1.0 - (center - cursor.x).abs() / range));
            if weight <= 0.0 {
                return (0.0, 0.0);
            }
            let dx = ((cursor.x - center) / (metrics.icon * 0.9).max(1.0)).clamp(-1.0, 1.0);
            let dy = ((cursor.y - icon_center_y) / (metrics.icon * 0.9).max(1.0)).clamp(-1.0, 1.0);
            (-dx * MAX_TILT_DEG * weight, dy * MAX_TILT_DEG * weight)
        })
        .collect()
}

/// `true` when the pointer is close enough to the dock for hover effects.
#[must_use]
pub fn pointer_in_band(cursor: Point, geometry: &DockGeometry, metrics: &Metrics) -> bool {
    cursor.x.is_finite()
        && cursor.y.is_finite()
        && cursor.y >= geometry.dock_base.top() - metrics.headroom
        && cursor.y <= geometry.window.height
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

    #[test]
    fn timed_envelope_is_140ms_and_reversible() {
        let mut envelope = TimedEnvelope::default();
        envelope.retarget(0.0, 1.0, 0.180);
        assert!((envelope.value(0.090) - 0.875).abs() < 0.01);
        assert!((envelope.value(0.180) - 1.0).abs() < 0.001);
        envelope.retarget(1.0, 0.0, 0.140);
        assert!(envelope.is_active(1.010));
        assert!(!envelope.is_active(1.140));
        assert!((envelope.value(1.140) - 0.0).abs() < 0.001);
    }

    #[test]
    fn wave_delays_outer_icons() {
        let mut wave = WaveState::default();
        let weights = wave_weights(2, 5, 2.6);
        wave.trigger(0.0, 2, weights);
        let early = wave.sample(0.060, 1.45, 1.0, 5);
        let later = wave.sample(0.200, 1.45, 1.0, 5);
        assert!(early[2] > early[0]);
        assert!(later[0] > early[0]);
        assert!(wave.is_active(0.200, 1.0));
    }

    #[test]
    fn wave_decays_after_leave() {
        let mut wave = WaveState::default();
        wave.trigger(0.0, 2, wave_weights(2, 5, 2.6));
        wave.leave(0.300);
        let peak = wave.sample(0.300, 1.45, 1.0, 5)[2];
        let late = wave.sample(0.300 + 0.220, 1.45, 1.0, 5)[2];
        assert!(peak > 1.0);
        assert!(late < peak);
    }

    #[test]
    fn perspective_tilt_is_bounded() {
        let metrics = metrics();
        let geometry = DockGeometry::new(5, &metrics);
        let center = geometry.slot_centers()[2];
        let cursor = Point::new(center, geometry.content_base.center().y);
        let targets = perspective_targets(cursor, &geometry, &metrics);
        assert!(
            targets
                .iter()
                .all(|(x, y)| x.abs() <= MAX_TILT_DEG && y.abs() <= MAX_TILT_DEG)
        );
        let left = Point::new(center - metrics.icon, geometry.content_base.center().y);
        let targets = perspective_targets(left, &geometry, &metrics);
        assert!(targets[2].0 > 0.0);
    }

    #[test]
    fn bounce_lift_scales_with_max_scale() {
        let metrics = metrics();
        let low = bounce_lift(&metrics, 1.1);
        let high = bounce_lift(&metrics, 1.45);
        assert!(low < 0.0);
        assert!(high < low);
    }

    #[test]
    fn wave_weights_peak_at_anchor() {
        let weights = wave_weights(3, 7, 2.6);
        assert!((weights[3] - 1.0).abs() < 0.001);
        assert!(weights[0] < weights[1]);
        assert!(weights[6] < weights[5]);
    }

    #[test]
    fn glide_targets_peak_at_cursor() {
        let metrics = metrics();
        let geometry = DockGeometry::new(6, &metrics);
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        let weights = cursor_weights(cursor, &geometry, &metrics);
        assert!((weights[2] - 1.0).abs() < 0.001);
        assert!(weights[0] < weights[1]);
    }
}
