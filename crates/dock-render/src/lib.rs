//! Glide-Dock scene preparation and tiny-skia renderer for live and offline frames.

#![warn(missing_docs)]

mod frame;
pub mod icons;
pub mod painter;
pub mod preview;
pub mod text;
pub mod text_backend;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub use frame::{DockFrame, DrawCommand, HitShape, MenuOverlay};
pub use text::Weight;
pub use text_backend::TextBackend;

use dock_core::layout::tooltip_rect;
use dock_core::models::DockConfig;
use dock_core::state::{MenuEntry, MenuIcon};
use dock_core::theme::{Color as ThemeColor, Palette};
use dock_core::{
    DockGeometry, DockItem, DockState, FrameLayout, Metrics, Point, Rect, RippleMotion, Size,
};
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::icons::IconSource;
use crate::painter::MenuGlyph;
use crate::text::FontStack;

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

fn draw_status_dot(frame: &mut DockFrame, icon: Rect, dpi_scale: f32, color: dock_core::Color) {
    let radius = 2.0 * dpi_scale;
    let center = (icon.center().x, icon.bottom() + 5.0 * dpi_scale + radius);
    frame.filled_circle(center, radius, color);
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
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum RasterKey {
    Text {
        text: String,
        size: u32,
        color: [u8; 3],
        weight: Weight,
    },
    Placeholder {
        id: u32,
        kind: u8,
        label: String,
        path: String,
        unresolved: bool,
    },
}

#[derive(Default)]
struct RasterCache {
    assets: HashMap<RasterKey, Arc<Pixmap>>,
    order: VecDeque<RasterKey>,
}

impl RasterCache {
    fn get_or_render(
        &mut self,
        key: RasterKey,
        render: impl FnOnce() -> Option<Pixmap>,
    ) -> Option<Arc<Pixmap>> {
        if let Some(pixels) = self.assets.get(&key) {
            return Some(Arc::clone(pixels));
        }
        let pixels = Arc::new(render()?);
        if self.order.len() >= 32
            && let Some(oldest) = self.order.pop_front()
        {
            self.assets.remove(&oldest);
        }
        self.order.push_back(key.clone());
        self.assets.insert(key, Arc::clone(&pixels));
        Some(pixels)
    }
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

impl RasterCache {
    fn clear(&mut self) {
        self.assets.clear();
        self.order.clear();
    }
}

/// The Glide-Dock renderer. Owns the active text backend and render caches.
pub struct Renderer {
    text: Arc<dyn TextBackend>,
    shadow_cache: Option<ShadowCache>,
    start_style: StartMenuStyle,
    raster_cache: RasterCache,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// Creates a renderer with the built-in system font stack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            text: Arc::new(FontStack::system()),
            shadow_cache: None,
            start_style: StartMenuStyle::Windows11,
            raster_cache: RasterCache::default(),
        }
    }

    /// Creates a renderer backed by a custom [`TextBackend`] without loading
    /// font files into heap memory.
    #[must_use]
    pub fn with_text_backend(text: Arc<dyn TextBackend>) -> Self {
        Self {
            text,
            shadow_cache: None,
            start_style: StartMenuStyle::Windows11,
            raster_cache: RasterCache::default(),
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
            text: Arc::new(fonts),
            shadow_cache: None,
            start_style: StartMenuStyle::Windows11,
            raster_cache: RasterCache::default(),
        }
    }

    /// Releases cached shadow and glyph rasters when settling into idle.
    pub fn trim_caches(&mut self) {
        self.shadow_cache = None;
        self.raster_cache.clear();
    }

    /// Replaces the active text backend and clears cached glyph rasters.
    pub fn set_text_backend(&mut self, text: Arc<dyn TextBackend>) {
        self.text = text;
        self.raster_cache.clear();
    }

    /// Access to the active text backend.
    #[must_use]
    pub fn fonts(&self) -> &dyn TextBackend {
        &*self.text
    }

    /// Renders the dock into a transparent window-sized pixmap.
    #[must_use]
    pub fn render_dock(&mut self, scene: &DockScene<'_>) -> Option<Pixmap> {
        let frame = self.prepare_frame(scene, None);
        self.render_frame(&frame)
    }

    /// Builds the ordered draw commands and the input geometry once per frame.
    #[must_use]
    pub fn prepare_frame(
        &mut self,
        scene: &DockScene<'_>,
        menu: Option<MenuOverlay<'_>>,
    ) -> DockFrame {
        let width = scene.geometry.window.width.ceil().max(1.0) as u32;
        let height = scene.geometry.window.height.ceil().max(1.0) as u32;
        let mut frame = DockFrame::new(width, height);
        let dock = scene.layout.dock;
        let radius = scene.geometry.radius;
        let margin = 6.0 * scene.metrics.scale;
        frame.hit_shape(dock.inset(-margin), radius + margin);
        let frosted = scene.glass && scene.native_glass;
        if frosted {
            // Keep the CPU layered surface interactive over native glass.
            frame.fill_rounded(dock, radius, ThemeColor::rgba(0, 0, 0, 1));
        } else {
            let scale = scene.metrics.scale;
            frame.commands.push(DrawCommand::Shadow {
                rect: dock,
                radius,
                blur: 15.0 * scale,
                offset: 5.0 * scale,
                color: scene.palette.shadow,
                dock: true,
            });
            let (top, bottom) = scene
                .palette
                .dock_background(scene.config.appearance.opacity);
            frame.fill_rounded_vertical(dock, radius, top, bottom);
            frame.fill_rounded_vertical(
                dock.inset(1.0),
                (radius - 1.0).max(0.0),
                ThemeColor::TRANSPARENT,
                scene.palette.dock_inner_shadow,
            );
            frame.stroke_rounded(
                dock.inset(0.5),
                (radius - 0.5).max(0.0),
                1.0,
                scene.palette.dock_border,
            );
        }

        if scene.geometry.is_empty() {
            self.draw_empty_state(&mut frame, scene, dock);
        } else {
            self.draw_hover_backdrop(&mut frame, scene);
            self.draw_items(&mut frame, scene);
            self.draw_drag(&mut frame, scene);
            self.draw_tooltip(&mut frame, scene);
            self.draw_error(&mut frame, scene);
        }
        if let Some(menu) = menu {
            if let Some(hit) = menu.hit {
                frame.hit_shapes.push(hit);
            }
            frame.bitmap(
                Arc::clone(menu.pixels),
                Transform::from_translate(menu.position.x, menu.position.y),
                menu.opacity,
            );
        }
        frame
    }

    /// Executes the prepared frame with tiny-skia.
    #[must_use]
    pub fn render_frame(&mut self, frame: &DockFrame) -> Option<Pixmap> {
        let mut pixmap = Pixmap::new(frame.width, frame.height)?;
        for command in &frame.commands {
            match *command {
                DrawCommand::Fill {
                    rect,
                    radius,
                    color,
                } => painter::fill_rounded(&mut pixmap, rect, radius, painter::color(color)),
                DrawCommand::Vertical {
                    rect,
                    radius,
                    top,
                    bottom,
                } => painter::fill_rounded_vertical(
                    &mut pixmap,
                    rect,
                    radius,
                    painter::color(top),
                    painter::color(bottom),
                ),
                DrawCommand::Stroke {
                    rect,
                    radius,
                    width,
                    color,
                } => {
                    painter::stroke_rounded(&mut pixmap, rect, radius, width, painter::color(color))
                }
                DrawCommand::Line {
                    from,
                    to,
                    width,
                    color,
                } => painter::line(&mut pixmap, from, to, width, painter::color(color)),
                DrawCommand::Circle {
                    center,
                    radius,
                    stroke: Some(width),
                    color,
                } => painter::stroke_circle(
                    &mut pixmap,
                    center,
                    radius,
                    width,
                    painter::color(color),
                ),
                DrawCommand::Circle {
                    center,
                    radius,
                    stroke: None,
                    color,
                } => painter::filled_circle(&mut pixmap, center, radius, painter::color(color)),
                DrawCommand::Polygon { ref points, color } => {
                    painter::filled_polygon(&mut pixmap, points, painter::color(color))
                }
                DrawCommand::Glow {
                    rect,
                    spread,
                    color,
                    strength,
                } => painter::glow(&mut pixmap, rect, spread, color, strength),
                DrawCommand::Shadow {
                    rect,
                    radius,
                    blur,
                    offset,
                    color,
                    dock,
                } => {
                    if dock {
                        self.draw_dock_shadow(&mut pixmap, rect, radius, blur, offset, color);
                    } else {
                        painter::shadow(&mut pixmap, rect, radius, blur, offset, color);
                    }
                }
                DrawCommand::Bitmap {
                    ref pixels,
                    transform,
                    opacity,
                } => {
                    let paint = PixmapPaint {
                        opacity,
                        quality: if transform.is_translate() {
                            FilterQuality::Nearest
                        } else {
                            FilterQuality::Bilinear
                        },
                        ..PixmapPaint::default()
                    };
                    pixmap.draw_pixmap(0, 0, pixels.as_ref().as_ref(), &paint, transform, None);
                }
            }
        }
        Some(pixmap)
    }

    fn draw_dock_shadow(
        &mut self,
        pixmap: &mut Pixmap,
        dock: Rect,
        radius: f32,
        blur: f32,
        offset: f32,
        color: ThemeColor,
    ) {
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
        };
        if self
            .shadow_cache
            .as_ref()
            .is_none_or(|cache| cache.key != key)
        {
            self.shadow_cache = painter::shadow_layer(
                pixmap.width(),
                pixmap.height(),
                dock,
                radius,
                blur,
                offset,
                color,
            )
            .map(|pixmap| ShadowCache { key, pixmap });
        }
        if let Some(shadow) = &self.shadow_cache {
            pixmap.draw_pixmap(
                0,
                0,
                shadow.pixmap.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }

    fn draw_hover_backdrop(&self, frame: &mut DockFrame, scene: &DockScene<'_>) {
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
        frame.fill_rounded(
            backdrop,
            13.0 * scale,
            scene.palette.hover.with_alpha(alpha),
        );
    }

    fn draw_items(&mut self, frame: &mut DockFrame, scene: &DockScene<'_>) {
        let scale = scene.metrics.scale;
        let drag_index = scene.drag.filter(|drag| drag.active).map(|drag| drag.index);
        for (index, item) in scene.config.items.iter().enumerate() {
            let Some(base) = scene.layout.icons.get(index).copied() else {
                break;
            };
            let animation = scene.animations.get(index).copied().unwrap_or_default();
            let rect = animated_icon_rect(base, animation);
            frame.hit_shape(base, 0.0);
            let opacity = if drag_index == Some(index) { 0.32 } else { 1.0 };
            if item.kind == dock_core::ItemKind::Separator {
                Self::draw_separator(
                    frame,
                    rect,
                    scene.palette.dock_border.with_alpha(110),
                    scale,
                    opacity,
                );
                continue;
            }
            if item.kind == dock_core::ItemKind::Widget {
                self.draw_start_widget(frame, rect, opacity);
                continue;
            }
            if animation.glow > 0.001 {
                frame.glow(
                    rect,
                    10.0 * scale,
                    scene.palette.motion_accent,
                    animation.glow,
                );
            }
            if let Some(ripple) = animation.ripple {
                let alpha = (ripple.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                frame.stroke_circle(
                    (rect.center().x, rect.center().y),
                    ripple.radius,
                    ripple.width,
                    scene.palette.motion_accent.with_alpha(alpha),
                );
            }
            self.draw_icon(frame, scene, item, rect, opacity, animation);

            if scene.config.behavior.show_running_indicator {
                if let Some(waiting) = animation.waiting {
                    draw_status_dot(
                        frame,
                        rect,
                        scale,
                        scene.palette.accent.scale_alpha(waiting * 0.8),
                    );
                } else if scene.state.is_running(item.id) && animation.running_dot_allowed {
                    draw_status_dot(frame, rect, scale, scene.palette.accent);
                }
            }
        }
    }

    fn draw_separator(
        frame: &mut DockFrame,
        rect: Rect,
        color: ThemeColor,
        scale: f32,
        opacity: f32,
    ) {
        let width = (1.5 * scale).max(1.0);
        let height = (rect.height * 0.58).max(1.0);
        let x = rect.center().x - width / 2.0;
        let y = rect.center().y - height / 2.0;
        frame.fill_rounded(
            Rect::new(x, y, width, height),
            width / 2.0,
            color.scale_alpha(opacity),
        );
    }

    fn draw_start_widget(&self, frame: &mut DockFrame, rect: Rect, opacity: f32) {
        // Exact geometry from the supplied icons8 SVGs (48×48 viewBox):
        // Windows 11 = four #1976D2 squares; Windows 10 = four #00B0FF
        // slanted panes. No white app tile is drawn.
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let (color, polygons): (ThemeColor, &[&[(f32, f32)]]) = match self.start_style {
            StartMenuStyle::Windows10 => (
                ThemeColor::rgba(0, 176, 255, alpha),
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
                ThemeColor::rgba(25, 118, 210, alpha),
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
            frame.filled_polygon(&points, color);
        }
    }

    fn draw_icon(
        &mut self,
        frame: &mut DockFrame,
        scene: &DockScene<'_>,
        item: &DockItem,
        rect: Rect,
        opacity: f32,
        animation: IconAnimation,
    ) {
        if item.kind == dock_core::ItemKind::Widget {
            self.draw_start_widget(frame, rect, opacity);
            return;
        }
        if item.kind == dock_core::ItemKind::Separator {
            Self::draw_separator(
                frame,
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
                frame,
                icon,
                rect,
                opacity,
                animation.tilt_x_deg,
                animation.tilt_y_deg,
            );
            if unresolved {
                draw_badge(frame, rect);
            }
        } else {
            // Missing/extracting icons are still reusable textures.
            let key = RasterKey::Placeholder {
                id: item.id,
                kind: item.kind as u8,
                label: item.label().to_owned(),
                path: item.target_path.clone(),
                unresolved,
            };
            if let Some(pixels) = self.raster_cache.get_or_render(key, || {
                let mut pixels = Pixmap::new(128, 128)?;
                icons::draw_placeholder(
                    &mut pixels,
                    item,
                    Rect::new(0.0, 0.0, 128.0, 128.0),
                    &*self.text,
                    unresolved,
                );
                Some(pixels)
            }) {
                draw_icon_pixmap(frame, pixels, rect, 1.0, 0.0, 0.0);
            }
            if opacity < 0.99 {
                frame.fill_rounded(
                    rect,
                    rect.width * 0.23,
                    ThemeColor::rgba(10, 10, 14, ((1.0 - opacity) * 190.0) as u8),
                );
            }
        }
    }

    fn draw_drag(&mut self, frame: &mut DockFrame, scene: &DockScene<'_>) {
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
            frame.fill_rounded(
                Rect::new(x - 7.0 * scale, y, 14.0 * scale, height),
                7.0 * scale,
                scene.palette.accent.scale_alpha(0.22),
            );
            frame.fill_rounded(bar, 2.0 * scale, scene.palette.accent);
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
        frame.shadow(
            floating.translate(0.0, 2.0 * scale),
            size * 0.22,
            10.0 * scale,
            5.0 * scale,
            ThemeColor::rgba(0, 0, 0, 130),
        );
        self.draw_icon(frame, scene, icon, floating, 1.0, IconAnimation::default());
    }

    fn draw_tooltip(&mut self, frame: &mut DockFrame, scene: &DockScene<'_>) {
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
            .text
            .ellipsize(item.label(), max_text, font_size, Weight::Regular);
        let text_width = self
            .text
            .measure(&label, font_size, Weight::Regular)
            .max(1.0);
        let mut bubble = tooltip_rect(icon, text_width, scene.geometry, scene.metrics);
        // Sit above the dock instead of overlapping its top edge by the
        // padding distance.
        bubble.y -= scene.metrics.padding;
        bubble.y = bubble.y.max(scene.metrics.shadow * 0.25);
        bubble.y -= (1.0 - progress) * 6.0 * scale;

        let shadow_alpha = (f32::from(scene.palette.shadow.a) * progress).round() as u8;
        frame.shadow(
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

        let bg = scene.palette.tooltip_bg.scale_alpha(progress);
        let border = scene.palette.tooltip_border.scale_alpha(progress);
        let text_color = scene.palette.tooltip_text.scale_alpha(progress);
        frame.fill_rounded(bubble, bubble.height / 2.0, bg);
        frame.stroke_rounded(bubble.inset(0.5), bubble.height / 2.0, 1.0, border);

        let arrow_center = icon.center().x.clamp(
            bubble.left() + bubble.height * 0.45,
            bubble.right() - bubble.height * 0.45,
        );
        frame.filled_polygon(
            &[
                (arrow_center - 5.0 * scale, bubble.bottom() - 1.0),
                (arrow_center + 5.0 * scale, bubble.bottom() - 1.0),
                (arrow_center, bubble.bottom() + 4.5 * scale),
            ],
            bg,
        );

        self.draw_cached_text_centered(
            frame,
            &label,
            bubble.center().x,
            bubble.center().y,
            font_size,
            text_color,
            Weight::Regular,
        );
    }

    fn draw_error(&mut self, frame: &mut DockFrame, scene: &DockScene<'_>) {
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
        let label = self.text.ellipsize(
            message,
            max_width - 26.0 * scale,
            font_size,
            Weight::Regular,
        );
        let text_width = self
            .text
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

        frame.shadow(
            bubble,
            height / 2.0,
            12.0 * scale,
            4.0 * scale,
            ThemeColor::rgba(0, 0, 0, (120.0 * progress) as u8),
        );
        frame.fill_rounded(
            bubble,
            height / 2.0,
            scene.palette.error_bg.scale_alpha(progress),
        );
        frame.stroke_rounded(
            bubble.inset(0.5),
            height / 2.0,
            1.0,
            scene.palette.error_text.scale_alpha(0.35 * progress),
        );
        self.draw_cached_text_centered(
            frame,
            &label,
            bubble.center().x,
            bubble.center().y,
            font_size,
            scene.palette.error_text.scale_alpha(progress),
            Weight::Regular,
        );
    }

    fn draw_empty_state(&mut self, frame: &mut DockFrame, scene: &DockScene<'_>, dock: Rect) {
        let scale = scene.metrics.scale;
        let radius = scene.geometry.radius;
        frame.stroke_rounded(
            dock.inset(6.0 * scale),
            (radius - 4.0 * scale).max(2.0),
            1.0,
            scene.palette.empty_border.scale_alpha(0.8),
        );

        let title = "拖入应用或快捷方式";
        let subtitle = "从资源管理器拖到这里";
        let title_size = 14.5 * scale;
        let subtitle_size = 11.5 * scale;
        let title_width = self.text.measure(title, title_size, Weight::Semibold);
        let subtitle_width = self.text.measure(subtitle, subtitle_size, Weight::Regular);
        let text_width = title_width.max(subtitle_width).max(1.0);
        let badge = 34.0 * scale;
        let gap = 13.0 * scale;
        let total = badge + gap + text_width;
        let start = dock.center().x - total / 2.0;
        let center_y = dock.center().y;
        let badge_rect = Rect::new(start, center_y - badge / 2.0, badge, badge);

        frame.fill_rounded(
            badge_rect,
            badge * 0.31,
            scene.palette.accent.scale_alpha(0.14),
        );
        frame.stroke_rounded(
            badge_rect.inset(0.5),
            badge * 0.31,
            1.0,
            scene.palette.accent.scale_alpha(0.30),
        );
        let plus_center = badge_rect.center();
        let arm = badge * 0.17;
        let plus_color = scene.palette.accent;
        frame.line(
            (plus_center.x - arm, plus_center.y),
            (plus_center.x + arm, plus_center.y),
            1.8 * scale,
            plus_color,
        );
        frame.line(
            (plus_center.x, plus_center.y - arm),
            (plus_center.x, plus_center.y + arm),
            1.8 * scale,
            plus_color,
        );

        if title_width > 0.0 {
            let text_x = start + badge + gap;
            let title_baseline = self
                .text
                .centered_baseline(center_y - 9.0 * scale, title_size);
            self.draw_cached_text(
                frame,
                title,
                text_x,
                title_baseline,
                title_size,
                scene.palette.text.scale_alpha(0.88),
                Weight::Semibold,
            );
            let subtitle_baseline = self
                .text
                .centered_baseline(center_y + 11.0 * scale, subtitle_size);
            self.draw_cached_text(
                frame,
                subtitle,
                text_x,
                subtitle_baseline,
                subtitle_size,
                scene.palette.text_faint,
                Weight::Regular,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cached_text_centered(
        &mut self,
        frame: &mut DockFrame,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: ThemeColor,
        weight: Weight,
    ) {
        let width = self.text.measure(text, size, weight);
        let baseline = self.text.centered_baseline(y, size);
        self.draw_cached_text(frame, text, x - width / 2.0, baseline, size, color, weight);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cached_text(
        &mut self,
        frame: &mut DockFrame,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: ThemeColor,
        weight: Weight,
    ) {
        if text.is_empty() || color.a == 0 || !self.text.is_available() {
            return;
        }
        let color_rgb = [color.r, color.g, color.b];
        let key = RasterKey::Text {
            text: text.to_owned(),
            size: size.to_bits(),
            color: color_rgb,
            weight,
        };
        // Allow room for glyph overhang and keep baselines on pixel boundaries.
        let pad = size.ceil() as u32;
        if let Some(pixels) = self.raster_cache.get_or_render(key, || {
            let width = self.text.measure(text, size, weight).ceil() as u32;
            let mut pixels = Pixmap::new(width + 2 * pad, 3 * pad)?;
            self.text.draw(
                &mut pixels,
                text,
                pad as f32,
                (2 * pad) as f32,
                size,
                color.with_alpha(255),
                weight,
            );
            Some(pixels)
        }) {
            let transform = Transform::from_translate(
                x.round() - pad as f32,
                baseline.round() - (2 * pad) as f32,
            );
            frame.bitmap(pixels, transform, f32::from(color.a) / 255.0);
        }
    }

    /// Measures the context menu for the given entries.
    #[must_use]
    pub fn menu_layout(&self, entries: &[MenuEntry], scale: f32) -> MenuLayout {
        menu_layout(&*self.text, entries, scale)
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
                        .text
                        .ellipsize(label, max_text, font_size, Weight::Regular);
                    self.text.draw(
                        &mut pixmap,
                        &label,
                        text_x,
                        self.text.centered_baseline(row.rect.center().y, font_size),
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
    frame: &mut DockFrame,
    icon: Arc<Pixmap>,
    rect: Rect,
    opacity: f32,
    tilt_x_deg: f32,
    tilt_y_deg: f32,
) {
    // Preserve aspect ratio for non-square shell icons.
    let scale = (rect.width / icon.width() as f32).min(rect.height / icon.height() as f32);
    let target = Rect::from_center(
        rect.center(),
        icon.width() as f32 * scale,
        icon.height() as f32 * scale,
    );
    let transform =
        painter::bitmap_transform(icon.width(), icon.height(), target, tilt_x_deg, tilt_y_deg);
    frame.bitmap(icon, transform, opacity);
}

fn draw_badge(frame: &mut DockFrame, rect: Rect) {
    let radius = rect.width * 0.11;
    let center = (
        rect.right() - rect.width * 0.16,
        rect.y + rect.height * 0.16,
    );
    frame.filled_circle(center, radius + 1.2, ThemeColor::rgba(20, 20, 24, 190));
    frame.filled_circle(center, radius, ThemeColor::rgb(245, 166, 35));
    let color = ThemeColor::rgba(38, 28, 10, 240);
    frame.line(
        (center.0, center.1 - radius * 0.52),
        (center.0, center.1 + radius * 0.12),
        radius * 0.3,
        color,
    );
    frame.filled_circle((center.0, center.1 + radius * 0.48), radius * 0.16, color);
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
pub fn menu_layout(fonts: &dyn TextBackend, entries: &[MenuEntry], scale: f32) -> MenuLayout {
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
        let mut frame = DockFrame::new(64, 64);
        let rect = Rect::new(8.0, 8.0, 48.0, 48.0);
        let mut renderer = Renderer::new();
        renderer.draw_start_widget(&mut frame, rect, 1.0);
        let pixmap = renderer.render_frame(&frame).expect("pixmap");
        assert!(pixmap.pixels().iter().any(|pixel| pixel.blue() > 150));

        let mut frame = DockFrame::new(64, 64);
        Renderer::draw_separator(
            &mut frame,
            rect,
            ThemeColor::rgba(255, 255, 255, 180),
            1.0,
            1.0,
        );
        let separator = renderer.render_frame(&frame).expect("pixmap");
        assert!(separator.pixels().iter().any(|pixel| pixel.alpha() > 0));

        let mut frame = DockFrame::new(64, 64);
        renderer.set_start_style(StartMenuStyle::Windows10);
        renderer.draw_start_widget(&mut frame, rect, 1.0);
        let win10 = renderer.render_frame(&frame).expect("pixmap");
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
