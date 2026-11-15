//! Pure distance → scale influence function.

use crate::geometry::{clamp01, smoothstep};

/// Converts a horizontal distance into a smooth 0..=1 weight.
#[must_use]
pub fn weight(distance: f32, radius: f32) -> f32 {
    if !distance.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return 0.0;
    }
    smoothstep(clamp01(1.0 - distance / radius))
}

/// Target scale for one icon.
#[must_use]
pub fn target_scale(cursor_x: f32, anchor_x: f32, max_scale: f32, radius: f32) -> f32 {
    if !cursor_x.is_finite() || !anchor_x.is_finite() || !max_scale.is_finite() || max_scale <= 1.0
    {
        return 1.0;
    }
    let influence = weight((cursor_x - anchor_x).abs(), radius);
    1.0 + (max_scale - 1.0) * influence
}

/// Target scales for every anchor.
#[must_use]
pub fn target_scales(
    anchors: &[f32],
    cursor_x: Option<f32>,
    max_scale: f32,
    radius: f32,
) -> Vec<f32> {
    let Some(cursor_x) = cursor_x.filter(|x| x.is_finite()) else {
        return vec![1.0; anchors.len()];
    };
    anchors
        .iter()
        .map(|anchor| target_scale(cursor_x, *anchor, max_scale, radius))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_reaches_the_peak_and_edges_are_neutral() {
        assert!((target_scale(100.0, 100.0, 1.35, 112.2) - 1.35).abs() < 1e-5);
        assert!((target_scale(100.0 + 112.2, 100.0, 1.35, 112.2) - 1.0).abs() < 1e-5);
        assert!((target_scale(100.0 + 200.0, 100.0, 1.35, 112.2) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn weight_is_smooth_and_monotonic() {
        let near = weight(10.0, 112.2);
        let middle = weight(56.1, 112.2);
        let far = weight(100.0, 112.2);
        assert!(near > middle && middle > far && far >= 0.0);
    }

    #[test]
    fn target_scales_are_symmetric() {
        let anchors = [0.0, 66.0, 132.0];
        let left = target_scales(&anchors, Some(56.0), 1.35, 112.2);
        let right = target_scales(&anchors, Some(76.0), 1.35, 112.2);
        assert!((left[1] - right[1]).abs() < 1e-5);
        assert!((left[0] - right[2]).abs() < 1e-5);
        assert!((left[2] - right[0]).abs() < 1e-5);
    }
}
