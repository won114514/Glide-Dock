//! File drop handling.

use dock_core::Point;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, HDROP,
};

/// A file drop translated to window-local coordinates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DropPayload {
    /// Dropped file system paths.
    pub paths: Vec<String>,
    /// Drop point in client coordinates, when the source provided one.
    pub client_point: Option<Point>,
}

/// Enables shell file drops on a window.
pub fn accept_files(hwnd: HWND) {
    unsafe {
        DragAcceptFiles(hwnd, true);
    }
}

/// Extracts every path from a `WM_DROPFILES` payload and finishes the drop.
#[must_use]
pub fn parse_drop(hdrop: HDROP) -> DropPayload {
    let mut paths = Vec::new();
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    for index in 0..count {
        let length = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        if length == 0 {
            continue;
        }
        let mut buffer = vec![0u16; length + 1];
        let written = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) } as usize;
        if written == 0 {
            continue;
        }
        paths.push(String::from_utf16_lossy(&buffer[..written]));
    }

    let mut point = POINT::default();
    let has_point = unsafe { DragQueryPoint(hdrop, &mut point) }.0 != 0;
    unsafe {
        DragFinish(hdrop);
    }

    DropPayload {
        paths,
        client_point: has_point.then_some(Point::new(point.x as f32, point.y as f32)),
    }
}
