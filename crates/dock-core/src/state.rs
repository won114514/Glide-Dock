//! Runtime state shared by the application layer and the renderer.

use std::collections::BTreeSet;

use crate::geometry::Point;

/// The context menu target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    /// A specific dock item.
    Item(u32),
    /// The dock background.
    Background,
}

/// Icons drawn next to context menu entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIcon {
    /// Launch / open.
    Open,
    /// Show in Explorer.
    Folder,
    /// Remove from the dock.
    Trash,
    /// Settings.
    Settings,
    /// Reload configuration.
    Refresh,
    /// Reveal the configuration folder.
    Edit,
    /// Quit the application.
    Power,
    /// Appearance / theme.
    Appearance,
    /// Dock layer.
    Layers,
    /// Tips and tricks.
    Tips,
    /// Check mark for a selected option.
    Check,
    /// No glyph, used to keep option columns aligned.
    Blank,
    /// Desktop-only display.
    Desktop,
    /// Any-window display.
    Window,
    /// Maximized-window display.
    Maximize,
    /// Start menu display.
    Start,
    /// Search display.
    Search,
    /// Task view display.
    TaskView,
}

/// Action attached to a context menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// No action; used by submenu rows.
    Noop,
    /// Launch the item.
    Activate(u32),
    /// Reveal the item in Explorer.
    OpenLocation(u32),
    /// Remove the item from the dock.
    Remove(u32),
    /// Open the settings window.
    OpenSettings,
    /// Open the configuration folder.
    OpenConfigDir,
    /// Add the built-in Windows Start menu widget.
    AddStartWidget,
    /// Add a visual separator.
    AddSeparator,
    /// Scan installed Store / packaged apps and add one.
    AddStoreApp,
    /// Reload the configuration from disk.
    ReloadConfig,
    /// Quit Glide-Dock.
    Quit,
    /// Follow the Windows app theme.
    ThemeSystem,
    /// Force the light theme.
    ThemeLight,
    /// Force the dark theme.
    ThemeDark,
    /// Keep the dock above the desktop.
    LayerDesktop,
    /// Raise the dock while interacting.
    LayerDynamic,
    /// Keep the dock above normal windows.
    LayerTopmost,
}

/// One context menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A clickable entry.
    Action {
        /// Visible label.
        label: String,
        /// Action performed on click.
        action: MenuAction,
        /// Glyph drawn on the left.
        icon: MenuIcon,
        /// Destructive entries use the danger color.
        danger: bool,
        /// Draw a submenu chevron instead of activating directly.
        chevron: bool,
        /// Draw a check mark in the icon column.
        checked: bool,
    },
    /// A thin separator line.
    Separator,
}

impl MenuEntry {
    /// Creates a regular entry.
    #[must_use]
    pub fn action(label: impl Into<String>, action: MenuAction, icon: MenuIcon) -> Self {
        Self::Action {
            label: label.into(),
            action,
            icon,
            danger: false,
            chevron: false,
            checked: false,
        }
    }

    /// Creates a destructive entry.
    #[must_use]
    pub fn danger(label: impl Into<String>, action: MenuAction, icon: MenuIcon) -> Self {
        Self::Action {
            label: label.into(),
            action,
            icon,
            danger: true,
            chevron: false,
            checked: false,
        }
    }

    /// Creates a submenu row; the app attaches the child entries.
    #[must_use]
    pub fn submenu(label: impl Into<String>, icon: MenuIcon) -> Self {
        Self::Action {
            label: label.into(),
            action: MenuAction::Noop,
            icon,
            danger: false,
            chevron: true,
            checked: false,
        }
    }

    /// Creates an entry with a check mark in the icon column.
    #[must_use]
    pub fn checked(label: impl Into<String>, action: MenuAction) -> Self {
        Self::Action {
            label: label.into(),
            action,
            icon: MenuIcon::Check,
            danger: false,
            chevron: false,
            checked: true,
        }
    }

    /// Creates a checkable option; unchecked options keep an empty icon column.
    #[must_use]
    pub fn option(label: impl Into<String>, action: MenuAction, checked: bool) -> Self {
        Self::Action {
            label: label.into(),
            action,
            icon: if checked {
                MenuIcon::Check
            } else {
                MenuIcon::Blank
            },
            danger: false,
            chevron: false,
            checked,
        }
    }
}

/// A drag session in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct DragSession {
    /// Id of the dragged item.
    pub item_id: u32,
    /// Original index of the dragged item.
    pub from: usize,
    /// Cursor offset from the icon's left edge when the drag started.
    pub grab_dx: f32,
    /// Where the pointer went down (for the threshold test).
    pub origin: Point,
    /// Current pointer position in window-local coordinates.
    pub cursor: Point,
    /// Current insertion index in the *remaining* item list.
    pub insertion: usize,
    /// Whether the 8 DIP threshold has been crossed.
    pub active: bool,
}

/// Progress of the hover tooltip.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TooltipState {
    /// Item index currently described.
    pub index: Option<usize>,
    /// Fade-in progress in `0..=1` (driven by a spring in the app).
    pub progress: f32,
}

/// A transient error bubble rendered above the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorToast {
    /// Message text.
    pub message: String,
    /// Monotonic counter used to restart the fade animation.
    pub serial: u64,
}

/// Everything that changes while the dock is running.
#[derive(Debug, Clone, Default)]
pub struct DockState {
    /// Cursor position in window-local pixel coordinates, `None` when the
    /// pointer is not over the dock.
    pub cursor: Option<Point>,
    /// Index of the hovered item.
    pub hovered: Option<usize>,
    /// Id of the item pressed but not yet released.
    pub pressed: Option<u32>,
    /// Active drag session.
    pub drag: Option<DragSession>,
    /// Ids of items whose process is running.
    pub running: BTreeSet<u32>,
    /// Ids of items whose target path no longer resolves.
    pub unresolved: BTreeSet<u32>,
    /// Hover tooltip state.
    pub tooltip: TooltipState,
    /// Transient error message.
    pub error: Option<ErrorToast>,
    /// True while the context menu is open.
    pub menu_open: bool,
}

impl DockState {
    /// Returns `true` when the given item is running.
    #[must_use]
    pub fn is_running(&self, id: u32) -> bool {
        self.running.contains(&id)
    }

    /// Returns `true` when the given item is unresolved.
    #[must_use]
    pub fn is_unresolved(&self, id: u32) -> bool {
        self.unresolved.contains(&id)
    }

    /// Clears transient pointer state (used when the window loses the pointer).
    pub fn clear_pointer(&mut self) {
        self.cursor = None;
        self.hovered = None;
        self.pressed = None;
        self.tooltip.index = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_and_unresolved_are_independent() {
        let mut state = DockState::default();
        state.running.insert(1);
        state.unresolved.insert(2);
        assert!(state.is_running(1));
        assert!(!state.is_unresolved(1));
        assert!(state.is_unresolved(2));
    }

    #[test]
    fn clear_pointer_resets_transient_fields() {
        let mut state = DockState {
            cursor: Some(Point::new(1.0, 2.0)),
            hovered: Some(3),
            pressed: Some(4),
            tooltip: TooltipState {
                index: Some(3),
                progress: 0.5,
            },
            ..DockState::default()
        };
        state.clear_pointer();
        assert!(state.cursor.is_none());
        assert!(state.hovered.is_none());
        assert!(state.pressed.is_none());
        assert!(state.tooltip.index.is_none());
    }
}
