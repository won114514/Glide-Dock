//! Layout mathematics: DIP→pixel metrics, dock geometry, magnification and
//! hit testing.
//!
//! The dock window is sized once per item-count / DPI change. Inside it the
//! background "breathes": it grows upward and sideways as icons magnify, which
//! keeps the bottom edge of the dock anchored to the screen edge.

use crate::geometry::{Point, Rect, Size, clamp01, smoothstep};
use crate::models::{Appearance, Behavior, DockItem, ItemKind, MAX_HOVER_SCALE, MotionConfig};

/// Width of the empty-state hint area, in DIP.
pub const EMPTY_CONTENT_WIDTH_DIP: f32 = 286.0;

/// Device-pixel metrics derived from the DIP configuration and the current DPI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Display scaling factor for converting DIPs to device pixels.
    pub scale: f32,
    /// Icon edge length in device pixels.
    pub icon: f32,
    /// Gap between slots in device pixels.
    pub gap: f32,
    /// Dock padding in device pixels.
    pub padding: f32,
    /// Dock corner radius in device pixels.
    pub radius: f32,
    /// Distance to the work-area edge in device pixels.
    pub edge_margin: f32,
    /// Transparent margin reserved for the drop shadow.
    pub shadow: f32,
    /// Extra height above the dock reserved for magnification and tooltips.
    pub headroom: f32,
    /// Extra width on each side reserved for magnified icons.
    pub slack: f32,
    /// Tooltip bubble height.
    pub tooltip_height: f32,
    /// Gap between the icon and its tooltip.
    pub tooltip_gap: f32,
    /// Maximum icon scale (for layout bounds).
    pub max_factor: f32,
    /// Magnification attenuation radius in icon slots.
    pub range_slots: f32,
    /// Width of a separator slot, in device pixels.
    pub separator_width: f32,
    /// Tighter gap around separator slots, in device pixels.
    pub separator_gap: f32,
}

impl Metrics {
    /// Builds the device-pixel metrics from configuration and DPI scale.
    #[must_use]
    pub fn new(
        appearance: &Appearance,
        behavior: &Behavior,
        motion: &MotionConfig,
        dpi_scale: f32,
    ) -> Self {
        let scale = if dpi_scale.is_finite() {
            dpi_scale.clamp(0.5, 4.0)
        } else {
            1.0
        };
        let icon = appearance.icon_size.clamp(24.0, 128.0) * scale;
        let gap = appearance.spacing.clamp(0.0, 48.0) * scale;
        let padding = appearance.padding.clamp(4.0, 48.0) * scale;
        let radius = appearance.corner_radius.clamp(0.0, 64.0) * scale;
        let edge_margin = appearance.edge_margin.max(0.0) * scale;

        let max_factor = if behavior.magnification {
            motion.max_scale.clamp(1.0, MAX_HOVER_SCALE)
        } else {
            1.0
        };
        let range_slots = motion.range_slots.clamp(0.5, 8.0);
        let extra = (max_factor - 1.0) * icon;

        let tooltip_height = 30.0 * scale;
        let tooltip_gap = 9.0 * scale;
        let tooltip_reserve = if behavior.show_tooltips {
            tooltip_height + tooltip_gap
        } else {
            0.0
        };

        let shadow = 24.0 * scale;
        // Reserve room above the dock only for magnification overhang and hover
        // tooltips; the live context menu now renders in its own popup window.
        let headroom = extra + tooltip_reserve + 8.0 * scale;
        // The magnified row can grow by roughly `extra * influence` in total.
        // Split that budget across both sides so high factors/ranges do not
        // push icons outside the layered window.
        let influence = (2.0 * range_slots + 1.0).min(9.0);
        let slack = extra * influence * 0.5 + 8.0 * scale;

        Self {
            scale,
            icon,
            gap,
            padding,
            radius,
            edge_margin,
            shadow,
            headroom,
            slack,
            tooltip_height,
            tooltip_gap,
            max_factor,
            range_slots,
            separator_width: 12.0 * scale,
            separator_gap: 18.0 * scale,
        }
    }

    /// Converts a DIP value to device pixels.
    #[must_use]
    pub fn dip(&self, value: f32) -> f32 {
        value * self.scale
    }

    /// Total layered-window width for `item_count` icons at rest.
    #[must_use]
    pub fn window_width(&self, item_count: usize) -> f32 {
        let content_width = if item_count == 0 {
            EMPTY_CONTENT_WIDTH_DIP * self.scale
        } else {
            item_count as f32 * self.icon + item_count.saturating_sub(1) as f32 * self.gap
        };
        let dock_width = content_width + 2.0 * self.padding;
        (dock_width + 2.0 * (self.shadow + self.slack)).max(440.0 * self.scale)
    }

    /// Shrinks icon/gap/radius so the window fits `max_width`. The persisted
    /// configuration is untouched; this is a display-time fit for tiny screens
    /// or very large configured icon counts.
    pub fn fit_to_width(&mut self, max_width: f32, item_count: usize) {
        if item_count == 0 || !max_width.is_finite() || max_width <= 0.0 {
            return;
        }
        for _ in 0..4 {
            let width = self.window_width(item_count);
            if width <= max_width {
                return;
            }
            let ratio = (max_width / width).clamp(0.5, 0.99);
            self.icon = (self.icon * ratio).max(16.0 * self.scale);
            self.gap *= ratio;
            self.radius *= ratio;
            self.separator_width *= ratio;
            self.separator_gap *= ratio;
            let extra = (self.max_factor - 1.0) * self.icon;
            let influence = (2.0 * self.range_slots + 1.0).min(9.0);
            self.slack = extra * influence * 0.5 + 8.0 * self.scale;
        }
    }

    /// Height of the dock background at rest.
    #[must_use]
    pub fn dock_height(&self) -> f32 {
        self.icon + 2.0 * self.padding
    }

    /// Total height of the layered window (shadow + headroom + dock + shadow).
    #[must_use]
    pub fn window_height(&self) -> f32 {
        self.shadow + self.headroom + self.dock_height() + self.shadow
    }
}

/// Static geometry of a dock with a given item count.
#[derive(Debug, Clone, PartialEq)]
pub struct DockGeometry {
    /// Full layered window size.
    pub window: Size,
    /// Background rectangle at rest.
    pub dock_base: Rect,
    /// Area occupied by the icon slots at rest.
    pub content_base: Rect,
    /// Icon edge length.
    pub icon: f32,
    /// Width of each slot, including separators.
    pub slot_widths: Vec<f32>,
    /// Gap after each slot (`len - 1`); separators use tighter gaps.
    pub gaps: Vec<f32>,
    /// Default gap between icons.
    pub gap: f32,
    /// Dock padding.
    pub padding: f32,
    /// Corner radius.
    pub radius: f32,
    /// Shadow margin inside the window.
    pub shadow: f32,
    /// Number of icon slots.
    pub item_count: usize,
}

impl DockGeometry {
    /// Computes the static geometry.
    #[must_use]
    pub fn new(item_count: usize, metrics: &Metrics) -> Self {
        let widths = vec![metrics.icon; item_count];
        let gaps = vec![metrics.gap; item_count.saturating_sub(1)];
        Self::from_widths(widths, gaps, metrics)
    }

    /// Computes geometry with a narrow slot and tighter gaps around separators.
    #[must_use]
    pub fn with_items(items: &[DockItem], metrics: &Metrics) -> Self {
        let widths: Vec<f32> = items
            .iter()
            .map(|item| {
                if item.kind == ItemKind::Separator {
                    metrics.separator_width
                } else {
                    metrics.icon
                }
            })
            .collect();
        let gaps = items
            .windows(2)
            .map(|pair| {
                if pair.iter().any(|item| item.kind == ItemKind::Separator) {
                    metrics.separator_gap
                } else {
                    metrics.gap
                }
            })
            .collect();
        Self::from_widths(widths, gaps, metrics)
    }

    fn from_widths(widths: Vec<f32>, gaps: Vec<f32>, metrics: &Metrics) -> Self {
        let item_count = widths.len();
        let content_width = if item_count == 0 {
            EMPTY_CONTENT_WIDTH_DIP * metrics.scale
        } else {
            widths.iter().sum::<f32>() + gaps.iter().sum::<f32>()
        };
        let dock_width = content_width + 2.0 * metrics.padding;
        let dock_height = metrics.dock_height();
        let window_width = if item_count == 0 {
            metrics.window_width(0)
        } else {
            (dock_width + 2.0 * (metrics.shadow + metrics.slack)).max(440.0 * metrics.scale)
        };
        let window_height = metrics.window_height();

        let dock_base = Rect::new(
            (window_width - dock_width) / 2.0,
            metrics.shadow + metrics.headroom,
            dock_width,
            dock_height,
        );
        let content_base = Rect::new(
            dock_base.x + metrics.padding,
            dock_base.y + metrics.padding,
            content_width,
            metrics.icon,
        );

        Self {
            window: Size::new(window_width, window_height),
            dock_base,
            content_base,
            icon: metrics.icon,
            slot_widths: widths,
            gaps,
            gap: metrics.gap,
            padding: metrics.padding,
            radius: metrics.radius,
            shadow: metrics.shadow,
            item_count,
        }
    }

    /// Returns `true` when the dock has no items (empty-state layout).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.item_count == 0
    }

    /// Horizontal center of every icon slot at rest.
    #[must_use]
    pub fn slot_centers(&self) -> Vec<f32> {
        let mut centers = Vec::with_capacity(self.item_count);
        let mut x = self.content_base.x;
        for (index, width) in self.slot_widths.iter().enumerate() {
            centers.push(x + width / 2.0);
            x += width + self.gaps.get(index).copied().unwrap_or(0.0);
        }
        centers
    }

    /// Rectangle of a slot at rest.
    #[must_use]
    pub fn slot_rects(&self) -> Vec<Rect> {
        let mut rects = Vec::with_capacity(self.item_count);
        let mut x = self.content_base.x;
        for (index, width) in self.slot_widths.iter().enumerate() {
            rects.push(Rect::new(x, self.content_base.y, *width, self.icon));
            x += width + self.gaps.get(index).copied().unwrap_or(0.0);
        }
        rects
    }
}

// Project the desired centered row into the available interval. The two
// directional passes bound each other without selecting a moving anchor.
// If every icon already fits at its center, both passes leave it there.
fn reflow_group(
    icons: &mut [Rect],
    centers: &[f32],
    base: f32,
    left: f32,
    right: f32,
    bottom: f32,
) {
    if icons.is_empty() {
        return;
    }
    let base_width = base * icons.len() as f32;
    let extra: f32 = icons.iter().map(|icon| icon.width - base).sum();
    let fraction = if extra > 0.0 {
        ((right - left - base_width) / extra).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut cursor = left;
    let mut latest = right - base_width - extra * fraction;
    for (icon, center) in icons.iter_mut().zip(centers) {
        let width = base + (icon.width - base) * fraction;
        let desired = center - width / 2.0;
        let x = desired.max(cursor).min(latest);
        *icon = Rect::new(x, bottom - width, width, width);
        cursor = x + width;
        latest += width;
    }
    cursor = right;
    let mut earliest = left + base_width + extra * fraction;
    for (icon, center) in icons.iter_mut().zip(centers).rev() {
        let desired = center - icon.width / 2.0;
        earliest -= icon.width;
        let x = desired.min(cursor - icon.width).max(earliest);
        cursor = x;
        icon.x = (icon.x + x) / 2.0;
    }
}

/// One rendered frame: icon rectangles, animated background and insertion point.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLayout {
    /// Animated icon rectangles, indexed like `config.items`.
    pub icons: Vec<Rect>,
    /// Animated dock background.
    pub dock: Rect,
    /// Content bounds of the animated icons.
    pub content: Rect,
}

impl FrameLayout {
    /// Computes the layout for the given spring values.
    #[must_use]
    pub fn compute(geometry: &DockGeometry, metrics: &Metrics, scales: &[f32]) -> Self {
        let count = scales.len().min(geometry.item_count);
        if count == 0 {
            return Self {
                icons: Vec::new(),
                dock: geometry.dock_base,
                content: geometry.content_base,
            };
        }

        let centers = geometry.slot_centers();
        let bottom = geometry.dock_base.bottom() - metrics.padding;
        let mut icons: Vec<Rect> = (0..count)
            .map(|index| {
                let base = geometry.slot_widths[index];
                let height = if base < geometry.icon {
                    metrics.icon
                } else {
                    base * scales[index].clamp(1.0, MAX_HOVER_SCALE)
                };
                let width = if base < geometry.icon { base } else { height };
                Rect::new(centers[index] - width / 2.0, bottom - height, width, height)
            })
            .collect();

        // Separators are immovable boundaries. The outside groups can use the
        // window's slack, while each inside group uses the gaps at its edges.
        let mut start = 0;
        let mut left = metrics.shadow * 0.5;
        for end in 0..=count {
            if end < count && geometry.slot_widths[end] >= geometry.icon {
                continue;
            }
            let right = icons
                .get(end)
                .map_or(geometry.window.width - metrics.shadow * 0.5, |icon| {
                    icon.left()
                });
            reflow_group(
                &mut icons[start..end],
                &centers[start..end],
                geometry.icon,
                left,
                right,
                bottom,
            );
            if end < count {
                left = icons[end].right();
                start = end + 1;
            }
        }
        let extra_top = icons
            .iter()
            .map(|icon| icon.height - metrics.icon)
            .fold(0.0, f32::max);

        let icon_left = icons
            .iter()
            .map(|rect| rect.left())
            .fold(f32::MAX, f32::min);
        let icon_right = icons
            .iter()
            .map(|rect| rect.right())
            .fold(f32::MIN, f32::max);
        let dock_left = geometry
            .dock_base
            .left()
            .min(icon_left - metrics.padding)
            .max(metrics.shadow * 0.35);
        let dock_right = geometry
            .dock_base
            .right()
            .max(icon_right + metrics.padding)
            .min(geometry.window.width - metrics.shadow * 0.35);

        let dock = Rect::new(
            dock_left,
            geometry.dock_base.top() - extra_top,
            (dock_right - dock_left).max(1.0),
            geometry.dock_base.height + extra_top,
        );

        let content = Rect::from_ltrb(
            icons[0].left(),
            icons.iter().map(|rect| rect.top()).fold(f32::MAX, f32::min),
            icons[count - 1].right(),
            icons
                .iter()
                .map(|rect| rect.bottom())
                .fold(f32::MIN, f32::max),
        );

        Self {
            icons,
            dock,
            content,
        }
    }

    /// Index of the icon containing `point`, if any.
    #[must_use]
    pub fn hit_icon(&self, point: Point) -> Option<usize> {
        self.icons.iter().position(|rect| rect.contains(point))
    }

    /// Returns `true` when the point is inside the animated dock background.
    #[must_use]
    pub fn hit_background(&self, point: Point) -> bool {
        self.dock.contains(point)
    }

    /// Closest icon index to a point, used for hover intent between slots.
    #[must_use]
    pub fn nearest_icon(&self, point: Point) -> Option<usize> {
        self.icons
            .iter()
            .enumerate()
            .filter(|(_, rect)| point.y >= rect.top() - 8.0 && point.y <= self.dock.bottom())
            .min_by(|(_, a), (_, b)| {
                let da = (a.center().x - point.x).abs();
                let db = (b.center().x - point.x).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
    }
}

/// Computes the target scale for every icon under the cursor.
#[must_use]
pub fn target_scales(
    count: usize,
    cursor: Option<Point>,
    geometry: &DockGeometry,
    metrics: &Metrics,
    behavior: &Behavior,
) -> Vec<f32> {
    let mut scales = vec![1.0; count];
    if count == 0 || !behavior.magnification || metrics.max_factor <= 1.0 {
        return scales;
    }
    let Some(cursor) = cursor else {
        return scales;
    };
    if !cursor.y.is_finite() || !cursor.x.is_finite() {
        return scales;
    }
    // Hovering far outside the dock (e.g. on the tooltip/transparent margin)
    // should not magnify anything.
    if cursor.y < geometry.dock_base.top() - metrics.headroom || cursor.y > geometry.window.height {
        return scales;
    }

    let range = (metrics.range_slots.max(0.5)) * (metrics.icon + metrics.gap);
    let factor = metrics.max_factor;
    for (index, center) in geometry.slot_centers().into_iter().enumerate().take(count) {
        let distance = (center - cursor.x).abs();
        let t = clamp01(1.0 - distance / range);
        let weight = smoothstep(t);
        scales[index] = 1.0 + (factor - 1.0) * weight;
    }
    scales
}

/// Computes the tooltip bubble rectangle for a hovered icon.
#[must_use]
pub fn tooltip_rect(
    icon: Rect,
    text_width: f32,
    geometry: &DockGeometry,
    metrics: &Metrics,
) -> Rect {
    let width = (text_width + 22.0 * metrics.scale).max(52.0 * metrics.scale);
    let height = metrics.tooltip_height;
    let ideal_x = icon.center().x - width / 2.0;
    let min_x = geometry.shadow * 0.25;
    let max_x = geometry.window.width - geometry.shadow * 0.25;
    let x = ideal_x.clamp(min_x, (max_x - width).max(min_x));
    let y = (icon.top() - metrics.tooltip_gap - height).max(geometry.shadow * 0.25);
    Rect::new(x, y, width, height)
}

/// Computes the insertion index for a horizontal drag at `x`.
///
/// The returned value is in `0..=icons.len()` and refers to the icon list
/// passed in (which may already have the dragged item removed).
#[must_use]
pub fn insertion_index(icons: &[Rect], x: f32) -> usize {
    for (index, rect) in icons.iter().enumerate() {
        if x < rect.center().x {
            return index;
        }
    }
    icons.len()
}

/// Horizontal position of the insertion indicator.
#[must_use]
pub fn insertion_x(icons: &[Rect], index: usize, gap: f32) -> Option<f32> {
    if icons.is_empty() {
        return None;
    }
    let index = index.min(icons.len());
    if index == 0 {
        Some(icons[0].left() - gap / 2.0)
    } else if index == icons.len() {
        Some(icons[icons.len() - 1].right() + gap / 2.0)
    } else {
        Some((icons[index - 1].right() + icons[index].left()) / 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Appearance, Behavior, MotionConfig};

    fn metrics() -> Metrics {
        Metrics::new(
            &Appearance::default(),
            &Behavior::default(),
            &MotionConfig::default(),
            1.0,
        )
    }

    #[test]
    fn geometry_for_zero_items_has_hint_width() {
        let geometry = DockGeometry::new(0, &metrics());
        assert!(geometry.is_empty());
        assert!((geometry.content_base.width - EMPTY_CONTENT_WIDTH_DIP).abs() < 0.01);
        assert_eq!(geometry.slot_centers().len(), 0);
    }

    #[test]
    fn geometry_grows_with_items() {
        let m = metrics();
        let one = DockGeometry::new(1, &m);
        let two = DockGeometry::new(2, &m);
        assert!((two.content_base.width - (one.content_base.width + 48.0 + 18.0)).abs() < 0.01);
        assert!(!one.is_empty());
    }

    #[test]
    fn slot_centers_are_uniform() {
        let geometry = DockGeometry::new(5, &metrics());
        let centers = geometry.slot_centers();
        assert_eq!(centers.len(), 5);
        for pair in centers.windows(2) {
            assert!((pair[1] - pair[0] - 66.0).abs() < 0.01);
        }
    }

    #[test]
    fn unit_scales_reproduce_slots() {
        let m = metrics();
        let geometry = DockGeometry::new(7, &m);
        let scales = vec![1.0; 7];
        let frame = FrameLayout::compute(&geometry, &m, &scales);
        let slots = geometry.slot_rects();
        for (icon, slot) in frame.icons.iter().zip(slots.iter()) {
            assert!((icon.left() - slot.left()).abs() < 0.01);
            assert!((icon.bottom() - slot.bottom()).abs() < 0.01);
            assert!((icon.width - slot.width).abs() < 0.01);
        }
    }

    #[test]
    fn layout_never_shrinks_application_slots() {
        let m = metrics();
        let geometry = DockGeometry::new(2, &m);
        let frame = FrameLayout::compute(&geometry, &m, &[0.2, 1.0]);
        assert!((frame.icons[0].width - m.icon).abs() < 0.01);
        assert!((frame.icons[0].height - m.icon).abs() < 0.01);
    }

    #[test]
    fn separator_slot_is_narrow_and_stays_fixed() {
        let m = metrics();
        let items = [
            DockItem::application(1, "One", "C:\\one.exe"),
            DockItem::separator(2),
            DockItem::application(3, "Three", "C:\\three.exe"),
        ];
        let geometry = DockGeometry::with_items(&items, &m);
        assert!((geometry.slot_widths[1] - m.separator_width).abs() < 0.01);
        assert!((geometry.gaps[0] - m.separator_gap).abs() < 0.01);
        assert!((geometry.gaps[1] - m.separator_gap).abs() < 0.01);
        let icon_edge_gap = geometry.gaps[0] + geometry.slot_widths[1] + geometry.gaps[1];
        assert!(
            (icon_edge_gap - (m.gap * 2.0 + m.separator_width)).abs() < 0.01,
            "separator keeps the original wide slot"
        );
        let scales = [1.35, 1.0, 1.35];
        let frame = FrameLayout::compute(&geometry, &m, &scales);
        assert!((frame.icons[1].width - m.separator_width).abs() < 0.01);
        assert!((frame.icons[1].height - m.icon).abs() < 0.01);
        assert!((frame.icons[1].center().x - geometry.slot_centers()[1]).abs() < 0.01);
        assert!(frame.icons[0].width > frame.icons[1].width);
    }

    #[test]
    fn separator_stays_at_its_base_center_during_reflow() {
        let appearance = Appearance::default();
        let behavior = Behavior::default();
        let motion = MotionConfig {
            max_scale: 2.6,
            ..MotionConfig::default()
        };
        let m = Metrics::new(&appearance, &behavior, &motion, 1.0);
        let items = [
            DockItem::application(1, "One", "C:\\one.exe"),
            DockItem::separator(2),
            DockItem::application(3, "Three", "C:\\three.exe"),
        ];
        let geometry = DockGeometry::with_items(&items, &m);
        let frame = FrameLayout::compute(&geometry, &m, &[2.6, 1.0, 2.6]);
        assert!((frame.icons[1].center().x - geometry.slot_centers()[1]).abs() < 0.01);
        assert!((frame.icons[1].width - m.separator_width).abs() < 0.01);
    }

    #[test]
    fn two_x_reflow_is_continuous_across_a_slot_midpoint() {
        let config = MotionConfig {
            max_scale: 2.0,
            ..MotionConfig::default()
        };
        let behavior = Behavior::default();
        let m = Metrics::new(&Appearance::default(), &behavior, &config, 1.0);
        let geometry = DockGeometry::new(10, &m);
        let centers = geometry.slot_centers();
        let x = (centers[4] + centers[5]) / 2.0;
        let frame_at = |x| {
            let cursor = Point::new(x, geometry.content_base.center().y);
            let scales = target_scales(10, Some(cursor), &geometry, &m, &behavior);
            FrameLayout::compute(&geometry, &m, &scales)
        };
        for (a, b) in frame_at(x - 0.01)
            .icons
            .iter()
            .zip(frame_at(x + 0.01).icons)
        {
            assert!((a.center().x - b.center().x).abs() < 0.1);
        }
    }

    #[test]
    fn group_spends_gaps_before_shrinking_all_increments_proportionally() {
        let m = metrics();
        let items = [
            DockItem::separator(1),
            DockItem::application(2, "A", "a.exe"),
            DockItem::application(3, "B", "b.exe"),
            DockItem::separator(4),
        ];
        let g = DockGeometry::with_items(&items, &m);
        let gap_used = FrameLayout::compute(&g, &m, &[1.0, 1.5, 1.5, 1.0]);
        assert!((gap_used.icons[1].width - m.icon * 1.5).abs() < 0.01);
        let squeezed = FrameLayout::compute(&g, &m, &[2.0, 2.0, 1.5, 2.0]);
        let icons = squeezed.icons;
        assert!(icons[1].width < m.icon * 2.0);
        assert!(((icons[1].width - m.icon) / (icons[2].width - m.icon) - 2.0).abs() < 0.01);
        assert!(
            icons
                .windows(2)
                .all(|pair| pair[0].right() <= pair[1].left() + 0.01)
        );
        assert_eq!(icons[0], g.slot_rects()[0]);
        assert_eq!(icons[3], g.slot_rects()[3]);
    }

    #[test]
    fn magnification_is_bounded_and_peaks_at_cursor() {
        let m = metrics();
        let geometry = DockGeometry::new(6, &m);
        let behavior = Behavior::default();
        let cursor = Point::new(geometry.slot_centers()[3], geometry.content_base.center().y);
        let scales = target_scales(6, Some(cursor), &geometry, &m, &behavior);
        assert!((scales[3] - 1.35).abs() < 0.001);
        assert!((scales[0] - 1.0).abs() < 0.001);
        assert!(scales.iter().all(|scale| *scale >= 1.0 && *scale <= 1.35));
    }

    #[test]
    fn magnified_icons_stay_inside_window_at_extremes() {
        let m = metrics();
        let geometry = DockGeometry::new(10, &m);
        let behavior = Behavior::default();
        for index in [0usize, 9] {
            let cursor = Point::new(
                geometry.slot_centers()[index],
                geometry.content_base.center().y,
            );
            let scales = target_scales(10, Some(cursor), &geometry, &m, &behavior);
            let frame = FrameLayout::compute(&geometry, &m, &scales);
            for icon in &frame.icons {
                assert!(icon.left() >= 0.0, "left {}", icon.left());
                assert!(
                    icon.right() <= geometry.window.width,
                    "right {}",
                    icon.right()
                );
            }
            assert!(frame.dock.left() >= 0.0);
            assert!(frame.dock.right() <= geometry.window.width);
        }
    }

    #[test]
    fn default_magnification_keeps_fixed_centres_and_bottom() {
        let m = metrics();
        let geometry = DockGeometry::new(5, &m);
        let behavior = Behavior::default();
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        let scales = target_scales(5, Some(cursor), &geometry, &m, &behavior);
        let frame = FrameLayout::compute(&geometry, &m, &scales);
        let base_bottom = geometry.slot_rects()[0].bottom();
        for (index, icon) in frame.icons.iter().enumerate() {
            assert!((icon.center().x - geometry.slot_centers()[index]).abs() < 0.01);
            assert!((icon.bottom() - base_bottom).abs() < 0.01);
        }
        assert!(frame.icons[2].width > m.icon);
    }

    #[test]
    fn max_hover_scale_three_point_five_is_not_clamped() {
        let appearance = Appearance::default();
        let behavior = Behavior::default();
        let motion = MotionConfig {
            max_scale: 3.5,
            ..MotionConfig::default()
        };
        let m = Metrics::new(&appearance, &behavior, &motion, 1.0);
        assert!((m.max_factor - 3.5).abs() < 0.001);
        let geometry = DockGeometry::new(5, &m);
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        let scales = target_scales(5, Some(cursor), &geometry, &m, &behavior);
        assert!((scales[2] - 3.5).abs() < 0.01);
        let frame = FrameLayout::compute(&geometry, &m, &scales);
        assert!(frame.icons[2].width > 3.0 * 48.0);
    }

    #[test]
    fn disabled_magnification_returns_unit_scales() {
        let m = metrics();
        let geometry = DockGeometry::new(4, &m);
        let behavior = Behavior {
            magnification: false,
            ..Behavior::default()
        };
        let cursor = Point::new(10.0, 10.0);
        let scales = target_scales(4, Some(cursor), &geometry, &m, &behavior);
        assert!(
            scales
                .iter()
                .all(|scale| (*scale - 1.0).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn hit_test_is_half_open() {
        let m = metrics();
        let geometry = DockGeometry::new(3, &m);
        let frame = FrameLayout::compute(&geometry, &m, &[1.0; 3]);
        let rect = frame.icons[1];
        assert_eq!(frame.hit_icon(rect.center()), Some(1));
        assert_eq!(
            frame.hit_icon(Point::new(rect.right(), rect.center().y)),
            None
        );
        assert_eq!(frame.hit_icon(Point::new(rect.x, rect.center().y)), Some(1));
    }

    #[test]
    fn insertion_index_matches_centers() {
        let m = metrics();
        let geometry = DockGeometry::new(4, &m);
        let frame = FrameLayout::compute(&geometry, &m, &[1.0; 4]);
        let icons = &frame.icons;
        assert_eq!(insertion_index(icons, -100.0), 0);
        assert_eq!(insertion_index(icons, 100_000.0), 4);
        assert_eq!(insertion_index(icons, icons[2].center().x - 1.0), 2);
        assert_eq!(insertion_index(icons, icons[2].center().x + 1.0), 3);
    }

    #[test]
    fn oversized_icon_sets_fit_the_work_area() {
        let appearance = Appearance {
            icon_size: 128.0,
            ..Appearance::default()
        };
        let behavior = Behavior::default();
        let motion = MotionConfig {
            max_scale: 2.0,
            range_slots: 8.0,
            ..MotionConfig::default()
        };
        let mut m = Metrics::new(&appearance, &behavior, &motion, 1.0);
        let count = 20;
        assert!(m.window_width(count) > 1920.0);
        m.fit_to_width(1920.0, count);
        assert!(
            m.window_width(count) <= 1920.5,
            "width {}",
            m.window_width(count)
        );
        let geometry = DockGeometry::new(count, &m);
        assert!((geometry.window.width - m.window_width(count)).abs() < 0.01);
    }

    #[test]
    fn tooltip_stays_inside_window() {
        let m = metrics();
        let geometry = DockGeometry::new(12, &m);
        let frame = FrameLayout::compute(&geometry, &m, &[1.0; 12]);
        for icon in &frame.icons {
            let tip = tooltip_rect(*icon, 120.0, &geometry, &m);
            assert!(tip.left() >= 0.0);
            assert!(tip.right() <= geometry.window.width);
            assert!(tip.top() >= 0.0);
        }
    }
}
