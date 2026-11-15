//! Color palette for the dark and light themes.
//!
//! Colors are stored as straight (non-premultiplied) 8-bit RGBA so the theme can
//! be unit tested without pulling in a rasterizer.

use serde::{Deserialize, Serialize};

/// User facing theme selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// Follow the Windows "app mode" setting.
    #[default]
    System,
    /// Always use the light palette.
    Light,
    /// Always use the dark palette.
    Dark,
}

impl ThemeMode {
    /// Resolves [`ThemeMode::System`] against the given system preference.
    #[must_use]
    pub fn resolve(self, system_prefers_light: bool) -> ResolvedTheme {
        match self {
            Self::Light => ResolvedTheme::Light,
            Self::Dark => ResolvedTheme::Dark,
            Self::System => {
                if system_prefers_light {
                    ResolvedTheme::Light
                } else {
                    ResolvedTheme::Dark
                }
            }
        }
    }
}

/// The effective palette after resolving [`ThemeMode::System`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedTheme {
    /// Dark glass.
    Dark,
    /// Light glass.
    Light,
}

/// Straight RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

impl Color {
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// Creates a color.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Returns the color with a different alpha.
    #[must_use]
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Scales the alpha channel.
    #[must_use]
    pub fn scale_alpha(self, factor: f32) -> Self {
        let a = (f32::from(self.a) * factor.clamp(0.0, 1.0)).round();
        self.with_alpha(a.clamp(0.0, 255.0) as u8)
    }

    /// Linear interpolation between two straight colors.
    #[must_use]
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let channel =
            |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Self::rgba(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
            channel(self.a, other.a),
        )
    }

    /// Converts the color to floats in the `0.0..=1.0` range.
    #[must_use]
    pub fn to_floats(self) -> [f32; 4] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            f32::from(self.a) / 255.0,
        ]
    }
}

/// User facing accent color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentColor {
    /// Windows blue.
    #[default]
    Blue,
    /// Teal.
    Teal,
    /// Violet.
    Violet,
    /// Warm orange.
    Orange,
    /// Rose pink.
    Pink,
}

impl AccentColor {
    /// All selectable accents, in menu order.
    pub const ALL: [Self; 5] = [
        Self::Blue,
        Self::Teal,
        Self::Violet,
        Self::Orange,
        Self::Pink,
    ];

    /// Bright accent used on the dark theme.
    #[must_use]
    pub fn dark(self) -> Color {
        match self {
            Self::Blue => Color::rgb(96, 205, 255),
            Self::Teal => Color::rgb(45, 212, 191),
            Self::Violet => Color::rgb(182, 156, 255),
            Self::Orange => Color::rgb(255, 164, 92),
            Self::Pink => Color::rgb(255, 143, 199),
        }
    }

    /// Deep accent used on the light theme.
    #[must_use]
    pub fn light(self) -> Color {
        match self {
            Self::Blue => Color::rgb(0, 103, 192),
            Self::Teal => Color::rgb(15, 118, 110),
            Self::Violet => Color::rgb(109, 40, 217),
            Self::Orange => Color::rgb(194, 65, 12),
            Self::Pink => Color::rgb(190, 24, 93),
        }
    }

    /// Resolves the accent for a theme.
    #[must_use]
    pub fn resolve(self, theme: ResolvedTheme) -> Color {
        match theme {
            ResolvedTheme::Dark => self.dark(),
            ResolvedTheme::Light => self.light(),
        }
    }

    /// Stable identifier used by the settings UI.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Teal => "teal",
            Self::Violet => "violet",
            Self::Orange => "orange",
            Self::Pink => "pink",
        }
    }
}

/// A complete palette for one resolved theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Which theme this palette belongs to.
    pub theme: ResolvedTheme,
    /// Dock gradient top color (alpha is replaced by configured opacity).
    pub dock_top: Color,
    /// Dock gradient bottom color (alpha is replaced by configured opacity).
    pub dock_bottom: Color,
    /// One pixel outer border.
    pub dock_border: Color,
    /// One pixel inner highlight along the top edge.
    pub dock_highlight: Color,
    /// Inner shadow along the bottom edge.
    pub dock_inner_shadow: Color,
    /// Outer drop shadow.
    pub shadow: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text.
    pub text_dim: Color,
    /// Tertiary text / hints.
    pub text_faint: Color,
    /// Hover overlay for icons.
    pub hover: Color,
    /// Hover overlay for menu entries.
    pub menu_hover: Color,
    /// Pressed overlay.
    pub pressed: Color,
    /// Accent color (indicators, sliders, selection).
    pub accent: Color,
    /// Blue-violet accent used by glow, ripple and launch light effects.
    pub motion_accent: Color,
    /// Running application indicator.
    pub running: Color,
    /// Tooltip background.
    pub tooltip_bg: Color,
    /// Tooltip border.
    pub tooltip_border: Color,
    /// Tooltip text.
    pub tooltip_text: Color,
    /// Context menu background.
    pub menu_bg: Color,
    /// Context menu border.
    pub menu_border: Color,
    /// Context menu icon color.
    pub menu_icon: Color,
    /// Context menu text.
    pub menu_text: Color,
    /// Danger action text.
    pub danger: Color,
    /// Separator line inside menus.
    pub separator: Color,
    /// Empty state dashed border.
    pub empty_border: Color,
    /// Empty state glyph.
    pub empty_glyph: Color,
    /// Placeholder tile base color.
    pub placeholder: Color,
    /// Error toast background.
    pub error_bg: Color,
    /// Error toast text.
    pub error_text: Color,
}

impl Palette {
    /// Returns the palette for a resolved theme.
    #[must_use]
    pub fn for_theme(theme: ResolvedTheme) -> Self {
        match theme {
            ResolvedTheme::Dark => Self::dark(),
            ResolvedTheme::Light => Self::light(),
        }
    }

    /// The default dark palette: graphite glass with a cool tint.
    #[must_use]
    pub fn dark() -> Self {
        Self {
            theme: ResolvedTheme::Dark,
            dock_top: Color::rgb(58, 58, 66),
            dock_bottom: Color::rgb(24, 24, 29),
            dock_border: Color::rgba(255, 255, 255, 26),
            dock_highlight: Color::rgba(255, 255, 255, 54),
            dock_inner_shadow: Color::rgba(0, 0, 0, 90),
            shadow: Color::rgba(0, 0, 0, 150),
            text: Color::rgba(245, 245, 248, 242),
            text_dim: Color::rgba(178, 178, 188, 220),
            text_faint: Color::rgba(138, 138, 150, 190),
            hover: Color::rgba(255, 255, 255, 18),
            menu_hover: Color::rgba(255, 255, 255, 24),
            pressed: Color::rgba(255, 255, 255, 14),
            accent: Color::rgb(96, 205, 255),
            motion_accent: Color::rgb(96, 205, 255).mix(Color::rgb(167, 139, 250), 0.45),
            running: Color::rgba(236, 236, 242, 235),
            tooltip_bg: Color::rgba(26, 26, 32, 250),
            tooltip_border: Color::rgba(255, 255, 255, 30),
            tooltip_text: Color::rgba(250, 250, 252, 248),
            menu_bg: Color::rgba(32, 32, 39, 252),
            menu_border: Color::rgba(255, 255, 255, 26),
            menu_icon: Color::rgba(220, 220, 228, 230),
            menu_text: Color::rgba(242, 242, 246, 246),
            danger: Color::rgba(255, 122, 122, 250),
            separator: Color::rgba(255, 255, 255, 20),
            empty_border: Color::rgba(255, 255, 255, 34),
            empty_glyph: Color::rgba(160, 160, 172, 210),
            placeholder: Color::rgb(70, 70, 82),
            error_bg: Color::rgba(58, 28, 32, 252),
            error_text: Color::rgba(255, 214, 214, 250),
        }
    }

    /// The light palette: frosted porcelain glass.
    #[must_use]
    pub fn light() -> Self {
        Self {
            theme: ResolvedTheme::Light,
            dock_top: Color::rgb(255, 255, 255),
            dock_bottom: Color::rgb(232, 232, 238),
            dock_border: Color::rgba(0, 0, 0, 24),
            dock_highlight: Color::rgba(255, 255, 255, 235),
            dock_inner_shadow: Color::rgba(0, 0, 0, 34),
            shadow: Color::rgba(0, 0, 0, 64),
            text: Color::rgba(26, 26, 30, 246),
            text_dim: Color::rgba(92, 92, 102, 230),
            text_faint: Color::rgba(128, 128, 138, 210),
            hover: Color::rgba(0, 0, 0, 12),
            menu_hover: Color::rgba(0, 0, 0, 16),
            pressed: Color::rgba(0, 0, 0, 10),
            accent: Color::rgb(0, 103, 192),
            motion_accent: Color::rgb(0, 103, 192).mix(Color::rgb(124, 58, 237), 0.40),
            running: Color::rgba(30, 30, 34, 210),
            tooltip_bg: Color::rgba(252, 252, 255, 252),
            tooltip_border: Color::rgba(0, 0, 0, 22),
            tooltip_text: Color::rgba(24, 24, 28, 250),
            menu_bg: Color::rgba(250, 250, 253, 252),
            menu_border: Color::rgba(0, 0, 0, 24),
            menu_icon: Color::rgba(60, 60, 68, 235),
            menu_text: Color::rgba(28, 28, 32, 246),
            danger: Color::rgba(196, 43, 28, 250),
            separator: Color::rgba(0, 0, 0, 20),
            empty_border: Color::rgba(0, 0, 0, 40),
            empty_glyph: Color::rgba(90, 90, 100, 210),
            placeholder: Color::rgb(210, 210, 218),
            error_bg: Color::rgba(255, 235, 235, 252),
            error_text: Color::rgba(160, 30, 30, 250),
        }
    }

    /// Background gradient with the configured opacity applied.
    #[must_use]
    pub fn dock_background(&self, opacity: f32) -> (Color, Color) {
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        (
            self.dock_top.with_alpha(alpha),
            self.dock_bottom.with_alpha(alpha),
        )
    }

    /// Returns the palette with a user selected accent color.
    #[must_use]
    pub fn with_accent(mut self, accent: AccentColor) -> Self {
        self.accent = accent.resolve(self.theme);
        self.motion_accent = match self.theme {
            ResolvedTheme::Dark => self.accent.mix(Color::rgb(167, 139, 250), 0.45),
            ResolvedTheme::Light => self.accent.mix(Color::rgb(124, 58, 237), 0.40),
        };
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_theme_follows_preference() {
        assert_eq!(ThemeMode::System.resolve(true), ResolvedTheme::Light);
        assert_eq!(ThemeMode::System.resolve(false), ResolvedTheme::Dark);
        assert_eq!(ThemeMode::Dark.resolve(true), ResolvedTheme::Dark);
        assert_eq!(ThemeMode::Light.resolve(false), ResolvedTheme::Light);
    }

    #[test]
    fn opacity_only_changes_alpha() {
        let palette = Palette::dark();
        let (top, _) = palette.dock_background(0.5);
        assert_eq!(top.r, palette.dock_top.r);
        assert_eq!(top.a, 128);
    }

    #[test]
    fn color_mix_hits_endpoints() {
        let a = Color::rgb(0, 0, 0);
        let b = Color::rgb(255, 255, 255);
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
        assert_eq!(a.mix(b, 0.5), Color::rgb(128, 128, 128));
    }

    #[test]
    fn accent_changes_with_theme() {
        let dark = Palette::dark().with_accent(AccentColor::Teal);
        let light = Palette::light().with_accent(AccentColor::Teal);
        assert_eq!(dark.accent, AccentColor::Teal.dark());
        assert_eq!(light.accent, AccentColor::Teal.light());
        assert_ne!(dark.accent, light.accent);
    }

    #[test]
    fn accent_round_trips() {
        let json = serde_json::to_string(&AccentColor::Violet).expect("serialize");
        assert_eq!(json, "\"violet\"");
        let restored: AccentColor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, AccentColor::Violet);
    }

    #[test]
    fn motion_accent_is_blue_violet_and_follows_the_accent() {
        let blue = Palette::dark().with_accent(AccentColor::Blue);
        let violet = Palette::dark().with_accent(AccentColor::Violet);
        assert_ne!(blue.motion_accent, blue.accent);
        assert_ne!(violet.motion_accent, violet.accent);
        assert_ne!(blue.motion_accent, violet.motion_accent);
        assert!(blue.motion_accent.b > blue.motion_accent.g);
    }
}
