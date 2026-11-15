//! Event-driven top-level window tracking.
//!
//! The dock installs one `SetWinEventHook` for create/show/hide/destroy and
//! re-scans visible windows only when such an event arrives. Animation never
//! polls process state to decide whether a launch succeeded.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE, GA_ROOT, GetAncestor, GetWindowThreadProcessId,
    PostMessageW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

/// Message posted by the tracker; wparam is the window's process id.
pub const WM_APP_WINDOW_EVENT: u32 = 0x8000 + 2;
/// Message posted by the background launch worker.
pub const WM_APP_LAUNCH_RESULT: u32 = 0x8000 + 3;

static NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);

/// Owns the WinEvent hook for the lifetime of the dock.
pub struct WindowTracker {
    hook: HWINEVENTHOOK,
}

impl WindowTracker {
    /// Installs the tracker. The callback only posts a message; it never
    /// touches application state from the Win32 callback thread.
    #[must_use]
    pub fn install(notify: HWND) -> Option<Self> {
        NOTIFY_HWND.store(notify.0 as isize, Ordering::SeqCst);
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_CREATE,
                EVENT_OBJECT_HIDE,
                None,
                Some(window_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.0.is_null() {
            NOTIFY_HWND.store(0, Ordering::SeqCst);
            tracing::warn!("SetWinEventHook failed; running state will only be seeded once");
            return None;
        }
        Some(Self { hook })
    }

    /// Returns `true` when a visible top-level window already belongs to `path`.
    #[must_use]
    pub fn has_visible_window(path: &str) -> bool {
        crate::system::has_visible_window(path)
    }
}

impl Drop for WindowTracker {
    fn drop(&mut self) {
        NOTIFY_HWND.store(0, Ordering::SeqCst);
        unsafe {
            let _ = UnhookWinEvent(self.hook);
        }
    }
}

unsafe extern "system" fn window_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != 0 || id_child != 0 || hwnd.0.is_null() {
        return;
    }
    // Ignore events for child HWNDs (controls, IME windows, ...). Only a
    // top-level create/show/hide/destroy can change the visible-window set.
    // A destroyed window may no longer resolve its root; let that event through
    // so the running dot can turn off.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if !root.0.is_null() && root != hwnd {
        return;
    }
    let notify = NOTIFY_HWND.load(Ordering::SeqCst);
    if notify == 0 {
        return;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 {
        return;
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(notify as *mut _)),
            WM_APP_WINDOW_EVENT,
            WPARAM(pid as usize),
            LPARAM(0),
        );
    }
}
