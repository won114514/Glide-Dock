//! Text rasterization on top of `ab_glyph`.
//!
//! Segoe UI is used for Latin glyphs and Microsoft YaHei as a fallback for CJK
//! (and anything else Segoe UI lacks). Fonts are reference counted so the large
//! CJK collection is loaded only once even when both weights use it.
//! Missing fonts degrade to "no text" instead of panicking.

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use dock_core::Color;
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Font weight selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    /// Regular text.
    Regular,
    /// Semibold text (titles, menu accents).
    Semibold,
}

struct Family {
    faces: Vec<FontArc>,
}

impl Family {
    fn new(faces: Vec<FontArc>) -> Option<Self> {
        if faces.is_empty() {
            None
        } else {
            Some(Self { faces })
        }
    }

    fn face_for(&self, character: char) -> Option<(usize, &FontArc)> {
        self.faces
            .iter()
            .enumerate()
            .find(|(_, font)| font.glyph_id(character).0 != 0)
    }

    fn primary(&self) -> &FontArc {
        &self.faces[0]
    }

    fn metrics(&self, size: f32) -> (f32, f32) {
        let scaled = self.primary().as_scaled(PxScale::from(size));
        (scaled.ascent(), scaled.descent())
    }
}

/// A regular/semibold font pair with CJK fallback.
pub struct FontStack {
    regular: Option<Family>,
    semibold: Option<Family>,
}

impl Default for FontStack {
    fn default() -> Self {
        Self::empty()
    }
}

impl FontStack {
    /// A font stack with no fonts. All text operations become no-ops.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            regular: None,
            semibold: None,
        }
    }

    /// Loads the system UI fonts (Segoe UI + Microsoft YaHei).
    #[must_use]
    pub fn system() -> Self {
        // Windows may be installed on a drive other than C:; resolve the font
        // directory from WINDIR instead of hard-coding it.
        let fonts_dir = std::env::var("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("C:\\Windows"))
            .join("Fonts");
        let font_path = |name: &str| fonts_dir.join(name).to_string_lossy().into_owned();
        let segoe = load_font(&font_path("segoeui.ttf"), 0);
        let segoe_semibold = load_font(&font_path("seguisb.ttf"), 0)
            .or_else(|| load_font(&font_path("segoeuib.ttf"), 0));
        let yahei = load_font(&font_path("msyh.ttc"), 0);

        let regular_faces = [segoe.clone(), yahei.clone()]
            .into_iter()
            .flatten()
            .collect();
        let semibold_faces = [segoe_semibold.or(segoe), yahei]
            .into_iter()
            .flatten()
            .collect();

        Self {
            regular: Family::new(regular_faces),
            semibold: Family::new(semibold_faces),
        }
    }

    /// Returns `true` when at least one font could be loaded.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.regular.is_some()
    }

    fn family(&self, weight: Weight) -> Option<&Family> {
        match weight {
            Weight::Regular => self.regular.as_ref(),
            Weight::Semibold => self.semibold.as_ref().or(self.regular.as_ref()),
        }
    }

    /// Measures the advance width of `text` at `size` pixels.
    #[must_use]
    pub fn measure(&self, text: &str, size: f32, weight: Weight) -> f32 {
        let Some(family) = self.family(weight) else {
            return 0.0;
        };
        let mut width = 0.0;
        let mut previous: Option<(usize, GlyphId)> = None;
        for character in text.chars() {
            let Some((face_index, face)) = family.face_for(character) else {
                continue;
            };
            let scaled = face.as_scaled(PxScale::from(size));
            let glyph = scaled.glyph_id(character);
            if let Some((previous_face, previous_glyph)) = previous {
                if previous_face == face_index {
                    width += scaled.kern(previous_glyph, glyph);
                }
            }
            width += scaled.h_advance(glyph);
            previous = Some((face_index, glyph));
        }
        width
    }

    /// Ascent of the regular face at `size`.
    #[must_use]
    pub fn ascent(&self, size: f32) -> f32 {
        self.regular
            .as_ref()
            .map_or(0.0, |family| family.metrics(size).0)
    }

    /// Descent (negative) of the regular face at `size`.
    #[must_use]
    pub fn descent(&self, size: f32) -> f32 {
        self.regular
            .as_ref()
            .map_or(0.0, |family| family.metrics(size).1)
    }

    /// Baseline that vertically centers a line in a box centered on `center_y`.
    #[must_use]
    pub fn centered_baseline(&self, center_y: f32, size: f32) -> f32 {
        let (ascent, descent) = self
            .regular
            .as_ref()
            .map_or((size * 0.78, -size * 0.22), |family| family.metrics(size));
        center_y + (ascent + descent) / 2.0
    }

    /// Draws `text` with its baseline at `baseline`, starting at `x`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: Color,
        weight: Weight,
    ) {
        let Some(family) = self.family(weight) else {
            return;
        };
        let mut pen = x;
        let mut previous: Option<(usize, GlyphId)> = None;
        for character in text.chars() {
            if character == '\n' || character == '\r' {
                continue;
            }
            let Some((face_index, face)) = family.face_for(character) else {
                continue;
            };
            let scaled = face.as_scaled(PxScale::from(size));
            let glyph_id = scaled.glyph_id(character);
            if let Some((previous_face, previous_glyph)) = previous {
                if previous_face == face_index {
                    pen += scaled.kern(previous_glyph, glyph_id);
                }
            }
            let advance = scaled.h_advance(glyph_id);
            if glyph_id.0 != 0 && color.a > 0 {
                let glyph =
                    glyph_id.with_scale_and_position(PxScale::from(size), point(pen, baseline));
                if let Some(outlined) = scaled.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|dx, dy, coverage| {
                        let px = bounds.min.x as i32 + dx as i32;
                        let py = bounds.min.y as i32 + dy as i32;
                        if px >= 0 && py >= 0 {
                            blend_coverage(pixmap, px as u32, py as u32, color, coverage);
                        }
                    });
                }
            }
            pen += advance;
            previous = Some((face_index, glyph_id));
        }
    }

    /// Draws `text` centered horizontally on `center_x` with its baseline at
    /// `baseline`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_centered(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        center_x: f32,
        baseline: f32,
        size: f32,
        color: Color,
        weight: Weight,
    ) {
        let width = self.measure(text, size, weight);
        self.draw(
            pixmap,
            text,
            center_x - width / 2.0,
            baseline,
            size,
            color,
            weight,
        );
    }

    /// Draws a single line vertically centered in `center_y`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_line_centered(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        center_x: f32,
        center_y: f32,
        size: f32,
        color: Color,
        weight: Weight,
    ) {
        let baseline = self.centered_baseline(center_y, size);
        self.draw_centered(pixmap, text, center_x, baseline, size, color, weight);
    }

    /// Shortens `text` with an ellipsis until it fits into `max_width`.
    #[must_use]
    pub fn ellipsize(&self, text: &str, max_width: f32, size: f32, weight: Weight) -> String {
        if max_width <= 0.0 {
            return String::new();
        }
        if self.measure(text, size, weight) <= max_width {
            return text.to_string();
        }
        let ellipsis = "…";
        let ellipsis_width = self.measure(ellipsis, size, weight);
        let budget = max_width - ellipsis_width;
        if budget <= 0.0 {
            return String::new();
        }
        let mut result = String::new();
        for character in text.chars() {
            let mut candidate = result.clone();
            candidate.push(character);
            if self.measure(&candidate, size, weight) > budget {
                break;
            }
            result = candidate;
        }
        result.push_str(ellipsis);
        result
    }
}

fn load_font(path: &str, index: u32) -> Option<FontArc> {
    let bytes = std::fs::read(path).ok()?;
    match ab_glyph::FontVec::try_from_vec_and_index(bytes, index) {
        Ok(font) => Some(FontArc::new(font)),
        Err(error) => {
            tracing::warn!(path, %error, "font could not be parsed");
            None
        }
    }
}

/// Blends straight `color` with `coverage` (0..=1) over a premultiplied pixel.
fn blend_coverage(pixmap: &mut Pixmap, x: u32, y: u32, color: Color, coverage: f32) {
    if coverage <= 0.0 || x >= pixmap.width() || y >= pixmap.height() {
        return;
    }
    let coverage = coverage.clamp(0.0, 1.0);
    let source_alpha = (f32::from(color.a) / 255.0) * coverage;
    if source_alpha <= 0.0 {
        return;
    }
    let index = (y * pixmap.width() + x) as usize;
    let pixels = pixmap.pixels_mut();
    let Some(destination) = pixels.get(index).copied() else {
        return;
    };
    let dest_alpha = f32::from(destination.alpha()) / 255.0;
    let dest_red = f32::from(destination.red()) / 255.0;
    let dest_green = f32::from(destination.green()) / 255.0;
    let dest_blue = f32::from(destination.blue()) / 255.0;

    let out_alpha = source_alpha + dest_alpha * (1.0 - source_alpha);
    if out_alpha <= 0.0 {
        return;
    }
    let out_red = (f32::from(color.r) / 255.0) * source_alpha + dest_red * (1.0 - source_alpha);
    let out_green = (f32::from(color.g) / 255.0) * source_alpha + dest_green * (1.0 - source_alpha);
    let out_blue = (f32::from(color.b) / 255.0) * source_alpha + dest_blue * (1.0 - source_alpha);

    let to_u8 = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
    let alpha = to_u8(out_alpha);
    let red = to_u8(out_red).min(alpha);
    let green = to_u8(out_green).min(alpha);
    let blue = to_u8(out_blue).min(alpha);
    pixels[index] = PremultipliedColorU8::from_rgba(red, green, blue, alpha)
        .unwrap_or(PremultipliedColorU8::TRANSPARENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_font_stack_is_safe() {
        let fonts = FontStack::empty();
        assert!(!fonts.is_available());
        assert_eq!(fonts.measure("hello", 14.0, Weight::Regular), 0.0);
        let mut pixmap = Pixmap::new(32, 32).expect("pixmap");
        fonts.draw(
            &mut pixmap,
            "hello",
            0.0,
            20.0,
            14.0,
            Color::rgb(255, 255, 255),
            Weight::Regular,
        );
        assert_eq!(fonts.centered_baseline(16.0, 12.0).round(), 19.0);
    }

    #[test]
    fn system_fonts_measure_and_draw() {
        let fonts = FontStack::system();
        if !fonts.is_available() {
            return;
        }
        let width = fonts.measure("Glide-Dock", 16.0, Weight::Semibold);
        assert!(width > 20.0);
        let mut pixmap = Pixmap::new(240, 40).expect("pixmap");
        fonts.draw_line_centered(
            &mut pixmap,
            "Glide-Dock 桌面",
            120.0,
            20.0,
            16.0,
            Color::rgb(255, 255, 255),
            Weight::Semibold,
        );
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0));
    }

    #[test]
    fn cjk_fallback_is_available() {
        let fonts = FontStack::system();
        if !fonts.is_available() {
            return;
        }
        let width = fonts.measure("拖入应用", 14.0, Weight::Regular);
        assert!(width > 20.0, "CJK glyphs should have advances, got {width}");
    }

    #[test]
    fn ellipsize_fits_budget() {
        let fonts = FontStack::system();
        if !fonts.is_available() {
            return;
        }
        let text = "A very long application name that will not fit";
        let max = 120.0;
        let short = fonts.ellipsize(text, max, 13.0, Weight::Regular);
        assert!(short.ends_with('…'));
        assert!(fonts.measure(&short, 13.0, Weight::Regular) <= max + 0.5);
    }
}
