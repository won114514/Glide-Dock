//! Device-pixel draw commands shared by the live backends and offline previews.

use std::sync::Arc;

use dock_core::{Color, Point, Rect};
use tiny_skia::{Pixmap, Transform};

/// Ordered draw commands for a transparent Dock window.
pub struct DockFrame {
    /// Target width, in pixels.
    pub width: u32,
    /// Target height, in pixels.
    pub height: u32,
    /// Back-to-front draw order.
    pub commands: Vec<DrawCommand>,
    /// Interactive geometry, independent of effects and composited opacity.
    pub hit_shapes: Vec<HitShape>,
}

/// One rounded rectangle in the input mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitShape {
    /// Bounds in window pixels.
    pub rect: Rect,
    /// Corner radius in pixels (zero for icon rectangles).
    pub radius: f32,
}

impl HitShape {
    /// Tests an integer Win32 client coordinate against the mask.
    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        self.span(point.y as i32)
            .is_some_and(|(left, right)| point.x >= left as f32 && point.x < right as f32)
    }

    /// Pixel interval within the shape at integer window coordinate `y`.
    #[must_use]
    pub fn span(self, y: i32) -> Option<(i32, i32)> {
        let y = y as f32;
        let r = self
            .radius
            .clamp(0.0, self.rect.width.min(self.rect.height) / 2.0);
        if y < self.rect.top() || y >= self.rect.bottom() {
            return None;
        }
        let dy = (self.rect.top() + r - y)
            .max(y - self.rect.bottom() + r)
            .max(0.0);
        let inset = r - (r * r - dy * dy).max(0.0).sqrt();
        let left = (self.rect.left() + inset).ceil() as i32;
        let right = (self.rect.right() - inset).ceil() as i32;
        (left < right).then_some((left, right))
    }
}

/// A draw operation with straight-alpha colors and premultiplied bitmap assets.
#[allow(missing_docs)]
pub enum DrawCommand {
    Fill {
        rect: Rect,
        radius: f32,
        color: Color,
    },
    Vertical {
        rect: Rect,
        radius: f32,
        top: Color,
        bottom: Color,
    },
    Stroke {
        rect: Rect,
        radius: f32,
        width: f32,
        color: Color,
    },
    Line {
        from: (f32, f32),
        to: (f32, f32),
        width: f32,
        color: Color,
    },
    Circle {
        center: (f32, f32),
        radius: f32,
        stroke: Option<f32>,
        color: Color,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        color: Color,
    },
    Glow {
        rect: Rect,
        spread: f32,
        color: Color,
        strength: f32,
    },
    Shadow {
        rect: Rect,
        radius: f32,
        blur: f32,
        offset: f32,
        color: Color,
        dock: bool,
    },
    Bitmap {
        pixels: Arc<Pixmap>,
        transform: Transform,
        opacity: f32,
    },
}

/// Cached menu pixels and their composition transform for this frame.
pub struct MenuOverlay<'a> {
    /// Rounded bubble (excluding its shadow), in Dock window coordinates.
    pub hit: Option<HitShape>,
    /// Last rasterized menu, reused while only position or opacity changes.
    pub pixels: &'a Arc<Pixmap>,
    /// Top-left corner after applying the opening/closing motion.
    pub position: Point,
    /// Opening/closing opacity.
    pub opacity: f32,
}

impl DockFrame {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            commands: Vec::new(),
            hit_shapes: Vec::new(),
        }
    }

    pub(crate) fn fill_rounded(&mut self, rect: Rect, radius: f32, color: Color) {
        self.commands.push(DrawCommand::Fill {
            rect,
            radius,
            color,
        });
    }

    pub(crate) fn fill_rounded_vertical(
        &mut self,
        rect: Rect,
        radius: f32,
        top: Color,
        bottom: Color,
    ) {
        self.commands.push(DrawCommand::Vertical {
            rect,
            radius,
            top,
            bottom,
        });
    }

    pub(crate) fn stroke_rounded(&mut self, rect: Rect, radius: f32, width: f32, color: Color) {
        self.commands.push(DrawCommand::Stroke {
            rect,
            radius,
            width,
            color,
        });
    }

    pub(crate) fn line(&mut self, from: (f32, f32), to: (f32, f32), width: f32, color: Color) {
        self.commands.push(DrawCommand::Line {
            from,
            to,
            width,
            color,
        });
    }

    pub(crate) fn filled_circle(&mut self, center: (f32, f32), radius: f32, color: Color) {
        self.commands.push(DrawCommand::Circle {
            center,
            radius,
            stroke: None,
            color,
        });
    }

    pub(crate) fn stroke_circle(
        &mut self,
        center: (f32, f32),
        radius: f32,
        width: f32,
        color: Color,
    ) {
        self.commands.push(DrawCommand::Circle {
            center,
            radius,
            stroke: Some(width),
            color,
        });
    }

    pub(crate) fn filled_polygon(&mut self, points: &[(f32, f32)], color: Color) {
        self.commands.push(DrawCommand::Polygon {
            points: points.to_vec(),
            color,
        });
    }

    pub(crate) fn glow(&mut self, rect: Rect, spread: f32, color: Color, strength: f32) {
        self.commands.push(DrawCommand::Glow {
            rect,
            spread,
            color,
            strength,
        });
    }

    pub(crate) fn shadow(&mut self, rect: Rect, radius: f32, blur: f32, offset: f32, color: Color) {
        self.commands.push(DrawCommand::Shadow {
            rect,
            radius,
            blur,
            offset,
            color,
            dock: false,
        });
    }

    pub(crate) fn bitmap(&mut self, pixels: Arc<Pixmap>, transform: Transform, opacity: f32) {
        self.commands.push(DrawCommand::Bitmap {
            pixels,
            transform,
            opacity,
        });
    }

    pub(crate) fn hit_shape(&mut self, rect: Rect, radius: f32) {
        self.hit_shapes.push(HitShape { rect, radius });
    }
}
