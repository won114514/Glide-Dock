//! Background launcher service.
//!
//! The service only starts applications off the UI thread and reports the
//! result. It deliberately knows nothing about animation state: the motion
//! engine is told about launches by `DockApp`, and the window tracker is the
//! only source of truth for a running application.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use dock_core::DockItem;
use dock_platform_windows::{ComGuard, WM_APP_LAUNCH_RESULT, launch};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// One completed background launch request.
pub type LaunchResult = (u32, Result<(), String>);

/// Queue shared with launch worker threads.
type LaunchQueue = Arc<Mutex<Vec<LaunchResult>>>;

/// Owns the background launch queue for one dock window.
#[derive(Clone)]
pub struct LauncherService {
    hwnd: HWND,
    results: LaunchQueue,
}

impl LauncherService {
    /// Creates a service that wakes `hwnd` with `WM_APP_LAUNCH_RESULT`.
    #[must_use]
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            results: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Starts `item` on a background thread. The UI thread is never blocked by
    /// ShellExecuteEx/CreateProcessW.
    pub fn request(&self, item: &DockItem) {
        let item = item.clone();
        let results = Arc::clone(&self.results);
        let hwnd = self.hwnd.0 as isize;
        tracing::info!(
            id = item.id,
            target = %item.target_path,
            launch_type = ?item.launch_type,
            "launch requested"
        );
        std::thread::spawn(move || {
            let _com = ComGuard::sta();
            let result = launch(&item);
            if let Ok(mut queue) = results.lock() {
                queue.push((item.id, result));
            }
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(hwnd as *mut c_void)),
                    WM_APP_LAUNCH_RESULT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
    }

    /// Drains completed launch results.
    #[must_use]
    pub fn drain(&self) -> Vec<LaunchResult> {
        self.results
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_drain_is_empty() {
        let service = LauncherService::new(HWND::default());
        assert!(service.drain().is_empty());
    }
}
