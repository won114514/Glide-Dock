//! Windows.UI.Composition host-backdrop window.
//!
//! A borderless `WS_EX_NOREDIRECTIONBITMAP` window hosts a composition visual
//! whose brush is `CreateHostBackdropBrush`. With `DWMWA_USE_HOSTBACKDROPBRUSH`
//! enabled, DWM samples and blurs the desktop behind the window. This gives the
//! dock a live acrylic backdrop without a second process or CPU blur.

use std::ffi::c_void;
use std::ptr;

use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionColorBrush, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual,
    SpriteVisual,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmFlush, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateRectRgn, CreateRoundRectRgn, DeleteObject, EndPaint, HGDIOBJ, PAINTSTRUCT,
    SetWindowRgn,
};
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, MA_NOACTIVATE, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use windows::core::{IUnknown, Interface, PCWSTR};
use windows_numerics::Vector2;

use crate::window::{register_class, wide};

/// Window class used by the composition backdrop.
pub const GLASS_WINDOW_CLASS: &str = "Glide-Dock.GlassBackdrop";

const DWMWA_USE_HOSTBACKDROPBRUSH: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(17);
const DWMWA_BORDER_COLOR: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(34);
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
const WS_EX_NOREDIRECTION: WINDOW_EX_STYLE = WINDOW_EX_STYLE(0x0020_0000);

#[repr(C)]
struct DispatcherQueueOptions {
    size: u32,
    thread_type: i32,
    apartment_type: i32,
}

#[link(name = "CoreMessaging")]
unsafe extern "system" {
    fn CreateDispatcherQueueController(
        options: DispatcherQueueOptions,
        controller: *mut *mut c_void,
    ) -> windows::core::HRESULT;
}

unsafe extern "system" fn glass_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = unsafe { BeginPaint(hwnd, &mut paint) };
            let _ = unsafe { EndPaint(hwnd, &paint) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// A live host-backdrop glass rectangle.
pub struct GlassBackdrop {
    hwnd: HWND,
    _target: DesktopWindowTarget,
    _root: ContainerVisual,
    geometry: CompositionRoundedRectangleGeometry,
    tint: CompositionColorBrush,
    input_bounds: (u32, u32, u32),
    _backdrop: SpriteVisual,
    _tint_visual: SpriteVisual,
    _compositor: Compositor,
    _dispatcher: IUnknown,
}

impl GlassBackdrop {
    /// Creates the glass window. Returns `None` when composition or host
    /// backdrop brushes are unavailable; the caller uses a theme gradient.
    pub fn create(
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        radius: f32,
        dark: bool,
        opacity: f32,
    ) -> Option<Self> {
        unsafe {
            let _ = RoInitialize(RO_INIT_SINGLETHREADED);
        }
        let instance = register_class(GLASS_WINDOW_CLASS, Some(glass_wndproc)).ok()?;
        let class = wide(GLASS_WINDOW_CLASS);
        let title = wide("Glide-Dock Glass");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOREDIRECTION | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR::from_raw(class.as_ptr()),
                PCWSTR::from_raw(title.as_ptr()),
                WS_POPUP,
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
        .ok()?;

        let options = DispatcherQueueOptions {
            size: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            thread_type: 2,    // DQTYPE_THREAD_CURRENT
            apartment_type: 2, // DQTAT_COM_STA
        };
        let mut controller = ptr::null_mut();
        let hr = unsafe { CreateDispatcherQueueController(options, &mut controller) };
        if hr.is_err() || controller.is_null() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            tracing::warn!(?hr, "dispatcher queue unavailable for composition backdrop");
            return None;
        }
        let dispatcher = unsafe { IUnknown::from_raw(controller) };

        let glass = Self::build(hwnd, dispatcher, x, y, width, height, radius, dark, opacity);
        if let Some(glass) = &glass {
            unsafe {
                let _ = ShowWindow(glass.hwnd(), SW_SHOWNOACTIVATE);
            }
        }
        if glass.is_none() {
            tracing::info!("Windows.UI.Composition host backdrop unavailable");
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        glass
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        hwnd: HWND,
        dispatcher: IUnknown,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        radius: f32,
        dark: bool,
        opacity: f32,
    ) -> Option<Self> {
        let compositor = Compositor::new().ok()?;
        let interop: ICompositorDesktopInterop = compositor.cast().ok()?;
        let target = unsafe { interop.CreateDesktopWindowTarget(hwnd, false) }.ok()?;

        let root = compositor.CreateContainerVisual().ok()?;
        root.SetRelativeSizeAdjustment(Vector2::new(1.0, 1.0))
            .ok()?;
        let geometry = compositor.CreateRoundedRectangleGeometry().ok()?;
        geometry
            .SetSize(Vector2::new(width as f32, height as f32))
            .ok()?;
        let corner = (radius * 2.0).max(0.0);
        geometry
            .SetCornerRadius(Vector2::new(corner / 2.0, corner / 2.0))
            .ok()?;
        let clip = compositor.CreateGeometricClipWithGeometry(&geometry).ok()?;
        root.SetClip(&clip).ok()?;

        let backdrop = compositor.CreateSpriteVisual().ok()?;
        backdrop
            .SetRelativeSizeAdjustment(Vector2::new(1.0, 1.0))
            .ok()?;
        let host_brush = compositor.CreateHostBackdropBrush().ok()?;
        backdrop.SetBrush(&host_brush).ok()?;
        root.Children().ok()?.InsertAtTop(&backdrop).ok()?;
        let tint_visual = compositor.CreateSpriteVisual().ok()?;
        tint_visual
            .SetRelativeSizeAdjustment(Vector2::new(1.0, 1.0))
            .ok()?;
        let tint = compositor
            .CreateColorBrushWithColor(tint_color(dark))
            .ok()?;
        tint_visual.SetBrush(&tint).ok()?;
        root.Children().ok()?.InsertAtTop(&tint_visual).ok()?;
        target.SetRoot(&root).ok()?;

        let enable: i32 = 1;
        let attribute = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_HOSTBACKDROPBRUSH,
                (&enable as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            )
        };
        if let Err(error) = attribute {
            tracing::warn!(?error, "DWMWA_USE_HOSTBACKDROPBRUSH failed");
            return None;
        }
        let no_border = DWMWA_COLOR_NONE;
        let border_result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                (&no_border as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        tracing::info!(?border_result, "glass border attribute");

        let mut glass = Self {
            hwnd,
            _target: target,
            _root: root,
            geometry,
            tint,
            input_bounds: (0, 0, 0),
            _backdrop: backdrop,
            _tint_visual: tint_visual,
            _compositor: compositor,
            _dispatcher: dispatcher,
        };
        glass.update(x, y, width, height, radius, dark, opacity);
        Some(glass)
    }

    /// Window handle, for diagnostics.
    #[must_use]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Shows or hides the backdrop without tearing down the composition tree.
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    /// Updates bounds, radius and tint.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        radius: f32,
        dark: bool,
        opacity: f32,
    ) {
        let width = width.max(1);
        let height = height.max(1);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        let _ = self
            .geometry
            .SetSize(Vector2::new(width as f32, height as f32));
        let corner = (radius * 2.0).clamp(0.0, width.min(height) as f32);
        self.clip_input_bounds(width, height, corner.ceil() as u32);
        let _ = self
            .geometry
            .SetCornerRadius(Vector2::new(corner / 2.0, corner / 2.0));
        let _ = self.tint.SetColor(tint_color(dark));
        let _ = self._root.SetOpacity(opacity.clamp(0.0, 1.0));
    }

    // Win32 can test the backdrop before its window procedure runs. Clip
    // its otherwise rectangular corners to the real bar, so a click outside
    // the Dock cannot land on an invisible corner of the glass HWND.
    fn clip_input_bounds(&mut self, width: u32, height: u32, diameter: u32) {
        let key = (width, height, diameter);
        if self.input_bounds == key {
            return;
        }
        unsafe {
            let region = if diameter == 0 {
                CreateRectRgn(0, 0, width as i32, height as i32)
            } else {
                CreateRoundRectRgn(
                    0,
                    0,
                    width as i32,
                    height as i32,
                    diameter as i32,
                    diameter as i32,
                )
            };
            if !region.is_invalid() {
                if SetWindowRgn(self.hwnd, Some(region), true) == 0 {
                    let _ = DeleteObject(HGDIOBJ(region.0));
                } else {
                    self.input_bounds = key;
                }
            }
        }
    }

    /// Forces DWM to process pending composition changes before returning.
    pub fn flush(&self) {
        unsafe {
            let _ = DwmFlush();
        }
    }

    /// Places the backdrop immediately below the layered dock window.
    pub fn place_below(&self, dock: HWND) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(dock),
                0,
                0,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | SWP_NOACTIVATE,
            );
        }
    }
}

impl Drop for GlassBackdrop {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

fn tint_color(dark: bool) -> Color {
    if dark {
        Color {
            A: 112,
            R: 30,
            G: 36,
            B: 60,
        }
    } else {
        Color {
            A: 123,
            R: 244,
            G: 245,
            B: 250,
        }
    }
}
