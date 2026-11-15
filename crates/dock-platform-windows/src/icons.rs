//! Icon extraction on a dedicated STA thread.
//!
//! Shell calls can block for hundreds of milliseconds (network paths, damaged
//! shortcuts). The UI thread only submits requests and receives results through
//! an `mpsc` channel plus a posted `WM_ICON_READY` message.

use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK,
    SIIGBF_RESIZETOFIT,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::PCWSTR;

use crate::com::ComGuard;
use crate::error::{PlatformError, Result};
use crate::window::wide;

/// Posted to the dock window whenever an icon result is ready.
pub const WM_ICON_READY: u32 = WM_APP + 1;

/// Premultiplied RGBA icon bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Premultiplied RGBA pixels.
    pub rgba: Vec<u8>,
}

/// A request sent to the icon worker.
#[derive(Debug, Clone)]
pub struct IconRequest {
    /// Dock item id this icon belongs to.
    pub item_id: u32,
    /// Candidate paths in priority order.
    pub paths: Vec<String>,
    /// Desired edge length in pixels.
    pub size: u32,
    /// Configuration generation; stale results are discarded by the caller.
    pub generation: u64,
}

/// The worker's answer.
#[derive(Debug, Clone)]
pub struct IconResult {
    /// Dock item id.
    pub item_id: u32,
    /// Configuration generation echoed from the request.
    pub generation: u64,
    /// Extracted image, when successful.
    pub image: Option<IconImage>,
    /// Path that produced the image.
    pub source: Option<String>,
    /// Failure description.
    pub error: Option<String>,
}

/// Handle to the icon worker thread.
pub struct IconService {
    sender: Sender<IconRequest>,
    receiver: Receiver<IconResult>,
}

impl IconService {
    /// Spawns the worker. `hwnd` is used to wake the UI thread.
    #[must_use]
    pub fn spawn(hwnd: HWND) -> Self {
        let (request_sender, request_receiver) = mpsc::channel::<IconRequest>();
        let (result_sender, result_receiver) = mpsc::channel::<IconResult>();
        let hwnd_value = hwnd.0 as isize;
        let spawn_result = thread::Builder::new()
            .name("glide-dock-icons".to_string())
            .spawn(move || {
                let _com = ComGuard::sta();
                while let Ok(request) = request_receiver.recv() {
                    let result = process_request(&request);
                    let _ = result_sender.send(result);
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_value as *mut c_void)),
                            WM_ICON_READY,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            });
        if let Err(error) = spawn_result {
            tracing::error!(%error, "failed to spawn icon worker");
        }
        Self {
            sender: request_sender,
            receiver: result_receiver,
        }
    }

    /// Queues an icon extraction. Failures are silently ignored when the worker
    /// has already exited; the renderer falls back to placeholder tiles.
    pub fn request(&self, item_id: u32, paths: Vec<String>, size: u32, generation: u64) {
        let _ = self.sender.send(IconRequest {
            item_id,
            paths,
            size,
            generation,
        });
    }

    /// Drains all completed results.
    #[must_use]
    pub fn drain(&self) -> Vec<IconResult> {
        self.receiver.try_iter().collect()
    }
}

fn process_request(request: &IconRequest) -> IconResult {
    for path in &request.paths {
        if path.trim().is_empty() {
            continue;
        }
        match extract_icon(path, request.size) {
            Ok(image) => {
                tracing::debug!(
                    item = request.item_id,
                    path,
                    size = request.size,
                    "icon extracted"
                );
                return IconResult {
                    item_id: request.item_id,
                    generation: request.generation,
                    image: Some(image),
                    source: Some(path.clone()),
                    error: None,
                };
            }
            Err(error) => {
                tracing::debug!(item = request.item_id, path, %error, "icon extraction failed");
            }
        }
    }
    IconResult {
        item_id: request.item_id,
        generation: request.generation,
        image: None,
        source: None,
        error: Some("no candidate path produced an icon".to_string()),
    }
}

/// Extracts a single icon through `IShellItemImageFactory`.
pub fn extract_icon(path: &str, size: u32) -> Result<IconImage> {
    let wide_path = wide(path);
    let item: IShellItemImageFactory = unsafe {
        SHCreateItemFromParsingName(PCWSTR::from_raw(wide_path.as_ptr()), None::<&IBindCtx>)
    }
    .map_err(|error| PlatformError::com(&error))?;

    let flags = SIIGBF(SIIGBF_RESIZETOFIT.0 | SIIGBF_BIGGERSIZEOK.0);
    let hbitmap = unsafe {
        item.GetImage(
            SIZE {
                cx: size as i32,
                cy: size as i32,
            },
            flags,
        )
    }
    .map_err(|error| PlatformError::icon(&error))?;

    let image = hbitmap_to_image(hbitmap);
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbitmap.0));
    }
    image
}

fn hbitmap_to_image(hbitmap: HBITMAP) -> Result<IconImage> {
    let mut bitmap = BITMAP::default();
    let copied = unsafe {
        GetObjectW(
            HGDIOBJ(hbitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bitmap as *mut BITMAP as *mut c_void),
        )
    };
    if copied == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        return Err(PlatformError::Icon(
            "GetObjectW returned an empty bitmap".to_string(),
        ));
    }
    let width = bitmap.bmWidth as u32;
    let height = bitmap.bmHeight as u32;

    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
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
    let mut buffer = vec![0u8; (width as usize) * (height as usize) * 4];
    // A screen DC is required when the shell hands us a device dependent
    // bitmap rather than a DIB section.
    let screen_dc = unsafe { GetDC(None) };
    let scan_lines = unsafe {
        GetDIBits(
            screen_dc,
            hbitmap,
            0,
            height,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        let _ = ReleaseDC(None, screen_dc);
    }
    if scan_lines == 0 {
        return Err(PlatformError::Icon(
            "GetDIBits returned no scan lines".to_string(),
        ));
    }

    Ok(normalize_bgra(&buffer, width, height))
}

/// Converts a top-down BGRA buffer into premultiplied RGBA.
///
/// The shell is inconsistent about premultiplication: some providers return
/// straight alpha, others already premultiplied. A single scan for `rgb > a`
/// tells us which one we received and avoids dark halos around icons.
fn normalize_bgra(bgra: &[u8], width: u32, height: u32) -> IconImage {
    let pixel_count = (width as usize) * (height as usize);
    let has_alpha = bgra
        .chunks_exact(4)
        .take(pixel_count)
        .any(|pixel| pixel[3] != 0);
    let straight = bgra.chunks_exact(4).take(pixel_count).any(|pixel| {
        pixel[3] != 0 && (pixel[0] > pixel[3] || pixel[1] > pixel[3] || pixel[2] > pixel[3])
    });

    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for pixel in bgra.chunks_exact(4).take(pixel_count) {
        let blue = u32::from(pixel[0]);
        let green = u32::from(pixel[1]);
        let red = u32::from(pixel[2]);
        let alpha = if has_alpha { u32::from(pixel[3]) } else { 255 };
        let (red, green, blue) = if straight && alpha < 255 {
            (
                (red * alpha) / 255,
                (green * alpha) / 255,
                (blue * alpha) / 255,
            )
        } else {
            (red, green, blue)
        };
        rgba.push(red as u8);
        rgba.push(green as u8);
        rgba.push(blue as u8);
        rgba.push(alpha as u8);
    }

    IconImage {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_alpha_is_premultiplied() {
        let bgra = [0u8, 0, 255, 128];
        let image = normalize_bgra(&bgra, 1, 1);
        assert_eq!(image.rgba, vec![128, 0, 0, 128]);
    }

    #[test]
    fn premultiplied_alpha_is_preserved() {
        let bgra = [0u8, 0, 100, 128];
        let image = normalize_bgra(&bgra, 1, 1);
        assert_eq!(image.rgba, vec![100, 0, 0, 128]);
    }

    #[test]
    fn missing_alpha_becomes_opaque() {
        let bgra = [10u8, 20, 30, 0];
        let image = normalize_bgra(&bgra, 1, 1);
        assert_eq!(image.rgba, vec![30, 20, 10, 255]);
    }

    #[test]
    fn extracts_system_icon() {
        let icon = extract_icon("C:\\Windows\\System32\\notepad.exe", 64);
        if let Ok(icon) = icon {
            assert!(icon.width > 0);
            assert_eq!(
                icon.rgba.len(),
                icon.width as usize * icon.height as usize * 4
            );
            assert!(icon.rgba.iter().any(|value| *value > 0));
        }
    }

    #[test]
    fn extracts_packaged_app_icon() {
        let _com = ComGuard::sta();
        let Ok(apps) = crate::store::scan_store_apps() else {
            return;
        };
        let Some(app) = apps.first() else {
            return;
        };
        let path = format!("shell:AppsFolder\\{}", app.aumid);
        let icon = extract_icon(&path, 64).expect("packaged icon");
        assert!(icon.width > 0);
        assert!(icon.rgba.iter().any(|value| *value > 0));
    }
}
