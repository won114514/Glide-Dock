//! Magnification parameters.

/// Enter time constant from the Apple-style specification, in seconds.
pub const CLASSIC_ENTER_TAU_S: f32 = 0.035;
/// Exit time constant from the Apple-style specification, in seconds.
pub const CLASSIC_EXIT_TAU_S: f32 = 0.050;

/// Runtime settings for one frame of the magnification field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagnificationSettings {
    /// Master switch.
    pub enabled: bool,
    /// Peak scale at the anchor centre.
    pub max_scale: f32,
    /// Influence radius in DIP.
    pub radius: f32,
    /// Time constant while growing, in seconds.
    pub enter_tau: f32,
    /// Time constant while shrinking, in seconds.
    pub exit_tau: f32,
}

impl MagnificationSettings {
    /// Classic Apple-style settings.
    #[must_use]
    pub fn classic(max_scale: f32, radius: f32) -> Self {
        Self {
            enabled: true,
            max_scale,
            radius,
            enter_tau: CLASSIC_ENTER_TAU_S,
            exit_tau: CLASSIC_EXIT_TAU_S,
        }
    }

    /// Neutral settings: every icon returns to 1.0 immediately.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            max_scale: 1.0,
            radius: 0.0,
            enter_tau: CLASSIC_ENTER_TAU_S,
            exit_tau: CLASSIC_EXIT_TAU_S,
        }
    }
}

impl Default for MagnificationSettings {
    fn default() -> Self {
        Self::classic(1.35, 112.2)
    }
}
