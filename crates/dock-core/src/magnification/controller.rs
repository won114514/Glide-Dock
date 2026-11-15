//! Whole-dock continuous magnification controller.

use crate::magnification::anchors::MagnificationAnchors;
use crate::magnification::influence::target_scale;
use crate::magnification::settings::MagnificationSettings;
use crate::magnification::smoothing::exponential_step;

/// Owns the current scale of every icon for the whole dock.
///
/// There is exactly one controller and one pointer sample per frame. Icons do
/// not have independent hover animations, timers, or mouse listeners.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MagnificationController {
    current: Vec<f32>,
    active: bool,
}

impl MagnificationController {
    /// Creates a neutral controller.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets every icon to 1.0 immediately.
    pub fn reset(&mut self) {
        self.current.fill(1.0);
        self.active = false;
    }

    /// Ensures the state has one scale per anchor.
    fn sync(&mut self, count: usize) {
        if self.current.len() < count {
            self.current.resize(count, 1.0);
        } else if self.current.len() > count {
            self.current.truncate(count);
        }
    }

    /// Advances the field by `dt` seconds and returns the current scales.
    ///
    /// `cursor_x` is `None` when the pointer is outside the dock interaction
    /// band; targets then become 1.0 and the existing scales decay with the
    /// exit time constant instead of snapping.
    pub fn update(
        &mut self,
        dt: f32,
        cursor_x: Option<f32>,
        anchors: &MagnificationAnchors,
        settings: &MagnificationSettings,
    ) -> &[f32] {
        self.sync(anchors.len());
        if !settings.enabled || settings.max_scale <= 1.0 || settings.radius <= 0.0 {
            self.reset();
            return &self.current;
        }
        let cursor_x = cursor_x.filter(|x| x.is_finite());
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.05)
        } else {
            0.0
        };
        let mut active = false;
        for (scale, anchor) in self.current.iter_mut().zip(anchors.as_slice()) {
            let target = cursor_x.map_or(1.0, |x| {
                target_scale(x, *anchor, settings.max_scale, settings.radius)
            });
            let tau = if target > *scale {
                settings.enter_tau
            } else {
                settings.exit_tau
            };
            let next = exponential_step(*scale, target, dt, tau);
            if (target - next).abs() <= 1e-5 {
                *scale = target;
            } else {
                *scale = next;
                active = true;
            }
        }
        self.active = active;
        &self.current
    }

    /// Current scales.
    #[must_use]
    pub fn scales(&self) -> &[f32] {
        &self.current
    }

    /// Whether the field still needs animation frames.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{DockGeometry, Metrics};
    use crate::models::{Appearance, Behavior, MotionConfig};

    fn setup(count: usize) -> (DockGeometry, Metrics, MagnificationAnchors) {
        let metrics = Metrics::new(
            &Appearance::default(),
            &Behavior::default(),
            &MotionConfig::default(),
            1.0,
        );
        let geometry = DockGeometry::new(count, &metrics);
        let anchors = MagnificationAnchors::from_geometry(&geometry, count);
        (geometry, metrics, anchors)
    }

    #[test]
    fn centre_grows_and_neighbours_follow() {
        let (geometry, _metrics, anchors) = setup(3);
        let mut controller = MagnificationController::new();
        let settings = MagnificationSettings::classic(1.35, 112.2);
        let cursor = Some(geometry.slot_centers()[1]);
        let mut scales = Vec::new();
        for _ in 0..30 {
            scales = controller
                .update(1.0 / 60.0, cursor, &anchors, &settings)
                .to_vec();
        }
        assert!(scales[1] > 1.34);
        assert!(scales[0] > 1.0 && scales[0] < scales[1]);
        assert!(scales[2] > 1.0 && scales[2] < scales[1]);
    }

    #[test]
    fn leaving_smoothly_returns_to_one() {
        let (geometry, _metrics, anchors) = setup(3);
        let mut controller = MagnificationController::new();
        let settings = MagnificationSettings::classic(1.35, 112.2);
        let cursor = Some(geometry.slot_centers()[1]);
        for _ in 0..30 {
            controller.update(1.0 / 60.0, cursor, &anchors, &settings);
        }
        for _ in 0..50 {
            controller.update(1.0 / 60.0, None, &anchors, &settings);
        }
        assert!(controller.scales().iter().all(|scale| *scale == 1.0));
        assert!(!controller.is_active());
    }

    #[test]
    fn reentry_continues_from_the_current_scale() {
        let (geometry, _metrics, anchors) = setup(3);
        let mut controller = MagnificationController::new();
        let settings = MagnificationSettings::classic(1.35, 112.2);
        let cursor = Some(geometry.slot_centers()[1]);
        for _ in 0..10 {
            controller.update(1.0 / 60.0, cursor, &anchors, &settings);
        }
        let before = controller.scales()[1];
        controller.update(
            1.0 / 60.0,
            Some(geometry.slot_centers()[0]),
            &anchors,
            &settings,
        );
        let after = controller.scales()[1];
        assert!(after > 1.0 && after < before);
    }

    #[test]
    fn disabled_settings_neutralize_immediately() {
        let (geometry, _metrics, anchors) = setup(2);
        let mut controller = MagnificationController::new();
        let settings = MagnificationSettings::classic(1.35, 112.2);
        let cursor = Some(geometry.slot_centers()[0]);
        for _ in 0..5 {
            controller.update(1.0 / 60.0, cursor, &anchors, &settings);
        }
        controller.update(
            1.0 / 60.0,
            cursor,
            &anchors,
            &MagnificationSettings::disabled(),
        );
        assert!(controller.scales().iter().all(|scale| *scale == 1.0));
        assert!(!controller.is_active());
    }

    #[test]
    fn settles_exactly_without_residual_frames() {
        let (geometry, _metrics, anchors) = setup(3);
        let mut controller = MagnificationController::new();
        let settings = MagnificationSettings::classic(1.35, 112.2);
        for _ in 0..300 {
            controller.update(
                1.0 / 60.0,
                Some(geometry.slot_centers()[1]),
                &anchors,
                &settings,
            );
        }
        assert!(!controller.is_active());
        assert!((controller.scales()[1] - 1.35).abs() < 1e-6);
    }

    #[test]
    fn frame_rate_does_not_change_the_result() {
        let (_geometry, _metrics, anchors) = setup(3);
        let settings = MagnificationSettings::classic(1.35, 112.2);
        let cursor = Some(66.0);
        let mut sixty = MagnificationController::new();
        for _ in 0..10 {
            sixty.update(1.0 / 60.0, cursor, &anchors, &settings);
        }
        let mut high = MagnificationController::new();
        for _ in 0..24 {
            high.update(1.0 / 144.0, cursor, &anchors, &settings);
        }
        let mut middle = MagnificationController::new();
        for _ in 0..20 {
            middle.update(1.0 / 120.0, cursor, &anchors, &settings);
        }
        let a = sixty.scales()[1];
        let b = high.scales()[1];
        let c = middle.scales()[1];
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        assert!((a - c).abs() < 1e-3, "{a} vs {c}");
    }

    #[test]
    fn midpoint_between_two_icons_is_symmetric() {
        let (_geometry, _metrics, anchors) = setup(3);
        let settings = MagnificationSettings::classic(1.35, 112.2);
        let cursor = (anchors.as_slice()[1] + anchors.as_slice()[2]) / 2.0;
        let mut controller = MagnificationController::new();
        for _ in 0..60 {
            controller.update(1.0 / 60.0, Some(cursor), &anchors, &settings);
        }
        let scales = controller.scales();
        assert!((scales[1] - scales[2]).abs() < 1e-4);
        assert!(scales[1] > 1.0);
        assert!(scales[0] < scales[1]);
    }

    #[test]
    fn dpi_does_not_change_the_relative_field() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let metrics = Metrics::new(
                &Appearance::default(),
                &Behavior::default(),
                &MotionConfig::default(),
                dpi,
            );
            let geometry = DockGeometry::new(3, &metrics);
            let anchors = MagnificationAnchors::from_geometry(&geometry, 3);
            let mut controller = MagnificationController::new();
            let settings = MagnificationSettings::classic(1.35, 112.2 * dpi);
            for _ in 0..60 {
                controller.update(
                    1.0 / 120.0,
                    Some(geometry.slot_centers()[1]),
                    &anchors,
                    &settings,
                );
            }
            let scales = controller.scales();
            assert!((scales[1] - 1.35).abs() < 0.01, "dpi {dpi}: {scales:?}");
            assert!(scales[0] > 1.0 && scales[0] < scales[1], "dpi {dpi}");
        }
    }
}
