//! Window procedure entry point. All real work is delegated to [`DockApp`].

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, PostMessageW, SPI_SETWORKAREA,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_NULL, WM_SETTINGCHANGE,
};

use crate::app::DockApp;

pub(crate) struct DockWindowState {
    pub(crate) app: RefCell<DockApp>,
    hwnd: HWND,
    display_changed: Cell<bool>,
}

impl DockWindowState {
    pub(crate) fn new(hwnd: HWND, app: DockApp) -> Self {
        Self {
            app: RefCell::new(app),
            hwnd,
            display_changed: Cell::new(false),
        }
    }

    fn queue_display_change(&self) {
        if !self.display_changed.replace(true) {
            // Sent messages can arrive while GetMessage is waiting for a
            // queued message. Post a wake so the outer loop runs again.
            let _ = unsafe { PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
        }
    }

    pub(crate) fn apply_display_change(&self) {
        if self.display_changed.take() {
            self.app.borrow_mut().on_display_changed();
        }
    }
}

/// Window procedure shared by the Dock and its tray menu.
pub unsafe extern "system" fn dock_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const DockWindowState;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    let window = unsafe { &*pointer };
    if hwnd == window.hwnd
        && (matches!(msg, WM_DPICHANGED | WM_DISPLAYCHANGE)
            || (msg == WM_SETTINGCHANGE && wparam.0 == SPI_SETWORKAREA.0 as usize))
    {
        // A resize or presentation can synchronously reenter this procedure.
        // Record display changes outside the app borrow, then resample the
        // monitor once the outer handler has returned.
        window.queue_display_change();
        return LRESULT(0);
    }

    let Ok(mut app) = window.app.try_borrow_mut() else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
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
