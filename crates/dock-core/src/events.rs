//! Commands and events exchanged between the platform layer, the application
//! state machine and the renderer.

use crate::geometry::Point;
use crate::state::MenuTarget;

/// Every user intent is translated into a command, then handled in one place.
#[derive(Debug, Clone, PartialEq)]
pub enum DockCommand {
    /// Add paths dropped onto the dock. `at` is an optional insertion index.
    AddPaths {
        /// File system paths (files, folders, shortcuts).
        paths: Vec<String>,
        /// Insertion index in the current item list.
        at: Option<usize>,
    },
    /// Remove an item by id.
    RemoveItem {
        /// Item id.
        id: u32,
    },
    /// Move an item to a new index.
    MoveItem {
        /// Source index.
        from: usize,
        /// Destination index.
        to: usize,
    },
    /// Launch an item by id.
    Activate {
        /// Item id.
        id: u32,
    },
    /// Reveal an item in Explorer.
    OpenItemLocation {
        /// Item id.
        id: u32,
    },
    /// Update the hovered item index.
    SetHovered(Option<usize>),
    /// Pointer button pressed.
    PointerDown(Point),
    /// Pointer button released.
    PointerUp(Point),
    /// Pointer moved.
    PointerMove(Point),
    /// Open the context menu.
    OpenMenu {
        /// What the menu applies to.
        target: MenuTarget,
        /// Screen-space anchor point.
        at: Point,
    },
    /// Close the context menu.
    CloseMenu,
    /// Reload the configuration from disk.
    ReloadConfig,
    /// Reload and send items to the renderer.
    RefreshItems,
    /// Quit the application.
    Shutdown,
}

/// Asynchronous notifications emitted by the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockEvent {
    /// The item list or its order changed and should be persisted.
    ItemsChanged,
    /// A launch attempt failed.
    LaunchFailed {
        /// Item id.
        id: u32,
        /// Human readable reason.
        message: String,
    },
    /// Configuration was saved successfully.
    ConfigSaved,
    /// The application should exit.
    Quit,
}
