//! Monitor work areas and dock placement.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::core::PCWSTR;

/// A monitor work area in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge.
    pub right: i32,
    /// Bottom edge.
    pub bottom: i32,
}

impl WorkArea {
    /// Width in pixels.
    #[must_use]
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
}

/// Monitor work area plus DPI scale for a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorMetrics {
    /// Work area (taskbar excluded).
    pub work: WorkArea,
    /// DPI scale, e.g. `1.25` for 125%.
    pub dpi_scale: f32,
    /// Best-effort monitor refresh rate in hertz.
    pub refresh_hz: u32,
}

/// Reads the work area and DPI for the monitor containing `hwnd`.
#[must_use]
pub fn metrics_for_window(hwnd: HWND) -> MonitorMetrics {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    let mut work = WorkArea {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.0 != 0 {
        work = WorkArea {
            left: info.rcWork.left,
            top: info.rcWork.top,
            right: info.rcWork.right,
            bottom: info.rcWork.bottom,
        };
    }
    // A desktop-band window may keep its old window DPI after the monitor
    // scale changes. The custom layout runs in physical pixels, so read the
    // scale from the monitor itself and fall back to the window if needed.
    let mut dpi_x = 0;
    let mut dpi_y = 0;
    let measured = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let dpi = if measured.is_ok() && dpi_x != 0 {
        dpi_x
    } else {
        unsafe { GetDpiForWindow(hwnd) }
    };
    let dpi_scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
    MonitorMetrics {
        work,
        dpi_scale,
        refresh_hz: monitor_refresh_hz(monitor).unwrap_or_else(composition_refresh_hz),
    }
}

fn monitor_refresh_hz(monitor: HMONITOR) -> Option<u32> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
        return None;
    }
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    };
    // A frequency of zero or one represents the hardware default.
    (ok.as_bool() && mode.dmDisplayFrequency > 1).then_some(mode.dmDisplayFrequency)
}

fn composition_refresh_hz() -> u32 {
    let mut timing = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..DWM_TIMING_INFO::default()
    };
    let ok = unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing) }.is_ok();
    if ok && timing.rateCompose.uiDenominator != 0 {
        let hz =
            f64::from(timing.rateCompose.uiNumerator) / f64::from(timing.rateCompose.uiDenominator);
        if hz.is_finite() && hz >= 20.0 {
            return hz.round() as u32;
        }
    }
    60
}

/// Reads the work area of the primary monitor.
#[must_use]
pub fn primary_work_area() -> WorkArea {
    let point = POINT { x: 0, y: 0 };
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.0 != 0 {
        WorkArea {
            left: info.rcWork.left,
            top: info.rcWork.top,
            right: info.rcWork.right,
            bottom: info.rcWork.bottom,
        }
    } else {
        WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }
}

/// Computes the bottom-centered window position for the dock.
#[must_use]
pub fn place_dock(
    work: WorkArea,
    window_width: u32,
    window_height: u32,
    edge_margin: f32,
    offset: f32,
) -> (i32, i32) {
    let width = window_width as i32;
    let height = window_height as i32;
    let centered = work.left + (work.width() - width) / 2;
    let x = centered + offset.round() as i32;
    let x = x.clamp(work.left, (work.right - width).max(work.left));
    let margin = edge_margin.round() as i32;
    let y = work.bottom - margin - height;
    (x, y)
}

/// Clamps a window rectangle into the work area.
#[must_use]
pub fn clamp_window(x: i32, y: i32, width: u32, height: u32, work: WorkArea) -> (i32, i32) {
    let width = width as i32;
    let height = height as i32;
    let max_x = (work.right - width).max(work.left);
    let max_y = (work.bottom - height).max(work.top);
    (x.clamp(work.left, max_x), y.clamp(work.top, max_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> WorkArea {
        WorkArea {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        }
    }

    #[test]
    fn dock_is_horizontally_centered() {
        let (x, y) = place_dock(work(), 800, 120, 10.0, 0.0);
        assert_eq!(x, (1920 - 800) / 2);
        assert_eq!(y, 1040 - 10 - 120);
    }

    #[test]
    fn negative_offset_is_clamped() {
        let (x, _) = place_dock(work(), 800, 120, 10.0, -1000.0);
        assert_eq!(x, 0);
    }

    #[test]
    fn menu_window_clamps_inside_work_area() {
        let (x, y) = clamp_window(-50, -50, 200, 100, work());
        assert_eq!((x, y), (0, 0));
        let (x, y) = clamp_window(3000, 3000, 200, 100, work());
        assert_eq!((x, y), (1720, 940));
    }
}
