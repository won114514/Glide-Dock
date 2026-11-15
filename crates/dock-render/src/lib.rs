//! Glide-Dock CPU renderer.
//!
//! The renderer is deliberately free of Win32: it consumes a [`DockScene`] and
//! produces a premultiplied RGBA [`Pixmap`]. The same code path is used for the
//! live layered window and for offline PNG previews.

#![warn(missing_docs)]

pub mod icons;
pub mod painter;
pub mod preview;
pub mod text;

use std::sync::Arc;

use dock_core::layout::tooltip_rect;
use dock_core::models::DockConfig;
use dock_core::state::{MenuEntry, MenuIcon};
use dock_core::theme::{Color as ThemeColor, Palette};
use dock_core::{
    DockGeometry, DockItem, DockState, FrameLayout, Metrics, Point, Rect, RippleMotion, Size,
};
use tiny_skia::{BlendMode, Color, FilterQuality, Pixmap, PixmapPaint, PixmapRef, Transform};

use crate::icons::{IconSource, draw_unresolved_badge};
use crate::painter::MenuGlyph;
use crate::text::{FontStack, Weight};

/// A drag in progress, prepared for rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragVisual {
    /// Index of the dragged item.
    pub index: usize,
    /// Pointer position in window-local pixels.
    pub cursor: Point,
    /// Offset of the grab point from the icon's left edge.
    pub grab_dx: f32,
    /// Horizontal position of the insertion indicator, if any.
    pub insertion_x: Option<f32>,
    /// `true` once the drag threshold has been crossed.
    pub active: bool,
}

/// Per-icon interaction transform for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconAnimation {
    /// Scale relative to the current layout base (hover magnification preserved).
    pub scale: Option<f32>,
    /// Horizontal nudge in device pixels.
    pub offset_x: f32,
    /// Vertical bounce in device pixels.
    pub offset_y: f32,
    /// Tilt around the X axis in degrees (Perspective).
    pub tilt_x_deg: f32,
    /// Tilt around the Y axis in degrees (Perspective).
    pub tilt_y_deg: f32,
    /// Blue-violet hover/launch glow in `0..=1`.
    pub glow: f32,
    /// Expanding ripple ring, if any.
    pub ripple: Option<RippleMotion>,
    /// Waiting-dot opacity, when the launch is waiting for a window.
    pub waiting: Option<f32>,
    /// Whether the fixed running dot may be shown for this frame.
    pub running_dot_allowed: bool,
}

impl Default for IconAnimation {
    fn default() -> Self {
        Self {
            scale: None,
            offset_x: 0.0,
            offset_y: 0.0,
            tilt_x_deg: 0.0,
            tilt_y_deg: 0.0,
            glow: 0.0,
            ripple: None,
            waiting: None,
            running_dot_allowed: true,
        }
    }
}

/// Everything the renderer needs for one dock frame.
pub struct DockScene<'a> {
    /// Persisted configuration (appearance, items, opacity).
    pub config: &'a DockConfig,
    /// Device pixel metrics.
    pub metrics: &'a Metrics,
    /// Static dock geometry.
    pub geometry: &'a DockGeometry,
    /// Animated frame layout.
    pub layout: &'a FrameLayout,
    /// Per-item interaction transforms, aligned with `config.items`.
    pub animations: &'a [IconAnimation],
    /// Runtime state.
    pub state: &'a DockState,
    /// Resolved palette.
    pub palette: Palette,
    /// Icon provider.
    pub icons: &'a dyn IconSource,

    /// Tooltip fade progress in `0..=1`.
    pub tooltip_progress: f32,
    /// Hover backdrop progress in `0..=1`.
    pub hover_progress: f32,
    /// Transient error message.
    pub error: Option<&'a str>,
    /// Error bubble fade progress in `0..=1`.
    pub error_progress: f32,
    /// Drag visual state.
    pub drag: Option<DragVisual>,
    /// When true, a Windows.UI.Composition host-backdrop window paints the
    /// live glass behind the dock.
    pub native_glass: bool,
    /// When true, the composition glass window paints the dock interior.
    pub glass: bool,
}

/// A single row inside a context menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuRow {
    /// Row rectangle in menu-window coordinates.
    pub rect: Rect,
    /// Index into the entries slice.
    pub entry: usize,
}

/// Measured context menu layout (includes transparent room for the shadow).
#[derive(Debug, Clone, PartialEq)]
pub struct MenuLayout {
    /// Window size including the shadow margin.
    pub window: Size,
    /// Menu bubble rectangle in window coordinates.
    pub bubble: Rect,
    /// Clickable rows.
    pub rows: Vec<MenuRow>,
    /// Corner radius of the bubble.
    pub radius: f32,
    /// Shadow margin.
    pub margin: f32,
}

impl MenuLayout {
    /// Hit tests a point against the menu rows.
    #[must_use]
    pub fn hit(&self, point: Point) -> Option<usize> {
        self.rows
            .iter()
            .find(|row| row.rect.contains(point))
            .map(|row| row.entry)
    }

    /// Returns `true` when the point is inside the bubble (including padding).
    #[must_use]
    pub fn inside_bubble(&self, point: Point) -> bool {
        self.bubble.inset(-self.margin * 0.25).contains(point)
    }
}

fn animated_icon_rect(base: Rect, animation: IconAnimation) -> Rect {
    if let Some(scale) = animation.scale {
        let scale = scale.clamp(0.2, 3.0);
        let width = (base.width * scale).max(1.0);
        let height = (base.height * scale).max(1.0);
        let center = base.center();
        Rect::new(
            center.x - width / 2.0 + animation.offset_x,
            center.y - height / 2.0 + animation.offset_y,
            width,
            height,
        )
    } else {
        base.translate(animation.offset_x, animation.offset_y)
    }
}

fn draw_status_dot(pixmap: &mut Pixmap, icon: Rect, dpi_scale: f32, color: dock_core::Color) {
    let radius = 2.0 * dpi_scale;
    let center = (icon.center().x, icon.bottom() + 5.0 * dpi_scale + radius);
    painter::filled_circle(pixmap, center, radius, painter::color(color));
}

/// A menu render input.
pub struct MenuScene<'a> {
    /// Menu entries.
    pub entries: &'a [MenuEntry],
    /// Pre-measured layout.
    pub layout: &'a MenuLayout,
    /// Hovered entry index.
    pub hovered: Option<usize>,
    /// Palette.
    pub palette: Palette,
}

/// A rendered menu frame plus its layout.
pub struct MenuFrame {
    /// Rendered pixels.
    pub pixmap: Pixmap,
    /// Layout used to draw them.
    pub layout: MenuLayout,
}

struct ShadowCache {
    key: ShadowKey,
    pixmap: Pixmap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShadowKey {
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: u32,
    rect_height: u32,
    radius: u32,
    blur: u32,
    offset: i32,
    color: u32,
    hollow: bool,
}

/// Start-menu artwork style selected from the host Windows version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartMenuStyle {
    /// Windows 10 taskbar: four white panes.
    Windows10,
    /// Windows 11 taskbar: four blue panes.
    #[default]
    Windows11,
}

/// The Glide-Dock renderer. Owns the loaded fonts and the shadow cache.
pub struct Renderer {
    fonts: FontStack,
    shadow_cache: Option<ShadowCache>,
    start_style: StartMenuStyle,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// Creates a renderer with the system fonts.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fonts: FontStack::system(),
            shadow_cache: None,
            start_style: StartMenuStyle::Windows11,
        }
    }

    /// Selects the Windows 10/11 Start artwork style.
    pub fn set_start_style(&mut self, style: StartMenuStyle) {
        self.start_style = style;
    }

    /// Creates a renderer with an explicit font stack.
    #[must_use]
    pub fn with_fonts(fonts: FontStack) -> Self {
        Self {
            fonts,
            shadow_cache: None,
            start_style: StartMenuStyle::Windows11,
        }
    }

    /// Access to the loaded fonts (for menu measurement and previews).
    #[must_use]
    pub fn fonts(&self) -> &FontStack {
        &self.fonts
    }

    /// Renders the dock into a transparent window-sized pixmap.
    #[must_use]
    pub fn render_dock(&mut self, scene: &DockScene<'_>) -> Option<Pixmap> {
        let width = scene.geometry.window.width.ceil().max(1.0) as u32;
        let height = scene.geometry.window.height.ceil().max(1.0) as u32;
        let mut pixmap = Pixmap::new(width, height)?;
        let dock = scene.layout.dock;
        let radius = scene.geometry.radius;

        if !(scene.glass && scene.native_glass) {
            self.draw_dock_shadow(&mut pixmap, scene, dock, radius);
        }

        let (top, bottom) = scene
            .palette
            .dock_background(scene.config.appearance.opacity);
        let frosted = scene.glass && scene.native_glass;
        if !frosted {
            painter::fill_rounded_vertical(
                &mut pixmap,
                dock,
                radius,
                painter::color(top),
                painter::color(bottom),
            );
        } else {
            // A layered window uses per-pixel alpha for hit testing. The
            // composition glass lives in a window below, so the dock layer
            // itself must keep at least alpha 1 over the glass area; otherwise
            // the whole bar is click-through and neither hover nor the
            // background context menu can ever fire.
            painter::fill_rounded(&mut pixmap, dock, radius, Color::from_rgba8(0, 0, 0, 1));
        }

        // Inner shading is only part of the gradient fallback; the native
        // composition glass carries its own tint and must stay transparent.
        if !frosted {
            let inner = dock.inset(1.0);
            painter::fill_rounded_vertical(
                &mut pixmap,
                inner,
                (radius - 1.0).max(0.0),
                Color::from_rgba8(0, 0, 0, 0),
                painter::color(scene.palette.dock_inner_shadow),
            );
        }

        if !frosted {
            painter::stroke_rounded(
                &mut pixmap,
                dock.inset(0.5),
                (radius - 0.5).max(0.0),
                1.0,
                painter::color(scene.palette.dock_border),
            );
        }

        if scene.geometry.is_empty() {
            self.draw_empty_state(&mut pixmap, scene, dock);
            return Some(pixmap);
        }

        self.draw_hover_backdrop(&mut pixmap, scene);
        self.draw_items(&mut pixmap, scene);
        self.draw_drag(&mut pixmap, scene);
        self.draw_tooltip(&mut pixmap, scene);
        self.draw_error(&mut pixmap, scene);
        Some(pixmap)
    }

    fn draw_dock_shadow(
        &mut self,
        pixmap: &mut Pixmap,
        scene: &DockScene<'_>,
        dock: Rect,
        radius: f32,
    ) {
        let scale = scene.metrics.scale;
        let blur = 15.0 * scale;
        let offset = 5.0 * scale;
        let color = scene.palette.shadow;
        let key = ShadowKey {
            width: pixmap.width(),
            height: pixmap.height(),
            x: (dock.x / 2.0).round() as i32,
            y: (dock.y / 2.0).round() as i32,
            rect_width: (dock.width / 2.0).round().max(1.0) as u32,
            rect_height: (dock.height / 2.0).round().max(1.0) as u32,
            radius: (radius / 2.0).round() as u32,
            blur: (blur / 2.0).round() as u32,
            offset: (offset / 2.0).round() as i32,
            color: u32::from_be_bytes([color.r, color.g, color.b, color.a]),
            hollow: scene.glass && scene.native_glass,
        };
        let cached = self
            .shadow_cache
            .as_ref()
            .filter(|cache| cache.key == key)
            .map(|cache| cache.pixmap.as_ref());
        let shadow_ref = match cached {
            Some(cached) => Some(cached),
            None => {
                let layer = if scene.glass && scene.native_glass {
                    painter::shadow_layer_outside(
                        pixmap.width(),
                        pixmap.height(),
                        dock,
                        radius,
                        blur,
                        offset,
                        color,
                    )
                } else {
                    painter::shadow_layer(
                        pixmap.width(),
                        pixmap.height(),
                        dock,
                        radius,
                        blur,
                        offset,
                        color,
                    )
                };
                if let Some(layer) = layer {
                    self.shadow_cache = Some(ShadowCache { key, pixmap: layer });
                    self.shadow_cache
                        .as_ref()
                        .map(|cache| cache.pixmap.as_ref())
                } else {
                    None
                }
            }
        };
        if let Some(shadow) = shadow_ref {
            let paint = PixmapPaint {
                opacity: 1.0,
                blend_mode: BlendMode::SourceOver,
                quality: FilterQuality::Nearest,
            };
            pixmap.draw_pixmap(0, 0, shadow, &paint, Transform::identity(), None);
        }
    }

    fn draw_hover_backdrop(&self, pixmap: &mut Pixmap, scene: &DockScene<'_>) {
        let scale = scene.metrics.scale;
        let Some(index) = scene.state.hovered else {
            return;
        };
        let Some(item) = scene.config.items.get(index) else {
            return;
        };
        if item.kind == dock_core::ItemKind::Separator {
            return;
        }
        let Some(icon) = scene.layout.icons.get(index) else {
            return;
        };
        let progress = scene.hover_progress.clamp(0.0, 1.0);
        if progress <= 0.01 {
            return;
        }
        let icon = animated_icon_rect(
            *icon,
            scene.animations.get(index).copied().unwrap_or_default(),
        );
        let backdrop = icon.inset(-3.5 * scale);
        let alpha = (f32::from(scene.palette.hover.a) * progress).round() as u8;
        painter::fill_rounded(
            pixmap,
            backdrop,
            13.0 * scale,
            painter::color(scene.palette.hover.with_alpha(alpha)),
        );
    }

    fn draw_items(&self, pixmap: &mut Pixmap, scene: &DockScene<'_>) {
        let scale = scene.metrics.scale;
        let drag_index = scene.drag.filter(|drag| drag.active).map(|drag| drag.index);
        for (index, item) in scene.config.items.iter().enumerate() {
            let Some(base) = scene.layout.icons.get(index).copied() else {
                break;
            };
            let animation = scene.animations.get(index).copied().unwrap_or_default();
            let rect = animated_icon_rect(base, animation);
            let opacity = if drag_index == Some(index) { 0.32 } else { 1.0 };
            if item.kind == dock_core::ItemKind::Separator {
                Self::draw_separator(
                    pixmap,
                    rect,
                    scene.palette.dock_border.with_alpha(110),
                    scale,
                    opacity,
                );
                continue;
            }
            if item.kind == dock_core::ItemKind::Widget {
                self.draw_start_widget(pixmap, rect, opacity);
                continue;
            }
            if animation.glow > 0.001 {
                painter::glow(
                    pixmap,
                    rect,
                    10.0 * scale,
                    scene.palette.motion_accent,
                    animation.glow,
                );
            }
            if let Some(ripple) = animation.ripple {
                let alpha = (ripple.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                painter::stroke_circle(
                    pixmap,
                    (rect.center().x, rect.center().y),
                    ripple.radius,
                    ripple.width,
                    painter::color(scene.palette.motion_accent.with_alpha(alpha)),
                );
            }
            self.draw_icon(pixmap, scene, item, rect, opacity, animation);

            if scene.config.behavior.show_running_indicator {
                if let Some(waiting) = animation.waiting {
                    draw_status_dot(
                        pixmap,
                        rect,
                        scale,
                        scene.palette.accent.scale_alpha(waiting * 0.8),
                    );
                } else if scene.state.is_running(item.id) && animation.running_dot_allowed {
                    draw_status_dot(pixmap, rect, scale, scene.palette.accent);
                }
            }
        }
    }

    fn draw_separator(
        pixmap: &mut Pixmap,
        rect: Rect,
        color: ThemeColor,
        scale: f32,
        opacity: f32,
    ) {
        let width = (1.5 * scale).max(1.0);
        let height = (rect.height * 0.58).max(1.0);
        let x = rect.center().x - width / 2.0;
        let y = rect.center().y - height / 2.0;
        painter::fill_rounded(
            pixmap,
            Rect::new(x, y, width, height),
            width / 2.0,
            painter::color(color.scale_alpha(opacity)),
        );
    }

    fn draw_start_widget(&self, pixmap: &mut Pixmap, rect: Rect, opacity: f32) {
        // Exact geometry from the supplied icons8 SVGs (48×48 viewBox):
        // Windows 11 = four #1976D2 squares; Windows 10 = four #00B0FF
        // slanted panes. No white app tile is drawn.
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let (color, polygons): (Color, &[&[(f32, f32)]]) = match self.start_style {
            StartMenuStyle::Windows10 => (
                Color::from_rgba8(0, 176, 255, alpha),
                &[
                    &[
                        (20.0, 25.026),
                        (5.011, 25.0),
                        (5.012, 37.744),
                        (20.0, 39.818),
                    ],
                    &[
                        (22.0, 25.03),
                        (22.0, 40.095),
                        (42.995, 43.0),
                        (43.0, 25.066),
                    ],
                    &[(20.0, 8.256), (5.0, 10.38), (5.014, 23.0), (20.0, 23.0)],
                    &[(22.0, 7.973), (22.0, 23.0), (42.995, 23.0), (42.995, 5.0)],
                ],
            ),
            StartMenuStyle::Windows11 => (
                Color::from_rgba8(25, 118, 210, alpha),
                &[
                    &[(6.0, 6.0), (23.0, 6.0), (23.0, 23.0), (6.0, 23.0)],
                    &[(25.042, 6.0), (42.0, 6.0), (42.0, 22.958), (25.042, 22.958)],
                    &[(6.0, 25.0), (23.0, 25.0), (23.0, 42.0), (6.0, 42.0)],
                    &[(25.0, 25.0), (42.0, 25.0), (42.0, 42.0), (25.0, 42.0)],
                ],
            ),
        };
        let scale = rect.width / 48.0;
        for polygon in polygons {
            let points: Vec<(f32, f32)> = polygon
                .iter()
                .map(|(x, y)| (rect.x + x * scale, rect.y + y * scale))
                .collect();
            painter::filled_polygon(pixmap, &points, color);
        }
    }

    fn draw_icon(
        &self,
        pixmap: &mut Pixmap,
        scene: &DockScene<'_>,
        item: &DockItem,
        rect: Rect,
        opacity: f32,
        animation: IconAnimation,
    ) {
        if item.kind == dock_core::ItemKind::Widget {
            self.draw_start_widget(pixmap, rect, opacity);
            return;
        }
        if item.kind == dock_core::ItemKind::Separator {
            Self::draw_separator(
                pixmap,
                rect,
                scene.palette.dock_border.with_alpha(110),
                scene.metrics.scale,
                opacity,
            );
            return;
        }
        let unresolved = scene.state.is_unresolved(item.id);
        if let Some(icon) = scene.icons.icon(item) {
            draw_icon_pixmap(
                pixmap,
                &icon,
                rect,
                opacity,
                animation.tilt_x_deg,
                animation.tilt_y_deg,
            );
            if unresolved {
                draw_unresolved_badge(pixmap, rect);
            }
        } else {
            icons::draw_placeholder(pixmap, item, rect, &self.fonts, unresolved);
            if opacity < 0.99 {
                painter::fill_rounded(
                    pixmap,
                    rect,
                    rect.width * 0.23,
                    Color::from_rgba8(10, 10, 14, ((1.0 - opacity) * 190.0) as u8),
                );
            }
        }
    }

    fn draw_drag(&self, pixmap: &mut Pixmap, scene: &DockScene<'_>) {
        let scale = scene.metrics.scale;
        let Some(drag) = scene.drag else {
            return;
        };
        if !drag.active {
            return;
        }
        let Some(icon) = scene.config.items.get(drag.index) else {
            return;
        };
        if let Some(x) = drag.insertion_x {
            let height = scene.metrics.icon * 0.95;
            let y = scene.layout.dock.center().y - height / 2.0;
            let bar = Rect::new(x - 2.0 * scale, y, 4.0 * scale, height);
            painter::fill_rounded(
                pixmap,
                Rect::new(x - 7.0 * scale, y, 14.0 * scale, height),
                7.0 * scale,
                painter::color(scene.palette.accent.scale_alpha(0.22)),
            );
            painter::fill_rounded(
                pixmap,
                bar,
                2.0 * scale,
                painter::color(scene.palette.accent),
            );
        }

        let Some(icon_rect) = scene.layout.icons.get(drag.index).copied() else {
            return;
        };
        let size = icon_rect.width * 1.06;
        let floating = Rect::new(
            drag.cursor.x - drag.grab_dx * (size / icon_rect.width.max(1.0)),
            drag.cursor.y - size / 2.0 - 4.0 * scale,
            size,
            size,
        );
        painter::shadow(
            pixmap,
            floating.translate(0.0, 2.0 * scale),
            size * 0.22,
            10.0 * scale,
            5.0 * scale,
            ThemeColor::rgba(0, 0, 0, 130),
        );
        self.draw_icon(pixmap, scene, icon, floating, 1.0, IconAnimation::default());
    }

    fn draw_tooltip(&self, pixmap: &mut Pixmap, scene: &DockScene<'_>) {
        let scale = scene.metrics.scale;
        let progress = scene.tooltip_progress.clamp(0.0, 1.0);
        if progress <= 0.01 || !scene.config.behavior.show_tooltips {
            return;
        }
        let Some(index) = scene.state.tooltip.index else {
            return;
        };
        let Some(item) = scene.config.items.get(index) else {
            return;
        };
        if item.kind == dock_core::ItemKind::Separator {
            return;
        }
        let Some(icon) = scene.layout.icons.get(index).copied() else {
            return;
        };
        let font_size = 13.5 * scale;
        let max_text = 260.0 * scale;
        let label = self
            .fonts
            .ellipsize(item.label(), max_text, font_size, Weight::Regular);
        let text_width = self
            .fonts
            .measure(&label, font_size, Weight::Regular)
            .max(1.0);
        let mut bubble = tooltip_rect(icon, text_width, scene.geometry, scene.metrics);
        // Sit above the dock instead of overlapping its top edge by the
        // padding distance.
        bubble.y -= scene.metrics.padding;
        bubble.y = bubble.y.max(scene.metrics.shadow * 0.25);
        bubble.y -= (1.0 - progress) * 6.0 * scale;

        let shadow_alpha = (f32::from(scene.palette.shadow.a) * progress).round() as u8;
        painter::shadow(
            pixmap,
            bubble,
            bubble.height / 2.0,
            10.0 * scale,
            3.0 * scale,
            ThemeColor::rgba(
                scene.palette.shadow.r,
                scene.palette.shadow.g,
                scene.palette.shadow.b,
                shadow_alpha,
            ),
        );

        let bg = fade(scene.palette.tooltip_bg, progress);
        let border = fade(scene.palette.tooltip_border, progress);
        let text_color = scene.palette.tooltip_text.scale_alpha(progress);
        painter::fill_rounded(pixmap, bubble, bubble.height / 2.0, bg);
        painter::stroke_rounded(pixmap, bubble.inset(0.5), bubble.height / 2.0, 1.0, border);

        let arrow_center = icon.center().x.clamp(
            bubble.left() + bubble.height * 0.45,
            bubble.right() - bubble.height * 0.45,
        );
        painter::filled_polygon(
            pixmap,
            &[
                (arrow_center - 5.0 * scale, bubble.bottom() - 1.0),
                (arrow_center + 5.0 * scale, bubble.bottom() - 1.0),
                (arrow_center, bubble.bottom() + 4.5 * scale),
            ],
            bg,
        );

        self.fonts.draw_line_centered(
            pixmap,
            &label,
            bubble.center().x,
            bubble.center().y,
            font_size,
            text_color,
            Weight::Regular,
        );
    }

    fn draw_error(&self, pixmap: &mut Pixmap, scene: &DockScene<'_>) {
        let scale = scene.metrics.scale;
        let progress = scene.error_progress.clamp(0.0, 1.0);
        if progress <= 0.01 {
            return;
        }
        let Some(message) = scene.error else {
            return;
        };
        let font_size = 12.0 * scale;
        let max_width = scene
            .geometry
            .window
            .width
            .min(420.0 * scale)
            .max(120.0 * scale);
        let label = self.fonts.ellipsize(
            message,
            max_width - 26.0 * scale,
            font_size,
            Weight::Regular,
        );
        let text_width = self
            .fonts
            .measure(&label, font_size, Weight::Regular)
            .max(40.0);
        let height = 30.0 * scale;
        let width = (text_width + 24.0 * scale)
            .min(scene.geometry.window.width - 2.0 * scene.metrics.shadow)
            .max(80.0 * scale);
        let x = (scene.layout.dock.center().x - width / 2.0).clamp(
            scene.metrics.shadow,
            (scene.geometry.window.width - scene.metrics.shadow - width).max(scene.metrics.shadow),
        );
        let y = scene.layout.dock.top() - 12.0 * scale - height + (1.0 - progress) * 6.0 * scale;
        let bubble = Rect::new(x, y, width, height);

        painter::shadow(
            pixmap,
            bubble,
            height / 2.0,
            12.0 * scale,
            4.0 * scale,
            ThemeColor::rgba(0, 0, 0, (120.0 * progress) as u8),
        );
        painter::fill_rounded(
            pixmap,
            bubble,
            height / 2.0,
            fade(scene.palette.error_bg, progress),
        );
        painter::stroke_rounded(
            pixmap,
            bubble.inset(0.5),
            height / 2.0,
            1.0,
            fade(scene.palette.error_text.scale_alpha(0.35), progress),
        );
        self.fonts.draw_line_centered(
            pixmap,
            &label,
            bubble.center().x,
            bubble.center().y,
            font_size,
            scene.palette.error_text.scale_alpha(progress),
            Weight::Regular,
        );
    }

    fn draw_empty_state(&self, pixmap: &mut Pixmap, scene: &DockScene<'_>, dock: Rect) {
        let scale = scene.metrics.scale;
        let radius = scene.geometry.radius;
        painter::stroke_rounded(
            pixmap,
            dock.inset(6.0 * scale),
            (radius - 4.0 * scale).max(2.0),
            1.0,
            painter::color(scene.palette.empty_border.scale_alpha(0.8)),
        );

        let title = "拖入应用或快捷方式";
        let subtitle = "从资源管理器拖到这里";
        let title_size = 14.5 * scale;
        let subtitle_size = 11.5 * scale;
        let title_width = self.fonts.measure(title, title_size, Weight::Semibold);
        let subtitle_width = self.fonts.measure(subtitle, subtitle_size, Weight::Regular);
        let text_width = title_width.max(subtitle_width).max(1.0);
        let badge = 34.0 * scale;
        let gap = 13.0 * scale;
        let total = badge + gap + text_width;
        let start = dock.center().x - total / 2.0;
        let center_y = dock.center().y;
        let badge_rect = Rect::new(start, center_y - badge / 2.0, badge, badge);

        painter::fill_rounded(
            pixmap,
            badge_rect,
            badge * 0.31,
            painter::color(scene.palette.accent.scale_alpha(0.14)),
        );
        painter::stroke_rounded(
            pixmap,
            badge_rect.inset(0.5),
            badge * 0.31,
            1.0,
            painter::color(scene.palette.accent.scale_alpha(0.30)),
        );
        let plus_center = badge_rect.center();
        let arm = badge * 0.17;
        let plus_color = painter::color(scene.palette.accent);
        painter::line(
            pixmap,
            (plus_center.x - arm, plus_center.y),
            (plus_center.x + arm, plus_center.y),
            1.8 * scale,
            plus_color,
        );
        painter::line(
            pixmap,
            (plus_center.x, plus_center.y - arm),
            (plus_center.x, plus_center.y + arm),
            1.8 * scale,
            plus_color,
        );

        if title_width > 0.0 {
            let text_x = start + badge + gap;
            let title_baseline = self
                .fonts
                .centered_baseline(center_y - 9.0 * scale, title_size);
            self.fonts.draw(
                pixmap,
                title,
                text_x,
                title_baseline,
                title_size,
                scene.palette.text.scale_alpha(0.88),
                Weight::Semibold,
            );
            let subtitle_baseline = self
                .fonts
                .centered_baseline(center_y + 11.0 * scale, subtitle_size);
            self.fonts.draw(
                pixmap,
                subtitle,
                text_x,
                subtitle_baseline,
                subtitle_size,
                scene.palette.text_faint,
                Weight::Regular,
            );
        }
    }

    /// Measures the context menu for the given entries.
    #[must_use]
    pub fn menu_layout(&self, entries: &[MenuEntry], scale: f32) -> MenuLayout {
        menu_layout(&self.fonts, entries, scale)
    }

    /// Renders a context menu using a previously measured layout.
    #[must_use]
    pub fn render_menu(&self, scene: &MenuScene<'_>) -> Option<MenuFrame> {
        let layout = scene.layout;
        let width = layout.window.width.ceil().max(1.0) as u32;
        let height = layout.window.height.ceil().max(1.0) as u32;
        let mut pixmap = Pixmap::new(width, height)?;
        let scale = (layout.radius / 14.0).max(0.5);
        let bubble = layout.bubble;
        let palette = scene.palette;

        painter::shadow(
            &mut pixmap,
            bubble,
            layout.radius,
            16.0 * scale,
            6.0 * scale,
            palette.shadow,
        );
        painter::fill_rounded(
            &mut pixmap,
            bubble,
            layout.radius,
            painter::color(palette.menu_bg),
        );
        painter::fill_rounded_vertical(
            &mut pixmap,
            bubble,
            layout.radius,
            Color::from_rgba8(255, 255, 255, 10),
            Color::from_rgba8(0, 0, 0, 0),
        );
        painter::stroke_rounded(
            &mut pixmap,
            bubble.inset(0.5),
            (layout.radius - 0.5).max(0.0),
            1.0,
            painter::color(palette.menu_border),
        );
        let font_size = 13.5 * scale;
        for row in &layout.rows {
            let Some(entry) = scene.entries.get(row.entry) else {
                continue;
            };
            match entry {
                MenuEntry::Separator => {
                    let y = row.rect.center().y;
                    painter::line(
                        &mut pixmap,
                        (row.rect.left() + 10.0 * scale, y),
                        (row.rect.right() - 10.0 * scale, y),
                        1.0,
                        painter::color(palette.separator),
                    );
                }
                MenuEntry::Action {
                    label,
                    icon,
                    danger,
                    chevron,
                    checked,
                    ..
                } => {
                    let hovered = scene.hovered == Some(row.entry);
                    if hovered {
                        painter::fill_rounded(
                            &mut pixmap,
                            row.rect,
                            10.0 * scale,
                            painter::color(palette.menu_hover),
                        );
                    }
                    let text_color = if *danger {
                        palette.danger
                    } else {
                        palette.menu_text
                    };
                    let glyph_color = if *danger {
                        palette.danger
                    } else {
                        palette.menu_icon
                    };
                    let icon_box = Rect::new(
                        row.rect.left() + 12.0 * scale,
                        row.rect.center().y - 9.0 * scale,
                        18.0 * scale,
                        18.0 * scale,
                    );
                    painter::menu_glyph(
                        &mut pixmap,
                        if *checked {
                            MenuGlyph::Check
                        } else {
                            glyph_for(*icon)
                        },
                        icon_box,
                        painter::color(glyph_color),
                    );
                    let text_x = icon_box.right() + 12.0 * scale;
                    let chevron_space = if *chevron { 18.0 * scale } else { 0.0 };
                    let max_text =
                        (row.rect.right() - 10.0 * scale - chevron_space - text_x).max(10.0);
                    let label = self
                        .fonts
                        .ellipsize(label, max_text, font_size, Weight::Regular);
                    self.fonts.draw(
                        &mut pixmap,
                        &label,
                        text_x,
                        self.fonts.centered_baseline(row.rect.center().y, font_size),
                        font_size,
                        text_color,
                        Weight::Regular,
                    );
                    if *chevron {
                        let x = row.rect.right() - 16.0 * scale;
                        let y = row.rect.center().y;
                        painter::line(
                            &mut pixmap,
                            (x - 3.0 * scale, y - 4.0 * scale),
                            (x + 1.0 * scale, y),
                            1.4 * scale,
                            painter::color(glyph_color),
                        );
                        painter::line(
                            &mut pixmap,
                            (x + 1.0 * scale, y),
                            (x - 3.0 * scale, y + 4.0 * scale),
                            1.4 * scale,
                            painter::color(glyph_color),
                        );
                    }
                }
            }
        }

        Some(MenuFrame {
            pixmap,
            layout: layout.clone(),
        })
    }
}

fn draw_icon_pixmap(
    pixmap: &mut Pixmap,
    icon: &Arc<Pixmap>,
    rect: Rect,
    opacity: f32,
    tilt_x_deg: f32,
    tilt_y_deg: f32,
) {
    let source: PixmapRef = icon.as_ref().as_ref();
    let source_width = source.width() as f32;
    let source_height = source.height() as f32;
    if source_width <= 0.0 || source_height <= 0.0 {
        return;
    }
    // `GetImage` preserves the source aspect ratio; stretch it into the square
    // slot only when the source is square, otherwise letterbox it so wide or
    // tall icons are not distorted.
    let scale = (rect.width / source_width).min(rect.height / source_height);
    let width = source_width * scale;
    let height = source_height * scale;
    let target = Rect::new(
        rect.center().x - width / 2.0,
        rect.center().y - height / 2.0,
        width,
        height,
    );
    painter::draw_scaled_tilted(pixmap, source, target, opacity, tilt_x_deg, tilt_y_deg);
}

fn fade(value: ThemeColor, progress: f32) -> Color {
    painter::color(value.scale_alpha(progress.clamp(0.0, 1.0)))
}

fn glyph_for(icon: MenuIcon) -> MenuGlyph {
    match icon {
        MenuIcon::Open => MenuGlyph::Open,
        MenuIcon::Folder => MenuGlyph::Folder,
        MenuIcon::Trash => MenuGlyph::Trash,
        MenuIcon::Settings => MenuGlyph::Settings,
        MenuIcon::Refresh => MenuGlyph::Refresh,
        MenuIcon::Edit => MenuGlyph::Edit,
        MenuIcon::Power => MenuGlyph::Power,
        MenuIcon::Appearance => MenuGlyph::Appearance,
        MenuIcon::Layers => MenuGlyph::Layers,
        MenuIcon::Tips => MenuGlyph::Tips,
        MenuIcon::Check => MenuGlyph::Check,
        MenuIcon::Blank => MenuGlyph::Blank,
        MenuIcon::Desktop => MenuGlyph::Desktop,
        MenuIcon::Window => MenuGlyph::Window,
        MenuIcon::Maximize => MenuGlyph::Maximize,
        MenuIcon::Start => MenuGlyph::Start,
        MenuIcon::Search => MenuGlyph::Search,
        MenuIcon::TaskView => MenuGlyph::TaskView,
    }
}

/// Measures a context menu.
#[must_use]
pub fn menu_layout(fonts: &FontStack, entries: &[MenuEntry], scale: f32) -> MenuLayout {
    let scale = scale.clamp(0.5, 4.0);
    let font_size = 13.5 * scale;
    let margin = 22.0 * scale;
    let padding_x = 14.0 * scale;
    let icon_size = 18.0 * scale;
    let icon_gap = 12.0 * scale;
    let row_height = 36.0 * scale;
    let separator_height = 11.0 * scale;

    let mut text_width: f32 = 0.0;
    let mut content_height = 0.0;
    for entry in entries {
        match entry {
            MenuEntry::Action { label, chevron, .. } => {
                let reserve = if *chevron { 18.0 * scale } else { 0.0 };
                text_width =
                    text_width.max(fonts.measure(label, font_size, Weight::Regular) + reserve);
                content_height += row_height;
            }
            MenuEntry::Separator => content_height += separator_height,
        }
    }
    let bubble_width = (padding_x * 2.0 + icon_size + icon_gap + text_width).max(220.0 * scale);
    let bubble_height = content_height + 14.0 * scale;

    let bubble = Rect::new(margin, margin, bubble_width, bubble_height);
    let mut rows = Vec::with_capacity(entries.len());
    let mut y = bubble.top() + 7.0 * scale;
    for (index, entry) in entries.iter().enumerate() {
        let height = match entry {
            MenuEntry::Action { .. } => row_height,
            MenuEntry::Separator => separator_height,
        };
        let rect = Rect::new(
            bubble.left() + 6.0 * scale,
            y,
            bubble.width - 12.0 * scale,
            height,
        );
        rows.push(MenuRow { rect, entry: index });
        y += height;
    }

    MenuLayout {
        window: Size::new(bubble_width + margin * 2.0, bubble_height + margin * 2.0),
        bubble,
        rows,
        radius: 14.0 * scale,
        margin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dock_core::models::DockConfig;
    use dock_core::{DockItem, Metrics};
    use icons::NoIcons;

    fn scene_config() -> DockConfig {
        let mut config = DockConfig::default();
        for index in 0..5 {
            config.items.push(DockItem::application(
                index + 1,
                format!("App {index}"),
                format!("C:\\Apps\\app{index}.exe"),
            ));
        }
        config
    }

    fn render_sample() -> Pixmap {
        let config = scene_config();
        let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 1.0);
        let geometry = DockGeometry::new(config.items.len(), &metrics);
        let layout = FrameLayout::compute(&geometry, &metrics, &[1.0; 5]);
        let state = DockState::default();
        let mut renderer = Renderer::new();
        let scene = DockScene {
            config: &config,
            metrics: &metrics,
            geometry: &geometry,
            layout: &layout,
            animations: &[],
            state: &state,
            palette: Palette::dark(),
            icons: &NoIcons,
            native_glass: false,
            tooltip_progress: 0.0,
            hover_progress: 0.0,
            error: None,
            error_progress: 0.0,
            drag: None,
            glass: false,
        };
        renderer.render_dock(&scene).expect("render")
    }

    #[test]
    fn start_widget_and_separator_draw_pixels() {
        let mut pixmap = Pixmap::new(64, 64).expect("pixmap");
        let rect = Rect::new(8.0, 8.0, 48.0, 48.0);
        let mut renderer = Renderer::new();
        renderer.draw_start_widget(&mut pixmap, rect, 1.0);
        assert!(pixmap.pixels().iter().any(|pixel| pixel.blue() > 150));

        let mut separator = Pixmap::new(64, 64).expect("pixmap");
        Renderer::draw_separator(
            &mut separator,
            rect,
            ThemeColor::rgba(255, 255, 255, 180),
            1.0,
            1.0,
        );
        assert!(separator.pixels().iter().any(|pixel| pixel.alpha() > 0));

        let mut win10 = Pixmap::new(64, 64).expect("pixmap");
        renderer.set_start_style(StartMenuStyle::Windows10);
        renderer.draw_start_widget(&mut win10, rect, 1.0);
        assert!(
            win10
                .pixels()
                .iter()
                .any(|pixel| pixel.green() > 140 && pixel.blue() > 200)
        );
    }

    #[test]
    fn renders_opaque_dock_pixels() {
        let pixmap = render_sample();
        assert!(pixmap.width() > 100);
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 240));
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() == 0));
    }

    #[test]
    fn empty_state_renders_hint() {
        let config = DockConfig::default();
        let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 1.0);
        let geometry = DockGeometry::new(0, &metrics);
        let layout = FrameLayout::compute(&geometry, &metrics, &[]);
        let state = DockState::default();
        let mut renderer = Renderer::new();
        let scene = DockScene {
            config: &config,
            metrics: &metrics,
            geometry: &geometry,
            layout: &layout,
            animations: &[],
            state: &state,
            palette: Palette::dark(),
            icons: &NoIcons,
            native_glass: false,
            tooltip_progress: 0.0,
            hover_progress: 0.0,
            error: None,
            error_progress: 0.0,
            drag: None,
            glass: false,
        };
        let pixmap = renderer.render_dock(&scene).expect("render");
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 200));
    }

    #[test]
    fn menu_layout_measures_rows() {
        let renderer = Renderer::new();
        let entries = vec![
            MenuEntry::action("Open", dock_core::MenuAction::Activate(1), MenuIcon::Open),
            MenuEntry::Separator,
            MenuEntry::danger("Remove", dock_core::MenuAction::Remove(1), MenuIcon::Trash),
        ];
        let layout = renderer.menu_layout(&entries, 1.0);
        assert_eq!(layout.rows.len(), 3);
        assert!(layout.window.width > layout.bubble.width);
        assert!(layout.rows[1].rect.height < layout.rows[0].rect.height);
        let hit = layout.hit(layout.rows[0].rect.center());
        assert_eq!(hit, Some(0));
        assert!(layout.inside_bubble(layout.bubble.center()));
    }

    #[test]
    fn menu_renders_with_hover() {
        let renderer = Renderer::new();
        let entries = vec![
            MenuEntry::action("Open", dock_core::MenuAction::Activate(1), MenuIcon::Open),
            MenuEntry::action(
                "Show in Explorer",
                dock_core::MenuAction::OpenLocation(1),
                MenuIcon::Folder,
            ),
        ];
        let layout = renderer.menu_layout(&entries, 1.0);
        let scene = MenuScene {
            entries: &entries,
            layout: &layout,
            hovered: Some(1),
            palette: Palette::dark(),
        };
        let frame = renderer.render_menu(&scene).expect("menu");
        assert_eq!(frame.pixmap.width(), layout.window.width.ceil() as u32);
        assert!(
            frame
                .pixmap
                .pixels()
                .iter()
                .any(|pixel| pixel.alpha() > 200)
        );
    }

    #[test]
    fn appearance_and_behavior_do_not_break_render() {
        let mut config = scene_config();
        config.appearance.opacity = 0.95;
        config.behavior.show_tooltips = false;
        let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 2.0);
        let geometry = DockGeometry::new(config.items.len(), &metrics);
        let layout = FrameLayout::compute(&geometry, &metrics, &[1.0; 5]);
        let state = DockState::default();
        let mut renderer = Renderer::new();
        let scene = DockScene {
            config: &config,
            metrics: &metrics,
            geometry: &geometry,
            layout: &layout,
            animations: &[],
            state: &state,
            palette: Palette::light(),
            icons: &NoIcons,
            native_glass: false,
            tooltip_progress: 0.0,
            hover_progress: 0.0,
            error: None,
            error_progress: 0.0,
            drag: None,
            glass: false,
        };
        let pixmap = renderer.render_dock(&scene).expect("render");
        assert!(pixmap.width() > 500);
    }

    #[test]
    fn menu_layout_min_width() {
        let fonts = FontStack::empty();
        let layout = menu_layout(&fonts, &[], 1.0);
        assert!(layout.bubble.width >= 200.0);
        assert!(layout.rows.is_empty());
    }
}
