//! Zero-retained-heap Windows DirectWrite text measurement and grayscale alpha
//! rasterization, defaulting to the shared OS font cache with optional scoped
//! loading of a custom font file.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_GLYPH_RUN,
    DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_LINE_METRICS, DWRITE_MATRIX, DWRITE_MEASURING_MODE,
    DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_STRIKETHROUGH, DWRITE_TEXT_METRICS,
    DWRITE_TEXTURE_CLEARTYPE_3x1, DWRITE_UNDERLINE, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteFactory3, IDWriteFactory5, IDWriteFontCollection,
    IDWriteInlineObject, IDWritePixelSnapping_Impl, IDWriteTextLayout, IDWriteTextRenderer,
    IDWriteTextRenderer_Impl,
};
use windows::core::{BOOL, HSTRING, IUnknown, Interface, PCWSTR, Ref, implement, w};

/// Font weight used for DirectWrite text layout and rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextWeight {
    /// Regular weight (400).
    Regular,
    /// Semibold weight (600).
    Semibold,
}

impl TextWeight {
    fn dwrite(self) -> windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT {
        match self {
            Self::Regular => DWRITE_FONT_WEIGHT_NORMAL,
            Self::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
        }
    }
}

/// Grayscale coverage bitmap produced by DirectWrite without retaining fonts.
#[derive(Debug, Clone)]
pub struct TextBitmap {
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Horizontal offset from left edge of the text origin inside `alpha`.
    pub origin_x: i32,
    /// Vertical offset from top edge to typographic baseline inside `alpha`.
    pub baseline_y: i32,
    /// Row-major `width * height` grayscale coverage bytes (`0..=255`).
    pub alpha: Vec<u8>,
}

struct SharedFactory(IDWriteFactory);

// SAFETY: `DWRITE_FACTORY_TYPE_SHARED` returns a thread-safe COM singleton
// documented by DirectWrite for concurrent read/layout usage across threads.
unsafe impl Send for SharedFactory {}
unsafe impl Sync for SharedFactory {}

struct CustomFont {
    collection: IDWriteFontCollection,
    family: Vec<u16>,
}

// SAFETY: `IDWriteFontCollection` built from an immutable font set under the
// shared DirectWrite factory is thread-safe for concurrent format/layout reads,
// and dropping it releases the private collection reference cleanly.
unsafe impl Send for CustomFont {}
unsafe impl Sync for CustomFont {}

/// DirectWrite text measurer and rasterizer for either the OS system font stack
/// or a scoped custom font file.
pub struct TextRenderer {
    custom: Option<CustomFont>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::system()
    }
}

impl TextRenderer {
    /// Creates a renderer using Windows `Segoe UI` and system font fallback.
    #[must_use]
    pub fn system() -> Self {
        Self { custom: None }
    }

    /// Loads `path` into a scoped DirectWrite font collection for `preferred_family`.
    ///
    /// Dropping the returned renderer releases the custom font collection reference.
    #[must_use]
    pub fn from_font_file(path: &Path, preferred_family: &str) -> Option<Self> {
        if !path.is_file() {
            return None;
        }
        let factory = dwrite_factory()?;
        let factory5: IDWriteFactory5 = factory.cast().ok()?;
        let factory3: IDWriteFactory3 = factory.cast().ok()?;
        let path_h = HSTRING::from(path.as_os_str());
        let font_file = unsafe { factory.CreateFontFileReference(&path_h, None) }.ok()?;
        let builder = unsafe { factory5.CreateFontSetBuilder() }.ok()?;
        unsafe { builder.AddFontFile(&font_file) }.ok()?;
        let font_set = unsafe { builder.CreateFontSet() }.ok()?;
        let collection1 = unsafe { factory3.CreateFontCollectionFromFontSet(&font_set) }.ok()?;
        let collection: IDWriteFontCollection = collection1.cast().ok()?;
        let family = resolve_family_utf16(&collection, preferred_family)?;
        Some(Self {
            custom: Some(CustomFont { collection, family }),
        })
    }

    /// Returns `true` when rendering against an explicitly loaded font collection.
    #[must_use]
    pub fn is_custom(&self) -> bool {
        self.custom.is_some()
    }

    /// Measures single-line advance width using Windows DirectWrite.
    #[must_use]
    pub fn measure(&self, text: &str, px_size: f32, weight: TextWeight) -> f32 {
        let Some((_, layout)) = create_layout(self.custom.as_ref(), text, px_size, weight) else {
            return 0.0;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if unsafe { layout.GetMetrics(&mut metrics) }.is_ok() {
            metrics.widthIncludingTrailingWhitespace.max(0.0)
        } else {
            0.0
        }
    }

    /// Rasterizes `text` into a transient grayscale alpha mask with baseline metrics.
    #[must_use]
    pub fn rasterize(&self, text: &str, px_size: f32, weight: TextWeight) -> Option<TextBitmap> {
        let (factory, layout) = create_layout(self.custom.as_ref(), text, px_size, weight)?;

        let mut line = [DWRITE_LINE_METRICS::default()];
        let mut count = 0u32;
        let baseline =
            if unsafe { layout.GetLineMetrics(Some(&mut line), &mut count) }.is_ok() && count > 0 {
                line[0].baseline
            } else {
                px_size * 0.85
            };

        let runs = Rc::new(RefCell::new(Vec::new()));
        let collector = GlyphCollector {
            factory,
            runs: Rc::clone(&runs),
        };
        let renderer: IDWriteTextRenderer = collector.into();
        if unsafe { layout.Draw(None, &renderer, 0.0, 0.0) }.is_err() {
            return None;
        }

        let runs = runs.borrow();
        if runs.is_empty() {
            return None;
        }

        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for run in runs.iter() {
            min_x = min_x.min(run.left);
            min_y = min_y.min(run.top);
            max_x = max_x.max(run.left + run.width as i32);
            max_y = max_y.max(run.top + run.height as i32);
        }
        let width = (max_x - min_x).max(0) as u32;
        let height = (max_y - min_y).max(0) as u32;
        if width == 0 || height == 0 {
            return None;
        }

        let mut alpha = vec![0u8; (width as usize) * (height as usize)];
        for run in runs.iter() {
            let ox = (run.left - min_x) as usize;
            let oy = (run.top - min_y) as usize;
            for sy in 0..run.height as usize {
                let dst_row = (oy + sy) * (width as usize);
                let src_row = sy * (run.width as usize);
                for sx in 0..run.width as usize {
                    let s = run.alpha[src_row + sx];
                    if s == 0 {
                        continue;
                    }
                    let idx = dst_row + ox + sx;
                    let d = alpha[idx];
                    alpha[idx] =
                        s.saturating_add(((u16::from(d) * u16::from(255 - s) + 127) / 255) as u8);
                }
            }
        }

        Some(TextBitmap {
            width,
            height,
            origin_x: -min_x,
            baseline_y: baseline.round() as i32 - min_y,
            alpha,
        })
    }
}

fn resolve_family_utf16(collection: &IDWriteFontCollection, preferred: &str) -> Option<Vec<u16>> {
    let preferred_utf16: Vec<u16> = preferred.encode_utf16().chain(std::iter::once(0)).collect();
    let mut index = 0u32;
    let mut exists = BOOL(0);
    if unsafe {
        collection.FindFamilyName(PCWSTR(preferred_utf16.as_ptr()), &mut index, &mut exists)
    }
    .is_ok()
        && exists.as_bool()
    {
        return Some(preferred_utf16);
    }

    if unsafe { collection.GetFontFamilyCount() } == 0 {
        return None;
    }
    let family = unsafe { collection.GetFontFamily(0) }.ok()?;
    let names = unsafe { family.GetFamilyNames() }.ok()?;
    if unsafe { names.GetCount() } == 0 {
        return None;
    }
    let len = unsafe { names.GetStringLength(0) }.ok()?;
    let mut buffer = vec![0u16; (len as usize) + 1];
    unsafe { names.GetString(0, &mut buffer) }.ok()?;
    Some(buffer)
}

fn dwrite_factory() -> Option<&'static IDWriteFactory> {
    static FACTORY: OnceLock<Option<SharedFactory>> = OnceLock::new();
    FACTORY
        .get_or_init(|| {
            unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) }
                .ok()
                .map(SharedFactory)
        })
        .as_ref()
        .map(|wrapper| &wrapper.0)
}

fn create_layout(
    custom: Option<&CustomFont>,
    text: &str,
    px_size: f32,
    weight: TextWeight,
) -> Option<(IDWriteFactory, IDWriteTextLayout)> {
    if text.is_empty() || px_size <= 0.0 {
        return None;
    }
    let factory = dwrite_factory()?.clone();
    let format = unsafe {
        match custom {
            Some(custom) => factory.CreateTextFormat(
                PCWSTR(custom.family.as_ptr()),
                &custom.collection,
                weight.dwrite(),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px_size,
                w!("zh-CN"),
            ),
            None => factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                weight.dwrite(),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px_size,
                w!("zh-CN"),
            ),
        }
        .ok()?
    };
    unsafe {
        let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
    }
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let layout = unsafe {
        factory
            .CreateTextLayout(&utf16, &format, 100_000.0, 100_000.0)
            .ok()?
    };
    Some((factory, layout))
}

struct GlyphRunMask {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    alpha: Vec<u8>,
}

#[implement(IDWriteTextRenderer)]
struct GlyphCollector {
    factory: IDWriteFactory,
    runs: Rc<RefCell<Vec<GlyphRunMask>>>,
}

impl IDWritePixelSnapping_Impl for GlyphCollector_Impl {
    fn IsPixelSnappingDisabled(&self, _context: *const c_void) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }

    fn GetCurrentTransform(
        &self,
        _context: *const c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> windows::core::Result<()> {
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m12: 0.0,
                m21: 0.0,
                m22: 1.0,
                dx: 0.0,
                dy: 0.0,
            };
        }
        Ok(())
    }

    fn GetPixelsPerDip(&self, _context: *const c_void) -> windows::core::Result<f32> {
        Ok(1.0)
    }
}

impl IDWriteTextRenderer_Impl for GlyphCollector_Impl {
    fn DrawGlyphRun(
        &self,
        _context: *const c_void,
        baseline_origin_x: f32,
        baseline_origin_y: f32,
        measuring_mode: DWRITE_MEASURING_MODE,
        glyph_run: *const DWRITE_GLYPH_RUN,
        _glyph_run_description: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _client_drawing_effect: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        let analysis = unsafe {
            self.factory.CreateGlyphRunAnalysis(
                glyph_run,
                1.0,
                None,
                DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                measuring_mode,
                baseline_origin_x,
                baseline_origin_y,
            )?
        };
        let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1)? };
        let w = bounds.right - bounds.left;
        let h = bounds.bottom - bounds.top;
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let mut rgb = vec![0u8; (w as usize) * (h as usize) * 3];
        unsafe {
            analysis.CreateAlphaTexture(DWRITE_TEXTURE_CLEARTYPE_3x1, &bounds, &mut rgb)?;
        }
        let mut alpha = Vec::with_capacity((w as usize) * (h as usize));
        for &[r, g, b] in rgb.as_chunks::<3>().0 {
            let cov = ((u16::from(r) * 77 + u16::from(g) * 150 + u16::from(b) * 29) >> 8) as u8;
            alpha.push(cov);
        }
        self.runs.borrow_mut().push(GlyphRunMask {
            left: bounds.left,
            top: bounds.top,
            width: w as u32,
            height: h as u32,
            alpha,
        });
        Ok(())
    }

    fn DrawUnderline(
        &self,
        _context: *const c_void,
        _baseline_origin_x: f32,
        _baseline_origin_y: f32,
        _underline: *const DWRITE_UNDERLINE,
        _client_drawing_effect: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn DrawStrikethrough(
        &self,
        _context: *const c_void,
        _baseline_origin_x: f32,
        _baseline_origin_y: f32,
        _strikethrough: *const DWRITE_STRIKETHROUGH,
        _client_drawing_effect: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn DrawInlineObject(
        &self,
        _context: *const c_void,
        _origin_x: f32,
        _origin_y: f32,
        _inline_object: Ref<'_, IDWriteInlineObject>,
        _is_sideways: BOOL,
        _is_right_to_left: BOOL,
        _client_drawing_effect: Ref<'_, IUnknown>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Measures single-line advance width using Windows DirectWrite system font fallback.
#[must_use]
pub fn measure_text(text: &str, px_size: f32, weight: TextWeight) -> f32 {
    TextRenderer::system().measure(text, px_size, weight)
}

/// Computes a visually centered baseline coordinate around `center_y`.
#[must_use]
pub fn centered_baseline(center_y: f32, px_size: f32) -> f32 {
    center_y + px_size * 0.35
}

/// Rasterizes `text` into a transient grayscale alpha mask with baseline metrics.
#[must_use]
pub fn rasterize_text(text: &str, px_size: f32, weight: TextWeight) -> Option<TextBitmap> {
    TextRenderer::system().rasterize(text, px_size, weight)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_rasterizes_latin_and_cjk() {
        let width_ascii = measure_text("Glide-Dock", 14.0, TextWeight::Regular);
        let width_cjk = measure_text("打开文件位置", 14.0, TextWeight::Regular);
        assert!(width_ascii > 30.0);
        assert!(width_cjk > 40.0);

        let bmp = rasterize_text("退出 Glide-Dock", 14.0, TextWeight::Semibold).expect("raster");
        assert!(bmp.width > 20 && bmp.height > 10);
        assert!(bmp.alpha.iter().any(|&a| a > 180));
        assert!(bmp.alpha.contains(&0));
    }

    #[test]
    fn empty_text_returns_zero_or_none() {
        assert_eq!(measure_text("", 14.0, TextWeight::Regular), 0.0);
        assert!(rasterize_text("", 14.0, TextWeight::Regular).is_none());
    }

    #[test]
    fn loads_and_rasterizes_custom_ttf_font() {
        let font_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts/NotoSansSC-VF.ttf");
        assert!(font_path.is_file(), "bundled font asset should exist");

        let renderer =
            TextRenderer::from_font_file(&font_path, "Noto Sans SC").expect("custom font loads");
        assert!(renderer.is_custom());
        assert!(renderer.measure("退出 Glide-Dock", 14.0, TextWeight::Semibold) > 50.0);
        let bmp = renderer
            .rasterize("设置...", 14.0, TextWeight::Regular)
            .expect("custom font rasterizes");
        assert!(bmp.width > 20 && bmp.height > 10);
        assert!(bmp.alpha.iter().any(|&a| a > 180));

        assert!(
            TextRenderer::from_font_file(&font_path.with_file_name("missing.ttf"), "Noto Sans SC")
                .is_none()
        );
    }
}
