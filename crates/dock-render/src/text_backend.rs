//! Backend-agnostic text measurement and rasterization contract for `dock-render`.

use crate::text::{FontStack, Weight};
use dock_core::Color as ThemeColor;
use tiny_skia::Pixmap;

/// Abstracts font lookup, text measurement, and grayscale glyph drawing.
///
/// Desktop builds can inject a Win32 GDI backend so `glide-dock.exe` avoids
/// loading megabytes of raw TrueType font files into heap memory.
pub trait TextBackend: Send + Sync {
    /// Returns `true` when at least one font face can render glyphs.
    fn is_available(&self) -> bool;

    /// Measures the advance width of a single line of text in pixels.
    fn measure(&self, text: &str, size: f32, weight: Weight) -> f32;

    /// Returns a visually centered baseline coordinate around `center_y`.
    fn centered_baseline(&self, center_y: f32, size: f32) -> f32;

    /// Draws `text` at `(x, baseline)` onto `pixmap`.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: ThemeColor,
        weight: Weight,
    );

    /// Truncates `text` with `...` when wider than `max_width`.
    fn ellipsize(&self, text: &str, max_width: f32, size: f32, weight: Weight) -> String {
        ellipsize_text(self, text, max_width, size, weight)
    }

    /// Draws a single centered line around `(center_x, center_y)`.
    #[allow(clippy::too_many_arguments)]
    fn draw_line_centered(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        center_x: f32,
        center_y: f32,
        size: f32,
        color: ThemeColor,
        weight: Weight,
    ) {
        if !self.is_available() || text.is_empty() || color.a == 0 {
            return;
        }
        let width = self.measure(text, size, weight);
        let baseline = self.centered_baseline(center_y, size);
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
}

/// Default text truncation helper shared across backends.
pub fn ellipsize_text<T: TextBackend + ?Sized>(
    backend: &T,
    text: &str,
    max_width: f32,
    size: f32,
    weight: Weight,
) -> String {
    if !backend.is_available()
        || text.is_empty()
        || backend.measure(text, size, weight) <= max_width
    {
        return text.to_string();
    }
    let ellipsis = "...";
    let ellipsis_width = backend.measure(ellipsis, size, weight);
    if ellipsis_width >= max_width {
        return ellipsis.to_string();
    }

    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    let mut low = 0usize;
    let mut high = boundaries.len();
    let mut candidate = String::new();
    while low < high {
        let mid = low + (high - low) / 2;
        candidate.clear();
        candidate.push_str(&text[..boundaries[mid]]);
        candidate.push_str(ellipsis);
        if backend.measure(&candidate, size, weight) <= max_width {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let cutoff = low.saturating_sub(1);
    let mut result = text[..boundaries.get(cutoff).copied().unwrap_or(0)].to_string();
    result.push_str(ellipsis);
    result
}

impl TextBackend for FontStack {
    fn is_available(&self) -> bool {
        FontStack::is_available(self)
    }

    fn measure(&self, text: &str, size: f32, weight: Weight) -> f32 {
        FontStack::measure(self, text, size, weight)
    }

    fn centered_baseline(&self, center_y: f32, size: f32) -> f32 {
        FontStack::centered_baseline(self, center_y, size)
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
        FontStack::draw(self, pixmap, text, x, baseline, size, color, weight);
    }

    fn ellipsize(&self, text: &str, max_width: f32, size: f32, weight: Weight) -> String {
        FontStack::ellipsize(self, text, max_width, size, weight)
    }

    fn draw_line_centered(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        center_x: f32,
        center_y: f32,
        size: f32,
        color: ThemeColor,
        weight: Weight,
    ) {
        FontStack::draw_line_centered(self, pixmap, text, center_x, center_y, size, color, weight);
    }
}
