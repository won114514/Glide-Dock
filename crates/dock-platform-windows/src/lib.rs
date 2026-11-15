//! Glide-Dock's Windows platform layer.
//!
//! This crate is the only place that talks to Win32 / COM. It knows nothing
//! about configuration files or the renderer.

#![warn(missing_docs)]

pub mod com;
pub mod dragdrop;
pub mod error;
pub mod glass;
pub mod icons;
pub mod monitor;
pub mod resolver;
pub mod shell;
pub mod store;
pub mod surface;
pub mod system;
pub mod tracker;
pub mod tray;
pub mod window;

pub use com::ComGuard;
pub use dragdrop::{DropPayload, accept_files, parse_drop};
pub use error::{PlatformError, Result};
pub use glass::GlassBackdrop;
pub use icons::{IconImage, IconResult, IconService, WM_ICON_READY, extract_icon};
pub use monitor::{
    MonitorMetrics, WorkArea, clamp_window, metrics_for_window, place_dock, primary_work_area,
    refresh_hz,
};
pub use resolver::{ResolveResult, ResolveService, WM_PATHS_RESOLVED};
pub use shell::{
    ResolvedEntry, expand_environment, launch, open_path, open_start_menu, resolve, reveal,
};
pub use store::{StoreApp, StoreAppService, WM_APP_STORE_APPS, pick_store_app, scan_store_apps};
pub use surface::LayeredSurface;
pub use system::{
    SingleInstance, SingleInstanceOutcome, activate_window, animations_enabled,
    find_window_by_title, has_visible_window, is_elevated, is_low_integrity, prefers_light_theme,
    running_executables, show_message, visible_window_paths, wait_for_desktop, windows_build,
};
pub use tracker::{WM_APP_LAUNCH_RESULT, WM_APP_WINDOW_EVENT, WindowTracker};
pub use tray::{TrayCommand, TrayEvent, TrayIcon, WM_APP_TRAY};
pub use window::{
    capture_mouse, client_to_screen, create_layered_popup, cursor_position, hide, last_error,
    place_above_desktop, register_class, release_mouse, screen_to_client, set_bounds, set_topmost,
    show_no_activate, wide,
};

/// Window class name for the dock window.
pub const DOCK_WINDOW_CLASS: &str = "Glide-Dock.DockWindow";

/// Window class name for the context menu window.
pub const MENU_WINDOW_CLASS: &str = "Glide-Dock.MenuWindow";
