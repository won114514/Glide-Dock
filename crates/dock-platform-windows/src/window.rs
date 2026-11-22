//! Layered popup window creation and small coordinate helpers.

use windows::Win32::Foundation::{HINSTANCE, HWND, POINT};
use windows::Win32::Graphics::Gdi::{ClientToScreen, HBRUSH, ScreenToClient};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, FindWindowW, GW_HWNDPREV, GWL_EXSTYLE, GetCursorPos,
    GetWindow, GetWindowLongPtrW, HICON, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, LoadCursorW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WNDCLASSW, WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::error::{PlatformError, Result};

/// Registers a window class. Returns the module instance to use when creating
/// windows of that class.
pub fn register_class(name: &str, wndproc: WNDPROC) -> Result<HINSTANCE> {
    let module = unsafe { GetModuleHandleW(None) }.map_err(|error| PlatformError::win32(&error))?;
    let instance = HINSTANCE(module.0);
    let class_name = wide(name);
    let cursor =
        unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(|error| PlatformError::win32(&error))?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON::default(),
        hCursor: cursor,
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        // A second registration of the same class is harmless.
        let last = unsafe { windows::Win32::Foundation::GetLastError() };
        if last.0 != 1410 {
            return Err(PlatformError::Window(format!(
                "RegisterClassW failed for {name} (error {})",
                last.0
            )));
        }
    }
    Ok(instance)
}

/// Creates a popup window with per-pixel alpha. The window starts hidden.
pub fn create_layered_popup(
    instance: HINSTANCE,
    class_name: &str,
    title: &str,
    width: u32,
    height: u32,
) -> Result<HWND> {
    let class = wide(class_name);
    let title = wide(title);
    let ex_style: WINDOW_EX_STYLE = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let style: WINDOW_STYLE = WS_POPUP;
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            style,
            0,
            0,
            width.max(1) as i32,
            height.max(1) as i32,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| PlatformError::win32(&error))?;

    Ok(hwnd)
}

/// Moves and resizes a layered window without changing its Z order or activation.
pub fn set_bounds(hwnd: HWND, x: i32, y: i32, width: u32, height: u32, show: bool) {
    let mut flags = SWP_NOACTIVATE | SWP_NOZORDER;
    if show {
        flags |= SWP_SHOWWINDOW;
    }
    if let Err(error) = unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width.max(1) as i32,
            height.max(1) as i32,
            flags,
        )
    } {
        tracing::warn!(%error, "SetWindowPos failed");
    }
}

/// Shows the window without stealing focus.
pub fn show_no_activate(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    place_above_desktop(hwnd);
}

/// Raises the window above normal applications without activating it.
pub fn set_topmost(hwnd: HWND) {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let topmost = WS_EX_TOPMOST.0 as isize;
    if style & topmost == 0 {
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | topmost);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

/// Places the window immediately above the desktop, below normal applications.
pub fn place_above_desktop(hwnd: HWND) {
    clear_topmost(hwnd);

    let class = wide("Progman");
    if let Ok(desktop) = unsafe { FindWindowW(PCWSTR::from_raw(class.as_ptr()), PCWSTR::null()) } {
        let mut current = unsafe { GetWindow(desktop, GW_HWNDPREV) }.ok();
        for _ in 0..256 {
            let Some(candidate) = current else {
                break;
            };
            if candidate != hwnd && !is_topmost(candidate) {
                match unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(candidate),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                } {
                    Ok(()) => return,
                    Err(error) => {
                        tracing::debug!(%error, "z-order candidate rejected");
                    }
                }
            }
            current = unsafe { GetWindow(candidate, GW_HWNDPREV) }.ok();
        }
    }

    tracing::debug!("no permitted desktop z-order anchor; keeping normal window order");
}

fn clear_topmost(hwnd: HWND) {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let topmost = WS_EX_TOPMOST.0 as isize;
    if style & topmost != 0 {
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !topmost);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

fn is_topmost(hwnd: HWND) -> bool {
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    style & WS_EX_TOPMOST.0 as isize != 0
}

/// Hides the window.
pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// Converts screen coordinates to client coordinates.
#[must_use]
pub fn screen_to_client(hwnd: HWND, point: POINT) -> POINT {
    let mut converted = point;
    unsafe {
        let _ = ScreenToClient(hwnd, &mut converted);
    }
    converted
}

/// Converts client coordinates to screen coordinates.
#[must_use]
pub fn client_to_screen(hwnd: HWND, point: POINT) -> POINT {
    let mut converted = point;
    unsafe {
        let _ = ClientToScreen(hwnd, &mut converted);
    }
    converted
}

/// Reads the current cursor position in screen coordinates.
#[must_use]
pub fn cursor_position() -> Option<POINT> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        Some(point)
    } else {
        None
    }
}

/// Captures the mouse so drags can continue outside the dock window.
pub fn capture_mouse(hwnd: HWND) {
    unsafe {
        SetCapture(hwnd);
    }
}

/// Releases a previous capture.
pub fn release_mouse() {
    unsafe {
        let _ = ReleaseCapture();
    }
}

/// Converts a Rust string to a NUL terminated UTF-16 buffer.
#[must_use]
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Returns the last Win32 error code.
#[must_use]
pub fn last_error() -> u32 {
    unsafe { windows::Win32::Foundation::GetLastError().0 }
}
