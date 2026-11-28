//! 32bpp top-down DIB surface presented through `UpdateLayeredWindow`.

use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ULW_ALPHA, UPDATELAYEREDWINDOWINFO, UpdateLayeredWindow, UpdateLayeredWindowIndirect,
};

use crate::error::{PlatformError, Result};

/// A reusable layered-window surface.
pub struct LayeredSurface {
    hwnd: HWND,
    memory_dc: HDC,
    bitmap: Option<HBITMAP>,
    previous_bitmap: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
    input_mask_valid: bool,
}

// The surface is only used from the UI thread.
impl LayeredSurface {
    /// Creates an empty surface for a window.
    pub fn new(hwnd: HWND) -> Result<Self> {
        let memory_dc = unsafe { CreateCompatibleDC(None) };
        if memory_dc.is_invalid() {
            return Err(PlatformError::Win32(
                "CreateCompatibleDC returned a null DC".to_string(),
            ));
        }
        Ok(Self {
            hwnd,
            memory_dc,
            bitmap: None,
            previous_bitmap: HGDIOBJ::default(),
            bits: ptr::null_mut(),
            width: 0,
            height: 0,
            input_mask_valid: false,
        })
    }

    /// Current surface width.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Current surface height.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Ensures the DIB matches the requested size.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        if self.bitmap.is_some() && self.width == width && self.height == height {
            return Ok(());
        }
        self.release_bitmap();

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: width * height * 4,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                Some(self.memory_dc),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
        }
        .map_err(|error| PlatformError::win32(&error))?;
        if bits.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            return Err(PlatformError::Win32(
                "CreateDIBSection returned no pixels".into(),
            ));
        }
        let previous = unsafe { SelectObject(self.memory_dc, HGDIOBJ(bitmap.0)) };
        if previous.is_invalid() || previous.0 as isize == -1 {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            return Err(PlatformError::Win32(
                "SelectObject rejected the DIB section".to_string(),
            ));
        }

        self.bitmap = Some(bitmap);
        self.previous_bitmap = previous;
        self.bits = bits.cast::<u8>();
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Copies a premultiplied RGBA pixmap into the DIB and presents it.
    pub fn present(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
        self.present_alpha(width, height, rgba, 255)
    }

    /// Copies a premultiplied RGBA pixmap into the DIB and presents it with
    /// an overall window alpha (`0..=255`).
    pub fn present_alpha(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        opacity: u8,
    ) -> Result<()> {
        // Validate before resize: a truncated buffer must not destroy the
        // current, still-valid DIB.
        let expected = (width.max(1) as usize)
            .saturating_mul(height.max(1) as usize)
            .saturating_mul(4);
        if rgba.len() < expected {
            return Err(PlatformError::Win32(format!(
                "surface buffer mismatch: need {expected} bytes, got {}",
                rgba.len()
            )));
        }
        self.resize(width, height)?;
        if self.bits.is_null() {
            return Err(PlatformError::Win32("surface has no DIB bits".to_string()));
        }

        self.input_mask_valid = false;
        // tiny-skia stores RGBA; a 32bpp Windows DIB expects BGRA.
        let destination = unsafe { std::slice::from_raw_parts_mut(self.bits, expected) };
        let (source_pixels, _) = rgba.as_chunks::<4>();
        let (target_pixels, _) = destination.as_chunks_mut::<4>();
        for (source, target) in source_pixels.iter().zip(target_pixels.iter_mut()) {
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
            target[3] = source[3];
        }

        let size = SIZE {
            cx: self.width as i32,
            cy: self.height as i32,
        };
        let source_point = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                None,
                Some(&size as *const SIZE),
                Some(self.memory_dc),
                Some(&source_point as *const POINT),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
        .map_err(|error| PlatformError::win32(&error))
    }

    /// Updates the hit-test DIB beneath a DirectComposition visual. The edit
    /// returns the dirty bounds, or `None` when no pixels changed.
    /// It receives `true` when the DIB was cleared and the entire mask must be
    /// rebuilt, including after a CPU frame overwrote it at the same size.
    pub fn update_input_mask(
        &mut self,
        width: u32,
        height: u32,
        edit: impl FnOnce(&mut [u32], bool) -> Option<RECT>,
    ) -> Result<()> {
        self.resize(width, height)?;
        let rebuild = !self.input_mask_valid;
        let len = self.width as usize * self.height as usize;
        // 32bpp DIB scanlines are DWORD-aligned and have no extra padding.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits.cast::<u32>(), len) };
        if rebuild {
            pixels.fill(0);
        }
        let changed = edit(pixels, rebuild);
        if !rebuild && changed.is_none() {
            return Ok(());
        }
        let size = SIZE {
            cx: self.width as i32,
            cy: self.height as i32,
        };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
            ..Default::default()
        };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            psize: &size,
            hdcSrc: self.memory_dc,
            pptSrc: &src,
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: if rebuild {
                ptr::null()
            } else {
                changed.as_ref().map_or(ptr::null(), ptr::from_ref)
            },
            ..Default::default()
        };
        let result = unsafe { UpdateLayeredWindowIndirect(self.hwnd, &info) }
            .ok()
            .map_err(|error| PlatformError::win32(&error));
        self.input_mask_valid = result.is_ok();
        result
    }

    fn release_bitmap(&mut self) {
        if let Some(bitmap) = self.bitmap.take() {
            if !self.previous_bitmap.is_invalid() {
                unsafe {
                    SelectObject(self.memory_dc, self.previous_bitmap);
                }
            }
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
        }
        self.previous_bitmap = HGDIOBJ::default();
        self.bits = ptr::null_mut();
        self.width = 0;
        self.height = 0;
        self.input_mask_valid = false;
    }
}

impl Drop for LayeredSurface {
    fn drop(&mut self) {
        self.release_bitmap();
        if !self.memory_dc.is_invalid() {
            unsafe {
                let _ = DeleteDC(self.memory_dc);
            }
        }
    }
}
