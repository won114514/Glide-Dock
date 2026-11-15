//! Icon providers and generated placeholder tiles.

use std::sync::Arc;

use dock_core::Color as ThemeColor;
use dock_core::Rect;
use dock_core::models::{DockItem, ItemKind};
use tiny_skia::{Color, Pixmap};

use crate::painter;
use crate::text::{FontStack, Weight};

/// Supplies application icons to the renderer.
///
/// The platform layer extracts icons on a worker thread; the app inserts the
/// results into [`IconMap`]. Tests and previews can use the same map directly.
pub trait IconSource {
    /// Returns the best available icon for `item`.
    fn icon(&self, item: &DockItem) -> Option<Arc<Pixmap>>;
}

/// An icon source that never returns icons (placeholder tiles are drawn).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoIcons;

impl IconSource for NoIcons {
    fn icon(&self, _item: &DockItem) -> Option<Arc<Pixmap>> {
        None
    }
}

/// An in-memory icon map used by preview mode and tests.
pub struct IconMap {
    icons: std::collections::HashMap<u32, Arc<Pixmap>>,
    order: std::collections::VecDeque<u32>,
    capacity: usize,
}

impl Default for IconMap {
    fn default() -> Self {
        Self {
            icons: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            capacity: 64,
        }
    }
}

impl IconMap {
    /// Creates an empty map with the default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of retained icons. Icons are evicted in
    /// insertion order; the dock keeps this at `max(64, item count)` so a
    /// normal dock never loses a live icon.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.icons.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.icons.remove(&oldest);
        }
    }

    /// Inserts an icon for an item id.
    pub fn insert(&mut self, item_id: u32, pixmap: Pixmap) {
        if !self.icons.contains_key(&item_id) {
            while self.icons.len() >= self.capacity {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.icons.remove(&oldest);
            }
            self.order.push_back(item_id);
        }
        self.icons.insert(item_id, Arc::new(pixmap));
    }

    /// Removes an icon.
    pub fn remove(&mut self, item_id: u32) -> Option<Arc<Pixmap>> {
        self.order.retain(|id| *id != item_id);
        self.icons.remove(&item_id)
    }

    /// Number of cached icons.
    #[must_use]
    pub fn len(&self) -> usize {
        self.icons.len()
    }

    /// Returns `true` when no icons are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }

    /// Returns `true` when an icon exists for the id.
    #[must_use]
    pub fn contains(&self, item_id: u32) -> bool {
        self.icons.contains_key(&item_id)
    }
}

impl IconSource for IconMap {
    fn icon(&self, item: &DockItem) -> Option<Arc<Pixmap>> {
        self.icons.get(&item.id).cloned()
    }
}

/// RGB color used by generated icon gradients.
type Rgb = (u8, u8, u8);
/// Top and bottom colors of a generated icon gradient.
type GradientPair = (Rgb, Rgb);

/// Palette of pleasant gradient pairs for generated icons.
const PLACEHOLDER_GRADIENTS: [GradientPair; 8] = [
    ((86, 130, 255), (46, 74, 190)),
    ((150, 108, 250), (88, 56, 190)),
    ((30, 190, 178), (14, 120, 112)),
    ((250, 146, 60), (196, 84, 26)),
    ((60, 200, 120), (24, 132, 74)),
    ((244, 96, 108), (176, 44, 58)),
    ((40, 190, 220), (16, 122, 156)),
    ((236, 92, 160), (178, 32, 108)),
];

/// Draws a generated placeholder tile for an item without a real icon.
pub fn draw_placeholder(
    pixmap: &mut Pixmap,
    item: &DockItem,
    rect: Rect,
    fonts: &FontStack,
    unresolved: bool,
) {
    if rect.width <= 1.0 || rect.height <= 1.0 {
        return;
    }
    let (top, bottom) = placeholder_colors(item);
    let radius = rect.width * 0.23;
    painter::fill_rounded_vertical(
        pixmap,
        rect,
        radius,
        Color::from_rgba8(top.0, top.1, top.2, 255),
        Color::from_rgba8(bottom.0, bottom.1, bottom.2, 255),
    );

    // Glassy highlight across the top third.
    let gloss = Rect::new(
        rect.x + rect.width * 0.06,
        rect.y + rect.height * 0.04,
        rect.width * 0.88,
        rect.height * 0.48,
    );
    painter::fill_rounded_vertical(
        pixmap,
        gloss,
        radius * 0.85,
        Color::from_rgba8(255, 255, 255, 38),
        Color::from_rgba8(255, 255, 255, 0),
    );

    painter::stroke_rounded(
        pixmap,
        rect.inset(0.5),
        radius,
        1.0,
        Color::from_rgba8(255, 255, 255, 26),
    );

    let glyph_color = Color::from_rgba8(255, 255, 255, 236);
    match item.kind {
        ItemKind::Folder => draw_folder_glyph(pixmap, rect, glyph_color),
        ItemKind::File => draw_file_glyph(pixmap, rect, glyph_color),
        ItemKind::Separator => {
            painter::filled_circle(
                pixmap,
                (rect.center().x, rect.center().y),
                rect.width * 0.08,
                glyph_color,
            );
        }
        ItemKind::Application | ItemKind::Widget => {
            let label = item.label();
            let character = label
                .chars()
                .find(|character| !character.is_whitespace())
                .unwrap_or('?');
            let letter = character.to_uppercase().to_string();
            let size = rect.width * 0.42;
            fonts.draw_line_centered(
                pixmap,
                &letter,
                rect.center().x,
                rect.center().y + rect.height * 0.01,
                size,
                ThemeColor::rgba(255, 255, 255, 240),
                Weight::Semibold,
            );
        }
    }

    if unresolved {
        painter::fill_rounded(pixmap, rect, radius, Color::from_rgba8(10, 10, 14, 80));
        draw_unresolved_badge(pixmap, rect);
    }
}

fn placeholder_colors(item: &DockItem) -> GradientPair {
    let key = if item.target_path.is_empty() {
        item.display_name.as_str()
    } else {
        item.target_path.as_str()
    };
    let mut hash = 2_166_136_261u32;
    for byte in key.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    PLACEHOLDER_GRADIENTS[(hash as usize) % PLACEHOLDER_GRADIENTS.len()]
}

fn draw_folder_glyph(pixmap: &mut Pixmap, rect: Rect, glyph_color: Color) {
    let body = Rect::new(
        rect.x + rect.width * 0.2,
        rect.y + rect.height * 0.36,
        rect.width * 0.6,
        rect.height * 0.4,
    );
    painter::fill_rounded(pixmap, body, rect.width * 0.055, glyph_color);
    let tab = Rect::new(
        rect.x + rect.width * 0.2,
        rect.y + rect.height * 0.28,
        rect.width * 0.28,
        rect.height * 0.14,
    );
    painter::fill_rounded(pixmap, tab, rect.width * 0.04, glyph_color);
}

fn draw_file_glyph(pixmap: &mut Pixmap, rect: Rect, glyph_color: Color) {
    let left = rect.x + rect.width * 0.29;
    let right = rect.x + rect.width * 0.71;
    let top = rect.y + rect.height * 0.24;
    let bottom = rect.y + rect.height * 0.76;
    let fold = rect.width * 0.14;
    let points = [
        (left, top),
        (right - fold, top),
        (right, top + fold),
        (right, bottom),
        (left, bottom),
    ];
    painter::filled_polygon(pixmap, &points, glyph_color);
    painter::filled_polygon(
        pixmap,
        &[
            (right - fold, top),
            (right - fold, top + fold),
            (right, top + fold),
        ],
        Color::from_rgba8(255, 255, 255, 110),
    );
}

pub(crate) fn draw_unresolved_badge(pixmap: &mut Pixmap, rect: Rect) {
    let radius = rect.width * 0.11;
    let center = (
        rect.right() - rect.width * 0.16,
        rect.y + rect.height * 0.16,
    );
    painter::filled_circle(
        pixmap,
        center,
        radius + 1.2,
        Color::from_rgba8(20, 20, 24, 190),
    );
    painter::filled_circle(pixmap, center, radius, Color::from_rgba8(245, 166, 35, 255));
    let bar_top = center.1 - radius * 0.52;
    let bar_bottom = center.1 + radius * 0.12;
    painter::line(
        pixmap,
        (center.0, bar_top),
        (center.0, bar_bottom),
        radius * 0.3,
        Color::from_rgba8(38, 28, 10, 240),
    );
    painter::filled_circle(
        pixmap,
        (center.0, center.1 + radius * 0.48),
        radius * 0.16,
        Color::from_rgba8(38, 28, 10, 240),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dock_core::DockItem;

    #[test]
    fn placeholder_is_deterministic() {
        let item = DockItem::application(1, "Editor", "C:\\Apps\\editor.exe");
        let first = placeholder_colors(&item);
        let second = placeholder_colors(&item);
        assert_eq!(first, second);
    }

    #[test]
    fn icon_map_evicts_oldest_beyond_capacity() {
        let mut map = IconMap::new();
        map.set_capacity(2);
        map.insert(1, Pixmap::new(4, 4).expect("pixmap"));
        map.insert(2, Pixmap::new(4, 4).expect("pixmap"));
        map.insert(3, Pixmap::new(4, 4).expect("pixmap"));
        assert!(!map.contains(1));
        assert!(map.contains(2));
        assert!(map.contains(3));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn icon_map_remove_keeps_order_consistent() {
        let mut map = IconMap::new();
        map.set_capacity(2);
        map.insert(1, Pixmap::new(4, 4).expect("pixmap"));
        map.insert(2, Pixmap::new(4, 4).expect("pixmap"));
        assert!(map.remove(1).is_some());
        map.insert(3, Pixmap::new(4, 4).expect("pixmap"));
        assert!(map.contains(2));
        assert!(map.contains(3));
    }

    #[test]
    fn placeholder_draws_pixels() {
        let item = DockItem::application(1, "Editor", "C:\\Apps\\editor.exe");
        let mut pixmap = Pixmap::new(64, 64).expect("pixmap");
        draw_placeholder(
            &mut pixmap,
            &item,
            Rect::new(0.0, 0.0, 64.0, 64.0),
            &FontStack::empty(),
            false,
        );
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 200));
    }
}
