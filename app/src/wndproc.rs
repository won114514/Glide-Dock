//! Window procedure entry point. All real work is delegated to [`DockApp`].

use std::panic::{AssertUnwindSafe, catch_unwind};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW};

use crate::app::DockApp;

/// Window procedure shared by the dock window.
pub unsafe extern "system" fn dock_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut DockApp;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let app = unsafe { &mut *pointer };
        app.handle_message(hwnd, msg, wparam, lparam)
    }));
    match outcome {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(%message, "panic in window procedure");
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}
