//! Glide-Dock domain core: pure data, geometry and animation.
//!
//! This crate has **no OS dependencies**: it never touches the filesystem, the
//! registry or the Win32 API. That makes layout and animation deterministic and
//! unit-testable.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod animation;
pub mod events;
pub mod geometry;
pub mod layout;
pub mod magnification;
pub mod models;
pub mod motion;
pub mod settings_ipc;
pub mod state;
pub mod theme;

pub use animation::{ScalarSpring, Spring, SpringSet};
pub use events::{DockCommand, DockEvent};
pub use geometry::{Point, Rect, Size};
pub use layout::{
    DockGeometry, FrameLayout, Metrics, insertion_index, insertion_x, target_scales, tooltip_rect,
};
pub use models::{
    Appearance, Behavior, CONFIG_VERSION, DockConfig, DockFont, DockItem, DockLayer, DockPosition,
    HoverPreset, ItemKind, LaunchPreset, LaunchType, MAX_HOVER_SCALE, MonitorConfig, MotionConfig,
};
pub use motion::{
    IconMotion, MotionEngine, MotionSnapshot, MotionTuning, RippleMotion, waiting_opacity,
};
pub use state::{
    DockState, DragSession, ErrorToast, MenuAction, MenuEntry, MenuIcon, MenuTarget, TooltipState,
};
pub use theme::{AccentColor, Color, Palette, ResolvedTheme, ThemeMode};
