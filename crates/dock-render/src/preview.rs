//! Offline preview helpers.
//!
//! The dock is composited over a generated wallpaper so visual review does not
//! depend on a live desktop session.

use dock_core::Rect;
use dock_core::theme::ResolvedTheme;
use tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Point, RadialGradient,
    SpreadMode, Transform,
};

use crate::painter;

/// Generates a soft, Windows 11 inspired wallpaper.
#[must_use]
pub fn wallpaper(width: u32, height: u32, theme: ResolvedTheme) -> Option<Pixmap> {
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixmap = Pixmap::new(width, height)?;
    let (top, bottom) = match theme {
        ResolvedTheme::Dark => (
            Color::from_rgba8(14, 20, 36, 255),
            Color::from_rgba8(28, 34, 58, 255),
        ),
        ResolvedTheme::Light => (
            Color::from_rgba8(240, 244, 252, 255),
            Color::from_rgba8(221, 228, 242, 255),
        ),
    };
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, height as f32),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let Some(shader) = shader {
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, width as f32, height as f32) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    let (w, h) = (width as f32, height as f32);
    let blobs: &[(f32, f32, f32, Color)] = match theme {
        ResolvedTheme::Dark => &[
            (0.18, 0.24, 0.62, Color::from_rgba8(64, 126, 255, 128)),
            (0.83, 0.72, 0.58, Color::from_rgba8(146, 86, 255, 96)),
            (0.58, 0.02, 0.45, Color::from_rgba8(0, 196, 255, 54)),
            (0.98, 0.36, 0.42, Color::from_rgba8(255, 132, 72, 32)),
        ],
        ResolvedTheme::Light => &[
            (0.16, 0.2, 0.6, Color::from_rgba8(110, 166, 255, 90)),
            (0.82, 0.75, 0.55, Color::from_rgba8(196, 150, 255, 70)),
            (0.6, 0.06, 0.42, Color::from_rgba8(120, 226, 255, 50)),
            (0.95, 0.3, 0.4, Color::from_rgba8(255, 184, 140, 40)),
        ],
    };
    for (x, y, radius, color) in blobs {
        let center = Point::from_xy(w * x, h * y);
        let shader = RadialGradient::new(
            center,
            0.0,
            center,
            (w.max(h)) * radius,
            vec![
                GradientStop::new(0.0, *color),
                GradientStop::new(0.55, with_alpha(*color, color.alpha() / 2.0)),
                GradientStop::new(1.0, with_alpha(*color, 0.0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        );
        if let Some(shader) = shader {
            let paint = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
    }

    // Subtle vignette for depth.
    let center = Point::from_xy(w / 2.0, h / 2.0);
    let vignette = match theme {
        ResolvedTheme::Dark => Color::from_rgba8(0, 0, 0, 90),
        ResolvedTheme::Light => Color::from_rgba8(30, 40, 60, 26),
    };
    if let Some(shader) = RadialGradient::new(
        center,
        (w.min(h)) * 0.32,
        center,
        w.max(h) * 0.78,
        vec![
            GradientStop::new(0.0, with_alpha(vignette, 0.0)),
            GradientStop::new(1.0, vignette),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    Some(pixmap)
}

/// Composites a rendered dock onto a generated wallpaper, centered at the
/// bottom edge with a comfortable margin.
#[must_use]
pub fn desktop_with_dock(
    dock: &Pixmap,
    theme: ResolvedTheme,
    width: u32,
    height: u32,
    bottom_margin: u32,
) -> Option<Pixmap> {
    let mut desktop = wallpaper(width, height, theme)?;
    let x = ((width as i64 - dock.width() as i64) / 2).max(0) as i32;
    let y = (height as i64 - dock.height() as i64 - bottom_margin as i64).max(0) as i32;
    let paint = PixmapPaint::default();
    desktop.draw_pixmap(x, y, dock.as_ref(), &paint, Transform::identity(), None);
    Some(desktop)
}

/// Draws a rendered menu bubble over a desktop image at a pixel position.
pub fn overlay(desktop: &mut Pixmap, layer: &Pixmap, x: i32, y: i32) {
    let paint = PixmapPaint::default();
    desktop.draw_pixmap(x, y, layer.as_ref(), &paint, Transform::identity(), None);
}

/// Helper for previews that need a solid tinted panel behind the dock.
pub fn tint_rect(pixmap: &mut Pixmap, rect: Rect, color: Color) {
    painter::fill_rounded(pixmap, rect, 24.0, color);
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color::from_rgba(color.red(), color.green(), color.blue(), alpha)
        .unwrap_or_else(|| Color::from_rgba8(0, 0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpaper_has_size_and_variation() {
        let pixmap = wallpaper(320, 200, ResolvedTheme::Dark).expect("wallpaper");
        assert_eq!(pixmap.width(), 320);
        assert_eq!(pixmap.height(), 200);
        let first = pixmap.pixel(0, 0).expect("pixel");
        let center = pixmap.pixel(160, 100).expect("pixel");
        assert_ne!(first, center);
    }

    #[test]
    fn composite_keeps_dock_pixels() {
        let mut dock = Pixmap::new(40, 20).expect("dock");
        dock.fill(Color::from_rgba8(255, 0, 0, 255));
        let desktop =
            desktop_with_dock(&dock, ResolvedTheme::Light, 200, 120, 10).expect("desktop");
        let x = 80;
        let y = 120 - 20 - 10;
        let pixel = desktop.pixel(x, y as u32).expect("pixel");
        assert_eq!(pixel.red(), 255);
    }
}
