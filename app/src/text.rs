//! Dock text backend adapter backed by Windows DirectWrite.
//!
//! Defaults to `DWRITE_FACTORY_TYPE_SHARED` (zero heap font retention) and can
//! optionally load the bundled `NotoSansSC-VF.ttf` on user request.

use std::path::{Path, PathBuf};

use dock_core::{Color as ThemeColor, DockFont};
use dock_platform_windows::{TextRenderer, TextWeight, centered_baseline};
use dock_render::{TextBackend, Weight};
use tiny_skia::{Pixmap, PremultipliedColorU8};

pub(crate) struct NativeText {
    inner: TextRenderer,
}

impl NativeText {
    pub(crate) fn system() -> Self {
        Self {
            inner: TextRenderer::system(),
        }
    }

    pub(crate) fn for_dock_font(font: DockFont) -> Self {
        match font {
            DockFont::System => Self::system(),
            DockFont::NotoSansSc => {
                if let Some(path) = bundled_font_path()
                    && let Some(renderer) = TextRenderer::from_font_file(&path, "Noto Sans SC")
                {
                    return Self { inner: renderer };
                }
                tracing::warn!(
                    "bundled NotoSansSC-VF.ttf unavailable; falling back to Windows system font"
                );
                Self::system()
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn uses_custom_font(&self) -> bool {
        self.inner.is_custom()
    }
}

fn bundled_font_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(directory) = exe.parent()
    {
        let installed = directory.join("assets/fonts/NotoSansSC-VF.ttf");
        if installed.is_file() {
            return Some(installed);
        }
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/fonts/NotoSansSC-VF.ttf");
    workspace.is_file().then_some(workspace)
}

fn map_weight(weight: Weight) -> TextWeight {
    match weight {
        Weight::Regular => TextWeight::Regular,
        Weight::Semibold => TextWeight::Semibold,
    }
}

/// Applies DirectWrite perceptual gamma adjustment so dark text on light surfaces
/// keeps crisp Windows 11 stem thickness when composited into sRGB pixmaps.
fn adjust_coverage(cov: u8, color: ThemeColor) -> u8 {
    if cov == 0 || cov == 255 {
        return cov;
    }
    let luma = (u16::from(color.r) * 77 + u16::from(color.g) * 150 + u16::from(color.b) * 29) >> 8;
    let gamma = if luma < 128 { 0.72_f32 } else { 0.88_f32 };
    ((f32::from(cov) / 255.0).powf(gamma) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

impl TextBackend for NativeText {
    fn is_available(&self) -> bool {
        true
    }

    fn measure(&self, text: &str, size: f32, weight: Weight) -> f32 {
        self.inner.measure(text, size, map_weight(weight))
    }

    fn centered_baseline(&self, center_y: f32, size: f32) -> f32 {
        centered_baseline(center_y, size)
    }

    fn draw(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: ThemeColor,
        weight: Weight,
    ) {
        if text.is_empty() || color.a == 0 {
            return;
        }
        let Some(bitmap) = self.inner.rasterize(text, size, map_weight(weight)) else {
            return;
        };
        let dst_x0 = x.round() as i32 - bitmap.origin_x;
        let dst_y0 = baseline.round() as i32 - bitmap.baseline_y;
        let pm_w = pixmap.width() as i32;
        let pm_h = pixmap.height() as i32;
        let bmp_w = bitmap.width as usize;
        let pixels = pixmap.pixels_mut();

        for sy in 0..bitmap.height as i32 {
            let dy = dst_y0 + sy;
            if dy < 0 || dy >= pm_h {
                continue;
            }
            let row_offset = (sy as usize) * bmp_w;
            let dst_row = (dy as usize) * (pm_w as usize);
            for sx in 0..bitmap.width as i32 {
                let dx = dst_x0 + sx;
                if dx < 0 || dx >= pm_w {
                    continue;
                }
                let raw_cov = bitmap.alpha[row_offset + (sx as usize)];
                if raw_cov == 0 {
                    continue;
                }
                let cov = adjust_coverage(raw_cov, color);
                let src_a = ((u16::from(color.a) * u16::from(cov) + 127) / 255) as u8;
                if src_a == 0 {
                    continue;
                }
                let src_r = ((u16::from(color.r) * u16::from(src_a) + 127) / 255) as u8;
                let src_g = ((u16::from(color.g) * u16::from(src_a) + 127) / 255) as u8;
                let src_b = ((u16::from(color.b) * u16::from(src_a) + 127) / 255) as u8;

                let inv = u16::from(255 - src_a);
                let idx = dst_row + (dx as usize);
                let dst = pixels[idx];
                let out_a =
                    src_a.saturating_add(((u16::from(dst.alpha()) * inv + 127) / 255) as u8);
                let out_r = src_r
                    .saturating_add(((u16::from(dst.red()) * inv + 127) / 255) as u8)
                    .min(out_a);
                let out_g = src_g
                    .saturating_add(((u16::from(dst.green()) * inv + 127) / 255) as u8)
                    .min(out_a);
                let out_b = src_b
                    .saturating_add(((u16::from(dst.blue()) * inv + 127) / 255) as u8)
                    .min(out_a);
                if let Some(mixed) = PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a) {
                    pixels[idx] = mixed;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dock_core::state::{MenuAction, MenuEntry, MenuIcon};
    use dock_core::theme::Palette;
    use dock_render::{MenuScene, Renderer};

    use super::*;

    #[test]
    fn renders_light_menu_with_directwrite_text() {
        let system = NativeText::system();
        assert!(!system.uses_custom_font());
        let noto = NativeText::for_dock_font(DockFont::NotoSansSc);
        assert!(noto.uses_custom_font());

        let mut renderer = Renderer::with_text_backend(Arc::new(noto));
        renderer.trim_caches();
        let entries = vec![
            MenuEntry::action("设置...", MenuAction::OpenSettings, MenuIcon::Settings),
            MenuEntry::action("添加开始菜单", MenuAction::AddStartWidget, MenuIcon::Open),
            MenuEntry::action("添加分隔线", MenuAction::AddSeparator, MenuIcon::Edit),
            MenuEntry::action("添加商店应用...", MenuAction::AddStoreApp, MenuIcon::Open),
            MenuEntry::Separator,
            MenuEntry::danger("退出 Glide-Dock", MenuAction::Quit, MenuIcon::Power),
        ];
        let layout = renderer.menu_layout(&entries, 1.25);
        let frame = renderer
            .render_menu(&MenuScene {
                entries: &entries,
                layout: &layout,
                hovered: Some(2),
                palette: Palette::light(),
            })
            .expect("light menu should render");
        assert!(frame.pixmap.width() > 200);
        assert!(frame.pixmap.pixels().iter().any(|p| p.alpha() > 240));
        if let Ok(out) = std::env::var("GLIDE_LIGHT_MENU_PREVIEW") {
            frame.pixmap.save_png(out).expect("save light menu preview");
        }
    }
}
