//! Fixed magnification anchors.
//!
//! Anchors are the unmodified layout centres of the icons. They are captured
//! once per geometry change and must never be derived from a transformed icon
//! rectangle; doing so creates a feedback loop and visible jitter.

use crate::layout::DockGeometry;

/// Horizontal base centres, in DIP, indexed like `DockGeometry::slot_centers`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MagnificationAnchors {
    centers: Vec<f32>,
}

impl MagnificationAnchors {
    /// Captures the fixed centres for `count` icons.
    #[must_use]
    pub fn from_geometry(geometry: &DockGeometry, count: usize) -> Self {
        Self {
            centers: geometry.slot_centers().into_iter().take(count).collect(),
        }
    }

    /// Anchor slice.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.centers
    }

    /// Number of anchors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.centers.len()
    }

    /// Whether there are no anchors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.centers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Metrics;
    use crate::models::{Appearance, Behavior, MotionConfig};

    #[test]
    fn anchors_are_the_fixed_slot_centres() {
        let metrics = Metrics::new(
            &Appearance::default(),
            &Behavior::default(),
            &MotionConfig::default(),
            1.0,
        );
        let geometry = DockGeometry::new(5, &metrics);
        let anchors = MagnificationAnchors::from_geometry(&geometry, 5);
        assert_eq!(anchors.len(), 5);
        assert_eq!(anchors.as_slice(), geometry.slot_centers().as_slice());
    }
}
