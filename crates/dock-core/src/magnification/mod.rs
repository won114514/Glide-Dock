//! Continuous Apple-style magnification field.
//!
//! The field is computed from the fixed base centres of every icon, never from
//! transformed/reflowed geometry. One controller owns all scales for the whole
//! dock; individual icons do not own hover animations.

mod anchors;
mod controller;
mod influence;
mod settings;
mod smoothing;

pub use anchors::MagnificationAnchors;
pub use controller::MagnificationController;
pub use influence::{target_scale, target_scales, weight};
pub use settings::{CLASSIC_ENTER_TAU_S, CLASSIC_EXIT_TAU_S, MagnificationSettings};
pub use smoothing::exponential_step;
