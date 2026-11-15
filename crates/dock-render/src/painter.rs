//! Low level painting primitives: rounded rectangles, gradients, soft shadows,
//! scaled pixmaps and the small line-art icon set used by menus.

use dock_core::Color as ThemeColor;
use dock_core::geometry::Rect;
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LinearGradient, Paint, Path,
    PathBuilder, Pixmap, PixmapPaint, PixmapRef, Point, PremultipliedColorU8, RadialGradient,
    Shader, SpreadMode, Stroke, Transform,
};

/// Converts a theme color to a tiny-skia color.
#[must_use]
pub fn color(value: ThemeColor) -> Color {
    Color::from_rgba8(value.r, value.g, value.b, value.a)
}

/// Converts a domain rectangle to a tiny-skia rectangle.
#[must_use]
pub fn rect(value: Rect) -> Option<tiny_skia::Rect> {
    if value.width <= 0.0 || value.height <= 0.0 || !value.x.is_finite() || !value.y.is_finite() {
        return None;
    }
    tiny_skia::Rect::from_xywh(value.x, value.y, value.width, value.height)
}

/// Builds a rounded rectangle path.
#[must_use]
pub fn rounded_rect_path(value: Rect, radius: f32) -> Option<Path> {
    let rect = rect(value)?;
    let max_radius = rect.width().min(rect.height()) / 2.0;
    let radius = radius.max(0.0).min(max_radius);
    if radius <= 0.5 {
        let mut builder = PathBuilder::new();
        builder.push_rect(rect);
        return builder.finish();
    }
    let (left, top, right, bottom) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let kappa = 0.552_284_8 * radius;
    let mut builder = PathBuilder::new();
    builder.move_to(left + radius, top);
    builder.line_to(right - radius, top);
    builder.cubic_to(
        right - radius + kappa,
        top,
        right,
        top + radius - kappa,
        right,
        top + radius,
    );
    builder.line_to(right, bottom - radius);
    builder.cubic_to(
        right,
        bottom - radius + kappa,
        right - radius + kappa,
        bottom,
        right - radius,
        bottom,
    );
    builder.line_to(left + radius, bottom);
    builder.cubic_to(
        left + radius - kappa,
        bottom,
        left,
        bottom - radius + kappa,
        left,
        bottom - radius,
    );
    builder.line_to(left, top + radius);
    builder.cubic_to(
        left,
        top + radius - kappa,
        left + radius - kappa,
        top,
        left + radius,
        top,
    );
    builder.close();
    builder.finish()
}

/// Fills a rounded rectangle with a solid color.
pub fn fill_rounded(pixmap: &mut Pixmap, value: Rect, radius: f32, fill: Color) {
    let Some(path) = rounded_rect_path(value, radius) else {
        return;
    };
    let paint = Paint {
        shader: Shader::SolidColor(fill),
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Fills a rounded rectangle with a vertical two stop gradient.
pub fn fill_rounded_vertical(
    pixmap: &mut Pixmap,
    value: Rect,
    radius: f32,
    top: Color,
    bottom: Color,
) {
    let Some(path) = rounded_rect_path(value, radius) else {
        return;
    };
    let shader = LinearGradient::new(
        Point::from_xy(0.0, value.y),
        Point::from_xy(0.0, value.y + value.height.max(1.0)),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    );
    let paint = Paint {
        shader: shader.unwrap_or(Shader::SolidColor(top)),
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Strokes a rounded rectangle outline.
pub fn stroke_rounded(
    pixmap: &mut Pixmap,
    value: Rect,
    radius: f32,
    width: f32,
    stroke_color: Color,
) {
    let Some(path) = rounded_rect_path(value, radius) else {
        return;
    };
    let paint = Paint {
        shader: Shader::SolidColor(stroke_color),
        anti_alias: true,
        ..Paint::default()
    };
    let stroke = Stroke {
        width,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draws a straight line with round caps.
pub fn line(
    pixmap: &mut Pixmap,
    from: (f32, f32),
    to: (f32, f32),
    width: f32,
    stroke_color: Color,
) {
    let mut builder = PathBuilder::new();
    builder.move_to(from.0, from.1);
    builder.line_to(to.0, to.1);
    let Some(path) = builder.finish() else {
        return;
    };
    let paint = Paint {
        shader: Shader::SolidColor(stroke_color),
        anti_alias: true,
        ..Paint::default()
    };
    let stroke = Stroke {
        width,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Fills a circle.
pub fn filled_circle(pixmap: &mut Pixmap, center: (f32, f32), radius: f32, fill: Color) {
    if radius <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::new();
    builder.push_circle(center.0, center.1, radius);
    let Some(path) = builder.finish() else {
        return;
    };
    let paint = Paint {
        shader: Shader::SolidColor(fill),
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Strokes a circle, used for launch and click ripples.
pub fn stroke_circle(
    pixmap: &mut Pixmap,
    center: (f32, f32),
    radius: f32,
    width: f32,
    stroke_color: Color,
) {
    if radius <= 0.0 || width <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::new();
    builder.push_circle(center.0, center.1, radius);
    let Some(path) = builder.finish() else {
        return;
    };
    let paint = Paint {
        shader: Shader::SolidColor(stroke_color),
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.stroke_path(
        &path,
        &paint,
        &Stroke {
            width,
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
}

/// Draws a soft blue-violet halo behind an icon.
///
/// A radial gradient is cheap enough to evaluate once per icon per frame and
/// avoids allocating a blurred mask.
pub fn glow(pixmap: &mut Pixmap, icon: Rect, radius: f32, glow_color: ThemeColor, strength: f32) {
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.001 || radius <= 0.0 {
        return;
    }
    let center = Point::from_xy(icon.center().x, icon.center().y);
    let outer = (icon.width.max(icon.height) * 0.5 + radius).max(1.0);
    let alpha = f32::from(glow_color.a) / 255.0;
    let shader = RadialGradient::new(
        center,
        outer * 0.30,
        center,
        outer,
        vec![
            GradientStop::new(
                0.0,
                Color::from_rgba8(glow_color.r, glow_color.g, glow_color.b, 0),
            ),
            GradientStop::new(
                0.55,
                Color::from_rgba8(
                    glow_color.r,
                    glow_color.g,
                    glow_color.b,
                    (alpha * 0.55 * strength * 255.0).round() as u8,
                ),
            ),
            GradientStop::new(
                1.0,
                Color::from_rgba8(glow_color.r, glow_color.g, glow_color.b, 0),
            ),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    );
    let Some(shader) = shader else {
        return;
    };
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    let expanded = icon.inset(-radius);
    if let Some(rect) = rect(expanded) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// Fills a polygon from points.
pub fn filled_polygon(pixmap: &mut Pixmap, points: &[(f32, f32)], fill: Color) {
    if points.len() < 3 {
        return;
    }
    let mut builder = PathBuilder::new();
    builder.move_to(points[0].0, points[0].1);
    for point in &points[1..] {
        builder.line_to(point.0, point.1);
    }
    builder.close();
    let Some(path) = builder.finish() else {
        return;
    };
    let paint = Paint {
        shader: Shader::SolidColor(fill),
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Draws a source pixmap into `destination`, scaling to fit.
pub fn draw_scaled(pixmap: &mut Pixmap, source: PixmapRef, destination: Rect, opacity: f32) {
    if destination.width <= 0.0 || destination.height <= 0.0 || opacity <= 0.001 {
        return;
    }
    let scale_x = destination.width / source.width() as f32;
    let scale_y = destination.height / source.height() as f32;
    let transform =
        Transform::from_scale(scale_x, scale_y).post_translate(destination.x, destination.y);
    let paint = PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode: BlendMode::SourceOver,
        quality: FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, source, &paint, transform, None);
}

/// Draws a source pixmap with a small faux-3D tilt around its centre.
///
/// tiny-skia is a 2D rasterizer, so the tilt is an affine approximation:
/// yaw/pitch compress the axis and add a small shear. At the Dock's ±5° range
/// this reads as perspective without per-pixel homography.
pub fn draw_scaled_tilted(
    pixmap: &mut Pixmap,
    source: PixmapRef,
    destination: Rect,
    opacity: f32,
    tilt_x_deg: f32,
    tilt_y_deg: f32,
) {
    if destination.width <= 0.0 || destination.height <= 0.0 || opacity <= 0.001 {
        return;
    }
    if tilt_x_deg.abs() < 0.01 && tilt_y_deg.abs() < 0.01 {
        draw_scaled(pixmap, source, destination, opacity);
        return;
    }
    let source_width = source.width() as f32;
    let source_height = source.height() as f32;
    if source_width <= 0.0 || source_height <= 0.0 {
        return;
    }
    let yaw = tilt_y_deg.to_radians();
    let pitch = tilt_x_deg.to_radians();
    let cos_yaw = (1.0 - 0.25 * yaw.sin().abs()).clamp(0.90, 1.0);
    let cos_pitch = (1.0 - 0.25 * pitch.sin().abs()).clamp(0.90, 1.0);
    let a = destination.width / source_width * cos_yaw;
    let d = destination.height / source_height * cos_pitch;
    let b = destination.width / source_width * yaw.sin() * 0.30;
    let c = -destination.height / source_height * pitch.sin() * 0.30;
    let center = destination.center();
    let tx = center.x - a * source_width / 2.0 - b * source_height / 2.0;
    let ty = center.y - c * source_width / 2.0 - d * source_height / 2.0;
    let transform = Transform::from_row(a, c, b, d, tx, ty);
    let paint = PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode: BlendMode::SourceOver,
        quality: FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, source, &paint, transform, None);
}

/// Draws a soft, blurred shadow of a rounded rectangle.
pub fn shadow(
    pixmap: &mut Pixmap,
    value: Rect,
    radius: f32,
    blur: f32,
    offset_y: f32,
    shadow_color: ThemeColor,
) {
    if let Some(shadow_pixmap) = shadow_layer(
        pixmap.width(),
        pixmap.height(),
        value,
        radius,
        blur,
        offset_y,
        shadow_color,
    ) {
        let paint = PixmapPaint {
            opacity: 1.0,
            blend_mode: BlendMode::SourceOver,
            quality: FilterQuality::Nearest,
        };
        pixmap.draw_pixmap(
            0,
            0,
            shadow_pixmap.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
    }
}

/// Renders a complete soft shadow layer for a rounded rectangle.
///
/// The returned pixmap is the size of the destination and already colorized and
/// premultiplied, so it can be cached and blitted directly.
#[must_use]
pub fn shadow_layer(
    width: u32,
    height: u32,
    value: Rect,
    radius: f32,
    blur: f32,
    offset_y: f32,
    shadow_color: ThemeColor,
) -> Option<Pixmap> {
    shadow_layer_inner(
        width,
        height,
        value,
        radius,
        blur,
        offset_y,
        shadow_color,
        None,
    )
}

/// Like [`shadow_layer`], but leaves the rectangle itself transparent. Used
/// when a DWM glass window already paints the interior.
#[must_use]
pub fn shadow_layer_outside(
    width: u32,
    height: u32,
    value: Rect,
    radius: f32,
    blur: f32,
    offset_y: f32,
    shadow_color: ThemeColor,
) -> Option<Pixmap> {
    shadow_layer_inner(
        width,
        height,
        value,
        radius,
        blur,
        offset_y,
        shadow_color,
        Some(value),
    )
}

#[allow(clippy::too_many_arguments)]
fn shadow_layer_inner(
    width: u32,
    height: u32,
    value: Rect,
    radius: f32,
    blur: f32,
    offset_y: f32,
    shadow_color: ThemeColor,
    hole: Option<Rect>,
) -> Option<Pixmap> {
    let shadow_rect = value.translate(0.0, offset_y);
    let mask = build_shadow_mask(width, height, shadow_rect, radius, blur, hole)?;
    colorize_mask(&mask, width, height, shadow_color)
}

fn build_shadow_mask(
    width: u32,
    height: u32,
    rect: Rect,
    radius: f32,
    blur: f32,
    hole: Option<Rect>,
) -> Option<Pixmap> {
    let mut mask = Pixmap::new(width, height)?;
    fill_rounded(
        &mut mask,
        rect,
        radius,
        Color::from_rgba8(255, 255, 255, 255),
    );
    let mut alpha: Vec<u8> = mask.pixels().iter().map(|pixel| pixel.alpha()).collect();
    let radius = ((blur * 0.5).round() as usize).clamp(1, 64);
    box_blur(&mut alpha, width as usize, height as usize, radius, 3);
    if let Some(hole) = hole {
        for y in 0..height {
            for x in 0..width {
                if hole.contains(dock_core::geometry::Point::new(
                    x as f32 + 0.5,
                    y as f32 + 0.5,
                )) {
                    alpha[(y * width + x) as usize] = 0;
                }
            }
        }
    }
    mask.pixels_mut()
        .iter_mut()
        .zip(alpha)
        .for_each(|(pixel, value)| {
            *pixel = PremultipliedColorU8::from_rgba(value, value, value, value)
                .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        });
    Some(mask)
}

fn colorize_mask(
    mask: &Pixmap,
    width: u32,
    height: u32,
    shadow_color: ThemeColor,
) -> Option<Pixmap> {
    let mut shadow = Pixmap::new(width, height)?;
    let alpha_scale = u32::from(shadow_color.a);
    for (target, source) in shadow.pixels_mut().iter_mut().zip(mask.pixels()) {
        let alpha = (u32::from(source.alpha()) * alpha_scale / 255) as u8;
        if alpha == 0 {
            *target = PremultipliedColorU8::TRANSPARENT;
            continue;
        }
        let red = (u32::from(shadow_color.r) * u32::from(alpha) / 255) as u8;
        let green = (u32::from(shadow_color.g) * u32::from(alpha) / 255) as u8;
        let blue = (u32::from(shadow_color.b) * u32::from(alpha) / 255) as u8;
        *target = PremultipliedColorU8::from_rgba(red, green, blue, alpha)
            .unwrap_or(PremultipliedColorU8::TRANSPARENT);
    }
    Some(shadow)
}

fn box_blur(alpha: &mut [u8], width: usize, height: usize, radius: usize, passes: usize) {
    if width == 0 || height == 0 || radius == 0 {
        return;
    }
    let mut scratch = vec![0u8; alpha.len()];
    for _ in 0..passes {
        box_blur_horizontal(alpha, &mut scratch, width, height, radius);
        box_blur_vertical(&scratch, alpha, width, height, radius);
    }
}

fn box_blur_horizontal(src: &[u8], dst: &mut [u8], width: usize, height: usize, radius: usize) {
    let diameter = (2 * radius + 1) as u32;
    for y in 0..height {
        let row = y * width;
        let first = u32::from(src[row]);
        let mut sum = first * (radius as u32 + 1);
        for offset in 1..=radius {
            sum += u32::from(src[row + offset.min(width - 1)]);
        }
        for x in 0..width {
            dst[row + x] = (sum / diameter).min(255) as u8;
            let incoming = src[row + (x + radius + 1).min(width - 1)];
            let outgoing = src[row + x.saturating_sub(radius)];
            sum = sum + u32::from(incoming) - u32::from(outgoing);
        }
    }
}

fn box_blur_vertical(src: &[u8], dst: &mut [u8], width: usize, height: usize, radius: usize) {
    let diameter = (2 * radius + 1) as u32;
    for x in 0..width {
        let first = u32::from(src[x]);
        let mut sum = first * (radius as u32 + 1);
        for offset in 1..=radius {
            sum += u32::from(src[(offset.min(height - 1)) * width + x]);
        }
        for y in 0..height {
            dst[y * width + x] = (sum / diameter).min(255) as u8;
            let incoming = src[(y + radius + 1).min(height - 1) * width + x];
            let outgoing = src[y.saturating_sub(radius) * width + x];
            sum = sum + u32::from(incoming) - u32::from(outgoing);
        }
    }
}

/// Draws a small line-art glyph used by the context menu.
pub fn menu_glyph(pixmap: &mut Pixmap, kind: MenuGlyph, box_rect: Rect, glyph_color: Color) {
    let scale = box_rect.width.max(1.0) / 16.0;
    let ox = box_rect.x;
    let oy = box_rect.y;
    let p = |x: f32, y: f32| (ox + x * scale, oy + y * scale);
    let w = 1.45 * scale;

    match kind {
        MenuGlyph::Open => {
            line(pixmap, p(3.0, 6.0), p(3.0, 13.0), w, glyph_color);
            line(pixmap, p(3.0, 13.0), p(10.0, 13.0), w, glyph_color);
            line(pixmap, p(10.0, 13.0), p(10.0, 9.0), w, glyph_color);
            line(pixmap, p(8.0, 3.0), p(13.0, 3.0), w, glyph_color);
            line(pixmap, p(13.0, 3.0), p(13.0, 8.0), w, glyph_color);
            line(pixmap, p(8.5, 7.5), p(13.0, 3.0), w, glyph_color);
        }
        MenuGlyph::Folder => {
            let mut builder = PathBuilder::new();
            let points = [
                p(2.5, 4.5),
                p(6.5, 4.5),
                p(8.0, 6.5),
                p(13.5, 6.5),
                p(13.5, 12.5),
                p(2.5, 12.5),
            ];
            builder.move_to(points[0].0, points[0].1);
            for point in &points[1..] {
                builder.line_to(point.0, point.1);
            }
            builder.close();
            if let Some(path) = builder.finish() {
                let paint = Paint {
                    shader: Shader::SolidColor(glyph_color),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &Stroke {
                        width: w,
                        ..Stroke::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
        MenuGlyph::Trash => {
            line(pixmap, p(3.5, 4.5), p(12.5, 4.5), w, glyph_color);
            line(pixmap, p(6.5, 4.5), p(6.5, 3.0), w, glyph_color);
            line(pixmap, p(6.5, 3.0), p(9.5, 3.0), w, glyph_color);
            line(pixmap, p(9.5, 3.0), p(9.5, 4.5), w, glyph_color);
            line(pixmap, p(4.8, 6.0), p(5.4, 13.0), w, glyph_color);
            line(pixmap, p(5.4, 13.0), p(10.6, 13.0), w, glyph_color);
            line(pixmap, p(10.6, 13.0), p(11.2, 6.0), w, glyph_color);
            line(pixmap, p(7.0, 7.5), p(7.2, 11.5), w * 0.8, glyph_color);
            line(pixmap, p(9.0, 7.5), p(8.8, 11.5), w * 0.8, glyph_color);
        }
        MenuGlyph::Settings => {
            for (y, knob) in [(4.0, 10.0), (8.0, 6.0), (12.0, 9.0)] {
                line(pixmap, p(3.0, y), p(13.0, y), w, glyph_color);
                filled_circle(pixmap, p(knob, y), 1.7 * scale, glyph_color);
            }
        }
        MenuGlyph::Refresh => {
            let center = p(8.0, 8.0);
            let radius = 5.0 * scale;
            let mut builder = PathBuilder::new();
            let start = -0.35 * std::f32::consts::PI;
            let end = 1.35 * std::f32::consts::PI;
            let steps = 20;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let angle = start + (end - start) * t;
                let x = center.0 + radius * angle.cos();
                let y = center.1 + radius * angle.sin();
                if step == 0 {
                    builder.move_to(x, y);
                } else {
                    builder.line_to(x, y);
                }
            }
            if let Some(path) = builder.finish() {
                let paint = Paint {
                    shader: Shader::SolidColor(glyph_color),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &Stroke {
                        width: w,
                        ..Stroke::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            // Arrow head at the end of the arc.
            let end_angle = end;
            let tip = p(8.0 + 5.0 * end_angle.cos(), 8.0 + 5.0 * end_angle.sin());
            line(
                pixmap,
                tip,
                (tip.0 - 2.2 * scale, tip.1 - 0.6 * scale),
                w,
                glyph_color,
            );
            line(
                pixmap,
                tip,
                (tip.0 - 0.4 * scale, tip.1 - 2.4 * scale),
                w,
                glyph_color,
            );
        }
        MenuGlyph::Edit => {
            line(pixmap, p(3.5, 12.5), p(11.0, 5.0), w, glyph_color);
            line(pixmap, p(11.0, 5.0), p(13.0, 7.0), w, glyph_color);
            line(pixmap, p(13.0, 7.0), p(5.5, 14.0), w, glyph_color);
            line(pixmap, p(5.5, 14.0), p(3.5, 12.5), w, glyph_color);
        }
        MenuGlyph::Power => {
            let center = p(8.0, 8.4);
            let radius = 4.6 * scale;
            let start = -0.75 * std::f32::consts::PI;
            let end = 0.75 * std::f32::consts::PI;
            let mut builder = PathBuilder::new();
            let steps = 22;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let angle = start + (end - start) * t;
                let x = center.0 + radius * angle.cos();
                let y = center.1 + radius * angle.sin();
                if step == 0 {
                    builder.move_to(x, y);
                } else {
                    builder.line_to(x, y);
                }
            }
            if let Some(path) = builder.finish() {
                let paint = Paint {
                    shader: Shader::SolidColor(glyph_color),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &Stroke {
                        width: w,
                        ..Stroke::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            line(pixmap, p(8.0, 2.8), p(8.0, 8.0), w, glyph_color);
        }
        MenuGlyph::Appearance => {
            outline_circle(pixmap, p(8.0, 8.0), 5.0 * scale, w, glyph_color);
            filled_circle(pixmap, p(5.8, 6.6), 1.0 * scale, glyph_color);
            filled_circle(pixmap, p(9.6, 5.8), 1.0 * scale, glyph_color);
            filled_circle(pixmap, p(10.2, 9.2), 1.0 * scale, glyph_color);
        }
        MenuGlyph::Layers => {
            line(pixmap, p(8.0, 3.0), p(13.0, 6.0), w, glyph_color);
            line(pixmap, p(13.0, 6.0), p(8.0, 9.0), w, glyph_color);
            line(pixmap, p(8.0, 9.0), p(3.0, 6.0), w, glyph_color);
            line(pixmap, p(3.0, 6.0), p(8.0, 3.0), w, glyph_color);
            line(pixmap, p(3.0, 9.0), p(8.0, 12.0), w, glyph_color);
            line(pixmap, p(8.0, 12.0), p(13.0, 9.0), w, glyph_color);
        }
        MenuGlyph::Tips => {
            outline_circle(pixmap, p(8.0, 6.5), 4.0 * scale, w, glyph_color);
            line(pixmap, p(6.0, 10.5), p(10.0, 10.5), w, glyph_color);
            line(pixmap, p(6.6, 12.5), p(9.4, 12.5), w, glyph_color);
            line(pixmap, p(8.0, 2.0), p(8.0, 1.0), w, glyph_color);
            line(pixmap, p(3.5, 3.5), p(2.7, 2.7), w, glyph_color);
            line(pixmap, p(12.5, 3.5), p(13.3, 2.7), w, glyph_color);
        }
        MenuGlyph::Check => {
            line(pixmap, p(3.0, 8.2), p(6.6, 11.8), w, glyph_color);
            line(pixmap, p(6.6, 11.8), p(13.0, 4.2), w, glyph_color);
        }
        MenuGlyph::Blank => {}
        MenuGlyph::Desktop => {
            line(pixmap, p(2.8, 3.5), p(13.2, 3.5), w, glyph_color);
            line(pixmap, p(13.2, 3.5), p(13.2, 10.5), w, glyph_color);
            line(pixmap, p(13.2, 10.5), p(2.8, 10.5), w, glyph_color);
            line(pixmap, p(2.8, 10.5), p(2.8, 3.5), w, glyph_color);
            line(pixmap, p(8.0, 10.5), p(8.0, 13.0), w, glyph_color);
            line(pixmap, p(5.8, 13.0), p(10.2, 13.0), w, glyph_color);
        }
        MenuGlyph::Window => {
            line(pixmap, p(3.0, 4.0), p(13.0, 4.0), w, glyph_color);
            line(pixmap, p(13.0, 4.0), p(13.0, 12.0), w, glyph_color);
            line(pixmap, p(13.0, 12.0), p(3.0, 12.0), w, glyph_color);
            line(pixmap, p(3.0, 12.0), p(3.0, 4.0), w, glyph_color);
            line(pixmap, p(3.0, 7.0), p(13.0, 7.0), w, glyph_color);
        }
        MenuGlyph::Maximize => {
            line(pixmap, p(3.0, 9.0), p(3.0, 13.0), w, glyph_color);
            line(pixmap, p(3.0, 13.0), p(7.0, 13.0), w, glyph_color);
            line(pixmap, p(7.0, 13.0), p(13.0, 7.0), w, glyph_color);
            line(pixmap, p(13.0, 7.0), p(13.0, 3.0), w, glyph_color);
            line(pixmap, p(13.0, 3.0), p(9.0, 3.0), w, glyph_color);
            line(pixmap, p(9.0, 3.0), p(13.0, 7.0), w, glyph_color);
        }
        MenuGlyph::Start => {
            for (x, y) in [(3.0, 3.0), (8.5, 3.0), (3.0, 8.5), (8.5, 8.5)] {
                line(pixmap, p(x, y), p(x + 4.5, y), w, glyph_color);
                line(pixmap, p(x + 4.5, y), p(x + 4.5, y + 4.5), w, glyph_color);
                line(pixmap, p(x + 4.5, y + 4.5), p(x, y + 4.5), w, glyph_color);
                line(pixmap, p(x, y + 4.5), p(x, y), w, glyph_color);
            }
        }
        MenuGlyph::Search => {
            outline_circle(pixmap, p(7.2, 7.2), 4.0 * scale, w, glyph_color);
            line(pixmap, p(10.2, 10.2), p(13.2, 13.2), w, glyph_color);
        }
        MenuGlyph::TaskView => {
            line(pixmap, p(3.0, 5.5), p(10.5, 5.5), w, glyph_color);
            line(pixmap, p(10.5, 5.5), p(10.5, 12.5), w, glyph_color);
            line(pixmap, p(10.5, 12.5), p(3.0, 12.5), w, glyph_color);
            line(pixmap, p(3.0, 12.5), p(3.0, 5.5), w, glyph_color);
            line(pixmap, p(6.0, 3.0), p(13.0, 3.0), w, glyph_color);
            line(pixmap, p(13.0, 3.0), p(13.0, 10.0), w, glyph_color);
            line(pixmap, p(13.0, 10.0), p(10.5, 10.0), w, glyph_color);
        }
    }
}

fn outline_circle(pixmap: &mut Pixmap, center: (f32, f32), radius: f32, width: f32, color: Color) {
    let steps = 24;
    let mut previous = center;
    for step in 0..=steps {
        let angle = step as f32 / steps as f32 * std::f32::consts::TAU;
        let point = (
            center.0 + radius * angle.cos(),
            center.1 + radius * angle.sin(),
        );
        if step > 0 {
            line(pixmap, previous, point, width, color);
        }
        previous = point;
    }
}

/// Glyph variants used by [`menu_glyph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuGlyph {
    /// Launch.
    Open,
    /// Folder.
    Folder,
    /// Trash.
    Trash,
    /// Sliders.
    Settings,
    /// Rotating arrow.
    Refresh,
    /// Pencil.
    Edit,
    /// Power symbol.
    Power,
    /// Color palette.
    Appearance,
    /// Stacked layers.
    Layers,
    /// Light bulb.
    Tips,
    /// Check mark.
    Check,
    /// Empty icon slot.
    Blank,
    /// Desktop monitor.
    Desktop,
    /// Window.
    Window,
    /// Maximize arrow.
    Maximize,
    /// Start grid.
    Start,
    /// Search.
    Search,
    /// Task view.
    TaskView,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dock_core::Color as ThemeColor;

    #[test]
    fn rounded_path_is_inside_bounds() {
        let path = rounded_rect_path(Rect::new(0.0, 0.0, 100.0, 50.0), 12.0).expect("path");
        let bounds = path.bounds();
        assert!(bounds.left() >= -0.01);
        assert!(bounds.right() <= 100.01);
        assert!(bounds.bottom() <= 50.01);
    }

    #[test]
    fn shadow_produces_semi_transparent_pixels() {
        let mut pixmap = Pixmap::new(200, 100).expect("pixmap");
        shadow(
            &mut pixmap,
            Rect::new(40.0, 30.0, 120.0, 50.0),
            16.0,
            12.0,
            4.0,
            ThemeColor::rgba(0, 0, 0, 150),
        );
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0));
        assert!(
            pixmap
                .pixels()
                .iter()
                .any(|pixel| pixel.alpha() > 0 && pixel.alpha() < 150)
        );
    }

    #[test]
    fn box_blur_preserves_uniform_plane() {
        let mut plane = vec![128u8; 20 * 10];
        box_blur(&mut plane, 20, 10, 3, 2);
        assert!(plane.iter().all(|value| *value == 128));
    }

    #[test]
    fn scaled_pixmap_lands_on_destination() {
        let mut destination = Pixmap::new(100, 100).expect("pixmap");
        let mut source = Pixmap::new(10, 10).expect("source");
        source.fill(Color::from_rgba8(255, 0, 0, 255));
        draw_scaled(
            &mut destination,
            source.as_ref(),
            Rect::new(20.0, 30.0, 40.0, 40.0),
            1.0,
        );
        assert_eq!(destination.pixel(25, 35).map(|p| p.alpha()), Some(255));
        assert_eq!(destination.pixel(5, 5).map(|p| p.alpha()), Some(0));
    }

    #[test]
    fn tilted_pixmap_stays_near_its_destination() {
        let mut destination = Pixmap::new(120, 120).expect("pixmap");
        let mut source = Pixmap::new(20, 20).expect("source");
        source.fill(Color::from_rgba8(80, 200, 255, 255));
        draw_scaled_tilted(
            &mut destination,
            source.as_ref(),
            Rect::new(40.0, 40.0, 40.0, 40.0),
            1.0,
            5.0,
            -5.0,
        );
        assert!(destination.pixel(60, 60).expect("center").alpha() > 0);
        let lit = destination
            .pixels()
            .iter()
            .filter(|pixel| pixel.alpha() > 0)
            .count();
        assert!(lit > 400);
        assert!(draw_scaled_tilted_check_bounds(&destination));
    }

    fn draw_scaled_tilted_check_bounds(pixmap: &Pixmap) -> bool {
        let mut min_x = u32::MAX;
        let mut max_x = 0;
        let mut min_y = u32::MAX;
        let mut max_y = 0;
        for y in 0..pixmap.height() {
            for x in 0..pixmap.width() {
                if pixmap.pixel(x, y).is_some_and(|pixel| pixel.alpha() > 0) {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        min_x >= 30 && max_x <= 90 && min_y >= 30 && max_y <= 90
    }

    #[test]
    fn glow_and_ring_paint_pixels() {
        let mut pixmap = Pixmap::new(120, 120).expect("pixmap");
        glow(
            &mut pixmap,
            Rect::new(40.0, 40.0, 40.0, 40.0),
            12.0,
            ThemeColor::rgb(120, 100, 255),
            1.0,
        );
        stroke_circle(
            &mut pixmap,
            (60.0, 60.0),
            30.0,
            2.0,
            Color::from_rgba8(255, 255, 255, 200),
        );
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0));
        assert!(pixmap.pixel(60, 30).is_some_and(|pixel| pixel.alpha() > 0));
        assert!(pixmap.pixel(60, 60).is_some_and(|pixel| pixel.alpha() == 0));
    }
}
