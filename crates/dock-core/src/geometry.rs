//! Small, allocation-free 2D geometry primitives used across Glide-Dock.
//!
//! All values are in **device pixels** unless the caller explicitly carries DIP
//! values. The [`Metrics`](crate::layout::Metrics) type is the single bridge
//! between the DIP-based configuration and pixel-space rendering.

use std::ops::{Add, AddAssign, Sub};

/// A point in a two dimensional coordinate space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

impl Point {
    /// Creates a point.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Linear interpolation between two points.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self::new(lerp(self.x, other.x, t), lerp(self.y, other.y, t))
    }

    /// Euclidian distance.
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    /// Rounds the point to the nearest whole pixel.
    #[must_use]
    pub fn round(self) -> Self {
        Self::new(self.x.round(), self.y.round())
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// A width/height pair.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    /// Width in pixels.
    pub width: f32,
    /// Height in pixels.
    pub height: f32,
}

impl Size {
    /// Creates a size.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Returns `true` when either dimension is not positive.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Grows the size by `amount` on every side.
    #[must_use]
    pub fn outset(self, amount: f32) -> Self {
        Self::new(self.width + 2.0 * amount, self.height + 2.0 * amount)
    }
}

/// An axis-aligned rectangle.
///
/// The right and bottom edges are exclusive, matching Win32 `RECT` semantics
/// for hit testing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// An empty rectangle at the origin.
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };

    /// Creates a rectangle from origin and size.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Creates a rectangle from its four edges.
    #[must_use]
    pub fn from_ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
    }

    /// Creates a rectangle centered on `center`.
    #[must_use]
    pub fn from_center(center: Point, width: f32, height: f32) -> Self {
        Self::new(
            center.x - width / 2.0,
            center.y - height / 2.0,
            width,
            height,
        )
    }

    /// Left edge.
    #[must_use]
    pub fn left(self) -> f32 {
        self.x
    }

    /// Top edge.
    #[must_use]
    pub fn top(self) -> f32 {
        self.y
    }

    /// Right edge (exclusive).
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge (exclusive).
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Center point.
    #[must_use]
    pub fn center(self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Returns `true` when the point lies inside the half-open rectangle.
    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    /// Returns `true` when the two rectangles overlap.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Grows the rectangle by `amount` on every side (negative shrinks).
    #[must_use]
    pub fn inset(self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.width - 2.0 * amount).max(0.0),
            (self.height - 2.0 * amount).max(0.0),
        )
    }

    /// Moves the rectangle.
    #[must_use]
    pub fn translate(self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Sets the center-x while keeping the size.
    #[must_use]
    pub fn with_center_x(self, center_x: f32) -> Self {
        self.translate(center_x - self.center().x, 0.0)
    }

    /// Sets the bottom edge while keeping the size.
    #[must_use]
    pub fn with_bottom(self, bottom: f32) -> Self {
        self.translate(0.0, bottom - self.bottom())
    }

    /// Smallest rectangle containing both inputs.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::from_ltrb(left, top, right, bottom)
    }

    /// Moves the rectangle horizontally so it lies inside `bounds` when
    /// possible. Rectangles wider than the bounds are centered.
    #[must_use]
    pub fn clamp_x(self, min_x: f32, max_x: f32) -> Self {
        let width = self.width;
        if width >= max_x - min_x {
            return self.with_center_x((min_x + max_x) / 2.0);
        }
        let x = self.x.clamp(min_x, max_x - width);
        Self::new(x, self.y, self.width, self.height)
    }

    /// Rounds the rectangle to whole pixels.
    #[must_use]
    pub fn round(self) -> Self {
        Self::new(
            self.x.round(),
            self.y.round(),
            self.width.round(),
            self.height.round(),
        )
    }
}

impl Add<Point> for Rect {
    type Output = Self;

    fn add(self, rhs: Point) -> Self::Output {
        self.translate(rhs.x, rhs.y)
    }
}

impl AddAssign<Point> for Rect {
    fn add_assign(&mut self, rhs: Point) {
        *self = self.translate(rhs.x, rhs.y);
    }
}

/// Clamps `t` into `[0, 1]`.
#[must_use]
pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

/// Linear interpolation.
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Classic smoothstep easing, C1 continuous.
#[must_use]
pub fn smoothstep(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

/// Hermite `ease-out` easing used for fades.
#[must_use]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_edges_and_center() {
        let r = Rect::new(10.0, 20.0, 100.0, 40.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 60.0);
        assert_eq!(r.center(), Point::new(60.0, 40.0));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(r.contains(Point::new(9.999, 9.999)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
        assert!(!r.contains(Point::new(5.0, 10.0)));
        assert!(!r.contains(Point::new(-0.001, 5.0)));
    }

    #[test]
    fn clamp_x_keeps_rect_inside() {
        let r = Rect::new(900.0, 0.0, 50.0, 10.0);
        let clamped = r.clamp_x(0.0, 400.0);
        assert_eq!(clamped.right(), 400.0);
        assert_eq!(clamped.width, 50.0);
    }

    #[test]
    fn clamp_x_centers_oversized() {
        let r = Rect::new(0.0, 0.0, 500.0, 10.0);
        let clamped = r.clamp_x(0.0, 400.0);
        assert_eq!(clamped.x, -50.0);
    }

    #[test]
    fn smoothstep_midpoint() {
        assert!((smoothstep(0.5) - 0.5).abs() < f32::EPSILON);
        assert_eq!(smoothstep(-2.0), 0.0);
        assert_eq!(smoothstep(2.0), 1.0);
    }
}
