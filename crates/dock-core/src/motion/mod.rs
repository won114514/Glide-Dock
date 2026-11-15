//! Layered motion system.
//!
//! Five independent channels feed one [`MotionSnapshot`]:
//!
//! * **Hover** — Glide Zoom, Bounce, Wave, Fluid Press, Glow, Perspective.
//! * **Interaction** — Fluid Press plus Bounce/Glow click accents.
//! * **Launch** — four launch timelines and the waiting/settling state machine.
//! * **Status** — waiting breathing dot and fixed running dot.
//! * **Composition** — multiplies scales, adds offsets and picks light effects
//!   without letting one layer overwrite another.
//!
//! The engine is pure: it never calls the launcher, the window tracker or the
//! renderer. The app feeds pointer/window events and asks for a snapshot.

pub mod tuning;

mod compose;
mod hover;
mod launch;
mod press;
mod status;

use std::collections::{BTreeSet, HashMap};

use crate::animation::Spring;
use crate::geometry::Point;
use crate::layout::{DockGeometry, Metrics};
use crate::magnification::{MagnificationAnchors, MagnificationController, MagnificationSettings};
use crate::models::{DockItem, HoverPreset, ItemKind, LaunchPreset};

pub use status::waiting_opacity;
pub use tuning::MotionTuning;

use compose::{ComposeInput, compose};
use hover::{
    TimedEnvelope, WaveState, bounce_lift, cursor_weights, fluid_hover_scale, glow_breathing,
    perspective_targets, pointer_in_band,
};
use launch::{LaunchPhase, LaunchState, nudge_distance};
use press::{PressAccent, PressFrame, PressState};
use status::status_frame;
use tuning::MAX_OFFSET_DIP;

/// An expanding ring rendered around an icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RippleMotion {
    /// Ring radius in device pixels.
    pub radius: f32,
    /// Ring width in device pixels.
    pub width: f32,
    /// Ring opacity in `0..=1`.
    pub opacity: f32,
}

/// Per-icon transforms and status for one rendered frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconMotion {
    /// Scale multiplier applied on top of the hover layout base.
    pub scale: f32,
    /// Horizontal displacement in device pixels.
    pub offset_x: f32,
    /// Vertical displacement in device pixels (negative is up).
    pub offset_y: f32,
    /// Tilt around the X axis, in degrees.
    pub tilt_x_deg: f32,
    /// Tilt around the Y axis, in degrees.
    pub tilt_y_deg: f32,
    /// Blue-violet glow strength in `0..=1`.
    pub glow: f32,
    /// Optional expanding ring.
    pub ripple: Option<RippleMotion>,
    /// Waiting-dot opacity, when the item is waiting for a window.
    pub waiting_opacity: Option<f32>,
    /// Whether the fixed running dot may be drawn.
    pub running_dot_allowed: bool,
}

impl Default for IconMotion {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            tilt_x_deg: 0.0,
            tilt_y_deg: 0.0,
            glow: 0.0,
            ripple: None,
            waiting_opacity: None,
            running_dot_allowed: true,
        }
    }
}

/// Everything the app needs for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MotionSnapshot {
    /// Hover scales aligned with the item list, fed to `FrameLayout`.
    pub layout_scales: Vec<f32>,
    /// Per-icon render transforms aligned with the item list.
    pub icons: Vec<IconMotion>,
    /// Hover backdrop progress in `0..=1`.
    pub hover_progress: f32,
    /// `true` while any channel still needs frames.
    pub active: bool,
}

/// One item's private motion state.
#[derive(Debug, Clone)]
struct ItemMotion {
    hover_scale: Spring,
    hover_lift: Spring,
    tilt_x: Spring,
    tilt_y: Spring,
    glow: Spring,
    press: PressState,
    launch: LaunchState,
}

impl Default for ItemMotion {
    fn default() -> Self {
        Self {
            hover_scale: Spring::new(1.0),
            hover_lift: Spring::new(0.0),
            tilt_x: Spring::new(0.0),
            tilt_y: Spring::new(0.0),
            glow: Spring::new(0.0),
            press: PressState::default(),
            launch: LaunchState::default(),
        }
    }
}

/// The animation engine owned by the dock.
#[derive(Debug, Clone)]
pub struct MotionEngine {
    tuning: MotionTuning,
    items: HashMap<u32, ItemMotion>,
    cursor: Option<Point>,
    hovered: Option<usize>,
    anchor_changed: bool,
    anchor_changed_at: f32,
    glow_started: f32,
    envelope: TimedEnvelope,
    wave: WaveState,
    magnification: MagnificationController,
}

impl MotionEngine {
    /// Creates an engine with the given tuning.
    #[must_use]
    pub fn new(tuning: MotionTuning) -> Self {
        Self {
            tuning,
            items: HashMap::new(),
            cursor: None,
            hovered: None,
            anchor_changed: false,
            anchor_changed_at: 0.0,
            glow_started: 0.0,
            envelope: TimedEnvelope::default(),
            wave: WaveState::default(),
            magnification: MagnificationController::new(),
        }
    }

    /// Applies new configuration/preset selection.
    pub fn apply_config(&mut self, tuning: MotionTuning, now: f32) {
        let preset_changed = tuning.hover_preset != self.tuning.hover_preset;
        self.tuning = tuning;
        if preset_changed {
            self.wave.reset();
            if self.tuning.hover_preset == HoverPreset::Wave {
                if let Some(index) = self.hovered {
                    self.wave.trigger(now, index, Vec::new());
                }
            }
            self.glow_started = now;
        }
        if !self.tuning.enabled {
            self.settle_all();
        }
    }

    /// Forgets every item, used when the configuration is reloaded.
    pub fn reset_items(&mut self) {
        self.items.clear();
        self.cursor = None;
        self.hovered = None;
        self.anchor_changed = false;
        self.envelope = TimedEnvelope::default();
        self.wave.reset();
    }

    /// Updates pointer state. `hovered` is an index into the current item list.
    pub fn set_pointer(&mut self, now: f32, cursor: Option<Point>, hovered: Option<usize>) {
        self.cursor = cursor;
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        if let Some(index) = hovered {
            self.anchor_changed = true;
            self.anchor_changed_at = now;
            self.glow_started = now;
            self.wave.trigger(now, index, Vec::new());
            self.envelope
                .retarget(now, 1.0, self.tuning.hover_s.max(0.001));
        } else {
            self.anchor_changed = false;
            self.wave.leave(now);
            self.envelope
                .retarget(now, 0.0, self.tuning.exit_s.max(0.001));
        }
    }

    /// Pointer went down on `id`.
    pub fn pointer_down(&mut self, now: f32, id: u32) {
        if !self.tuning.enabled {
            return;
        }
        let item = self.items.entry(id).or_default();
        let current = item.press.sample_scale(now);
        item.press.pointer_down(now, current);
    }

    /// Pointer went up on `id`.
    ///
    /// The accent is chosen from the selected hover preset. When the selected
    /// launch preset already provides the same effect the accent is suppressed
    /// so the two channels do not double up.
    pub fn pointer_up(&mut self, now: f32, id: u32) {
        if !self.tuning.enabled {
            return;
        }
        let accent = match (self.tuning.hover_preset, self.tuning.launch_preset) {
            (HoverPreset::Bounce, LaunchPreset::TripleBounce) => None,
            (HoverPreset::Glow, LaunchPreset::RippleLaunch) => None,
            (HoverPreset::Bounce, _) => Some(PressAccent::BounceDouble),
            (HoverPreset::Glow, _) => Some(PressAccent::GlowWave),
            _ => None,
        };
        if let Some(item) = self.items.get_mut(&id) {
            item.press.pointer_up(now, accent);
        }
    }

    /// Cancels a press (drag, capture loss, window deactivation).
    pub fn cancel_press(&mut self, now: f32, id: u32) {
        if let Some(item) = self.items.get_mut(&id) {
            item.press.cancel(now);
        }
    }
    /// Cancels every pressed item (capture lost).
    pub fn cancel_all_presses(&mut self, now: f32) {
        for item in self.items.values_mut() {
            if item.press.is_active(now) {
                item.press.cancel(now);
            }
        }
    }

    /// Starts the launch visual timeline for `id`.
    ///
    /// `confirmed` is the app's pre-launch window snapshot; it only affects the
    /// phase that follows the animation. Animation success never implies a
    /// running application.
    pub fn begin_launch(&mut self, now: f32, id: u32, confirmed: bool) {
        let item = self.items.entry(id).or_default();
        let press_scale = item.press.sample_scale(now);
        if self.tuning.launch_enabled {
            item.press.hand_over_to_launch();
        }
        item.launch.begin(
            now,
            self.tuning.launch_preset,
            self.tuning.launch_enabled,
            press_scale,
            0.0,
            confirmed,
        );
    }

    /// Launch failed: clear waiting, ease back and keep status truthful.
    pub fn launch_failed(&mut self, now: f32, id: u32) {
        if let Some(item) = self.items.get_mut(&id) {
            item.launch.fail(now);
        }
    }

    /// Window tracker confirmed a visible window for `id`.
    pub fn window_detected(&mut self, id: u32) {
        if let Some(item) = self.items.get_mut(&id) {
            item.launch.window_detected();
        }
    }

    /// All known windows for `id` disappeared.
    pub fn window_closed(&mut self, id: u32) {
        if let Some(item) = self.items.get_mut(&id) {
            item.launch.window_closed();
        }
    }

    /// Sets the tracker confirmation without changing the phase.
    pub fn set_confirmed(&mut self, id: u32, confirmed: bool) {
        if let Some(item) = self.items.get_mut(&id) {
            item.launch.set_confirmed(confirmed);
        }
    }

    /// Advances every channel and returns the frame snapshot.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        now: f32,
        dt: f32,
        items: &[DockItem],
        geometry: &DockGeometry,
        metrics: &Metrics,
        running: &BTreeSet<u32>,
    ) -> MotionSnapshot {
        self.sync_items(items);
        if self.hovered.is_some_and(|index| index >= items.len()) {
            self.hovered = None;
            self.wave.leave(now);
        }
        if !self.tuning.enabled {
            return self.neutral_snapshot(now, items, metrics, running);
        }
        let dt = dt.clamp(0.0, tuning::MAX_FRAME_DT);
        let count = items.len();
        let hovered = self.hovered.filter(|index| *index < count);
        self.update_hover_targets(now, items, hovered, geometry, metrics);
        self.wave
            .sync_weights(hovered, count, self.tuning.range_slots);

        let cursor = self.cursor;

        // Glide Zoom is the Apple-style continuous field. It uses one pointer
        // sample, fixed base anchors, and one real-time smoothing state for the
        // whole dock. Other presets keep their own channel behaviour.
        let classic = self.tuning.hover_preset == HoverPreset::GlideZoom;
        let hovered_separator = hovered
            .and_then(|index| items.get(index))
            .is_some_and(|item| item.kind == ItemKind::Separator);
        let classic_scales = if classic {
            let settings = MagnificationSettings::classic(
                self.tuning.max_scale,
                metrics.range_slots * (metrics.icon + metrics.gap),
            );
            let anchors = MagnificationAnchors::from_geometry(geometry, count);
            let classic_top = geometry.dock_base.top()
                - (self.tuning.max_scale - 1.0).max(0.0) * metrics.icon
                - metrics.icon * 0.5;
            let classic_in_band = cursor.is_some_and(|point| {
                point.x.is_finite()
                    && point.y.is_finite()
                    && point.y >= classic_top
                    && point.y <= geometry.window.height
            });
            let cursor_x = if self.tuning.hover_enabled && classic_in_band && !hovered_separator {
                cursor.map(|point| point.x)
            } else {
                None
            };
            Some(
                self.magnification
                    .update(dt, cursor_x, &anchors, &settings)
                    .to_vec(),
            )
        } else {
            self.magnification.reset();
            None
        };
        let classic_active = classic && self.magnification.is_active();

        let mut active = self.envelope.is_active(now) || classic_active;
        for (index, item) in items.iter().enumerate() {
            let Some(state) = self.items.get_mut(&item.id) else {
                continue;
            };
            if item.kind == ItemKind::Separator {
                state.hover_scale.set_target(1.0);
                state.hover_scale.settle();
                state.hover_lift.set_target(0.0);
                state.hover_lift.settle();
                state.tilt_x.set_target(0.0);
                state.tilt_x.settle();
                state.tilt_y.set_target(0.0);
                state.tilt_y.settle();
                state.glow.set_target(0.0);
                state.glow.settle();
            }
            if !classic {
                active |= state
                    .hover_scale
                    .step(dt, self.tuning.stiffness, self.tuning.damping);
            }
            active |= state
                .hover_lift
                .step(dt, self.tuning.bounce_stiffness, self.tuning.damping);
            active |= state
                .tilt_x
                .step(dt, self.tuning.perspective_stiffness, self.tuning.damping);
            active |= state
                .tilt_y
                .step(dt, self.tuning.perspective_stiffness, self.tuning.damping);
            active |= state
                .glow
                .step(dt, self.tuning.stiffness, self.tuning.damping.max(0.85));
            state.launch.advance(now, running.contains(&item.id));
            if !self.tuning.launch_enabled {
                state.launch.skip_animation(now);
            }
            if state.launch.phase() == LaunchPhase::Waiting {
                active = true;
            }
            if state.launch.is_active() {
                active = true;
            }
            if state.press.is_active(now) {
                active = true;
            }
            if self.tuning.hover_preset == HoverPreset::Glow
                && self.tuning.hover_enabled
                && hovered == Some(index)
            {
                active = true;
            }
        }
        active |= self.wave.is_active(now, self.tuning.time_scale);
        if self.tuning.hover_preset == HoverPreset::Glow
            && self.tuning.hover_enabled
            && hovered.is_some()
        {
            active = true;
        }

        let mut nudges = vec![0.0f32; count];
        for (index, item) in items.iter().enumerate() {
            let Some(state) = self.items.get(&item.id) else {
                continue;
            };
            let factor = state.launch.nudge_factor(now);
            if factor <= 0.0 {
                continue;
            }
            let preset = state.launch.preset();
            for slot in 1..=2 {
                let distance = nudge_distance(preset, slot) * factor * metrics.icon;
                if index + slot < count {
                    nudges[index + slot] += distance;
                }
                if let Some(target) = index.checked_sub(slot) {
                    nudges[target] -= distance;
                }
            }
        }
        let nudge_limit = 0.05 * metrics.icon;
        for nudge in &mut nudges {
            *nudge = nudge.clamp(-nudge_limit, nudge_limit);
        }

        let wave_scales =
            if self.tuning.hover_enabled && self.tuning.hover_preset == HoverPreset::Wave {
                self.wave
                    .sample(now, self.tuning.max_scale, self.tuning.time_scale, count)
            } else {
                vec![1.0; count]
            };

        let mut snapshot = MotionSnapshot {
            layout_scales: vec![1.0; count],
            icons: vec![IconMotion::default(); count],
            hover_progress: self.envelope.value(now).clamp(0.0, 1.0),
            active,
        };
        let max_offset_px = MAX_OFFSET_DIP * metrics.scale;
        for (index, item) in items.iter().enumerate() {
            let Some(state) = self.items.get(&item.id) else {
                continue;
            };
            let layout_scale = if item.kind == ItemKind::Separator {
                1.0
            } else if classic {
                classic_scales
                    .as_ref()
                    .and_then(|scales| scales.get(index))
                    .copied()
                    .unwrap_or(1.0)
            } else {
                match self.tuning.hover_preset {
                    HoverPreset::GlideZoom | HoverPreset::FluidPress => state
                        .hover_scale
                        .value
                        .clamp(0.8, self.tuning.max_scale * 1.06),
                    HoverPreset::Wave => wave_scales[index],
                    HoverPreset::Bounce | HoverPreset::Glow | HoverPreset::Perspective => 1.0,
                }
            };
            snapshot.layout_scales[index] = layout_scale;
            let press_frame = state.press.frame(now, metrics);
            let launch_frame = state.launch.frame(now, self.tuning.reduced, metrics);
            let press_active = state.press.is_active(now);
            let status = status_frame(&state.launch, press_active, self.tuning.reduced, now);
            let breathing = if self.tuning.hover_preset == HoverPreset::Glow
                && hovered == Some(index)
                && !self.tuning.reduced
            {
                glow_breathing(now - self.glow_started)
            } else {
                1.0
            };
            snapshot.icons[index] = compose(ComposeInput {
                hover_lift: state.hover_lift.value,
                tilt_x: state.tilt_x.value,
                tilt_y: state.tilt_y.value,
                hover_glow: (state.glow.value * breathing).clamp(0.0, 1.0),
                neighbour_nudge: if item.kind == ItemKind::Separator {
                    0.0
                } else {
                    nudges[index]
                },
                press: press_frame,
                launch: launch_frame,
                status,
                max_offset_px,
            });
        }
        snapshot
    }

    /// Snaps every channel to rest. Used when animation is disabled.
    fn settle_all(&mut self) {
        for item in self.items.values_mut() {
            item.hover_scale.set_target(1.0);
            item.hover_scale.settle();
            item.hover_lift.set_target(0.0);
            item.hover_lift.settle();
            item.tilt_x.set_target(0.0);
            item.tilt_x.settle();
            item.tilt_y.set_target(0.0);
            item.tilt_y.settle();
            item.glow.set_target(0.0);
            item.glow.settle();
            item.press.reset();
        }
        self.envelope.settle();
        self.wave.reset();
        self.anchor_changed = false;
    }

    fn neutral_snapshot(
        &mut self,
        now: f32,
        items: &[DockItem],
        metrics: &Metrics,
        running: &BTreeSet<u32>,
    ) -> MotionSnapshot {
        self.magnification.reset();
        for item in items {
            if let Some(state) = self.items.get_mut(&item.id) {
                state.launch.advance(now, running.contains(&item.id));
                state.launch.skip_animation(now);
            }
        }
        let mut snapshot = MotionSnapshot {
            layout_scales: vec![1.0; items.len()],
            icons: vec![IconMotion::default(); items.len()],
            hover_progress: 0.0,
            active: false,
        };
        for (index, item) in items.iter().enumerate() {
            let Some(state) = self.items.get(&item.id) else {
                continue;
            };
            if state.launch.phase() == LaunchPhase::Waiting {
                // Keep a bounded frame timer alive so the 12 s timeout can
                // clear a static waiting dot in reduced-motion mode.
                snapshot.active = true;
            }
            let launch_frame = state.launch.frame(now, true, metrics);
            let status = status_frame(&state.launch, false, true, now);
            snapshot.icons[index] = compose(ComposeInput {
                hover_lift: 0.0,
                tilt_x: 0.0,
                tilt_y: 0.0,
                hover_glow: 0.0,
                neighbour_nudge: 0.0,
                press: PressFrame::default(),
                launch: launch_frame,
                status,
                max_offset_px: MAX_OFFSET_DIP * metrics.scale,
            });
        }
        snapshot
    }

    fn sync_items(&mut self, items: &[DockItem]) {
        let ids: BTreeSet<u32> = items.iter().map(|item| item.id).collect();
        self.items.retain(|id, _| ids.contains(id));
        for item in items {
            self.items.entry(item.id).or_default();
        }
    }

    fn update_hover_targets(
        &mut self,
        now: f32,
        items: &[DockItem],
        hovered: Option<usize>,
        geometry: &DockGeometry,
        metrics: &Metrics,
    ) {
        if !self.tuning.hover_enabled {
            self.wave.reset();
            self.anchor_changed = false;
            for item in items {
                if let Some(state) = self.items.get_mut(&item.id) {
                    state.hover_scale.set_target(1.0);
                    state.hover_lift.set_target(0.0);
                    state.tilt_x.set_target(0.0);
                    state.tilt_y.set_target(0.0);
                    state.glow.set_target(0.0);
                }
            }
            return;
        }
        let cursor = self.cursor;
        let in_band = cursor.is_some_and(|point| pointer_in_band(point, geometry, metrics));
        let glide_weights = if self.tuning.hover_preset == HoverPreset::GlideZoom
            && self.tuning.hover_enabled
            && hovered.is_some()
            && in_band
        {
            cursor.map(|point| cursor_weights(point, geometry, metrics))
        } else {
            None
        };
        let perspective = if self.tuning.hover_preset == HoverPreset::Perspective
            && self.tuning.hover_enabled
            && hovered.is_some()
            && in_band
        {
            cursor.map(|point| perspective_targets(point, geometry, metrics))
        } else {
            None
        };
        let fluid_scale = fluid_hover_scale(self.tuning.max_scale);
        let lift = bounce_lift(metrics, self.tuning.max_scale);
        let max_scale = self.tuning.max_scale;
        let anchor_changed = self.anchor_changed;
        let anchor_changed_at = self.anchor_changed_at;

        for (index, item) in items.iter().enumerate() {
            let Some(state) = self.items.get_mut(&item.id) else {
                continue;
            };
            // Every preset writes all five channels so switching presets cannot
            // leave a stale Bounce lift or Perspective tilt behind.
            state.hover_lift.set_target(0.0);
            state.tilt_x.set_target(0.0);
            state.tilt_y.set_target(0.0);
            state.glow.set_target(0.0);
            match self.tuning.hover_preset {
                HoverPreset::GlideZoom => {
                    let target = glide_weights
                        .as_ref()
                        .and_then(|weights| weights.get(index))
                        .map_or(1.0, |weight| 1.0 + (max_scale - 1.0) * weight);
                    let delayed = anchor_changed
                        && hovered != Some(index)
                        && hovered.is_some_and(|anchor| {
                            now < anchor_changed_at + index.abs_diff(anchor) as f32 * 0.035
                        });
                    if !delayed {
                        state.hover_scale.set_target(target);
                    }
                }
                HoverPreset::FluidPress => {
                    let target = if hovered == Some(index) {
                        fluid_scale
                    } else {
                        1.0
                    };
                    state.hover_scale.set_target(target);
                }
                HoverPreset::Bounce => {
                    state.hover_scale.set_target(1.0);
                    state
                        .hover_lift
                        .set_target(if hovered == Some(index) { lift } else { 0.0 });
                }
                HoverPreset::Glow => {
                    state.hover_scale.set_target(1.0);
                    state
                        .glow
                        .set_target(if hovered == Some(index) { 1.0 } else { 0.0 });
                }
                HoverPreset::Perspective => {
                    state.hover_scale.set_target(1.0);
                    state.tilt_x.set_target(0.0);
                    state.tilt_y.set_target(0.0);
                    if let Some(target) = perspective.as_ref().and_then(|list| list.get(index)) {
                        state.tilt_x.set_target(target.1);
                        state.tilt_y.set_target(target.0);
                    }
                }
                HoverPreset::Wave => {
                    state.hover_scale.set_target(1.0);
                }
            }
        }

        if anchor_changed && now >= anchor_changed_at + items.len().saturating_sub(1) as f32 * 0.035
        {
            self.anchor_changed = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{DockGeometry, Metrics};
    use crate::models::{Appearance, Behavior, DockItem, MotionConfig};

    fn items(count: usize) -> Vec<DockItem> {
        (0..count)
            .map(|index| {
                DockItem::application(
                    index as u32 + 1,
                    format!("App {index}"),
                    format!("C:\\Apps\\app{index}.exe"),
                )
            })
            .collect()
    }

    fn metrics() -> Metrics {
        Metrics::new(
            &Appearance::default(),
            &Behavior::default(),
            &MotionConfig::default(),
            1.0,
        )
    }

    fn tuning(hover: HoverPreset, launch: LaunchPreset) -> MotionTuning {
        let behavior = Behavior::default();
        let motion = MotionConfig {
            hover_preset: hover,
            launch_preset: launch,
            ..MotionConfig::default()
        };
        MotionTuning::new(&behavior, &motion, true)
    }

    fn engine(hover: HoverPreset, launch: LaunchPreset, count: usize) -> MotionEngine {
        let _ = count;
        MotionEngine::new(tuning(hover, launch))
    }

    #[test]
    fn glide_zoom_peaks_at_the_hovered_icon() {
        let items = items(5);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 5);
        let now = 0.0;
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        engine.set_pointer(now, Some(cursor), Some(2));
        let mut snapshot = engine.update(now, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        for frame in 0..40 {
            let now = frame as f32 / 60.0;
            snapshot = engine.update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        assert!(
            snapshot.layout_scales[2] > 1.30,
            "{:?}",
            snapshot.layout_scales
        );
        assert!(snapshot.layout_scales[1] < snapshot.layout_scales[2]);
        assert!(snapshot.layout_scales[0] < snapshot.layout_scales[1]);
    }

    #[test]
    fn launch_continues_when_hover_leaves() {
        let items = items(5);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 5);
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        let mut now = 0.0;
        engine.set_pointer(now, Some(cursor), Some(2));
        for _ in 0..20 {
            now += 1.0 / 60.0;
            engine.update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        engine.begin_launch(now, items[2].id, false);
        engine.set_pointer(now, None, None);
        let mut saw_launch_scale = false;
        for _ in 0..24 {
            now += 1.0 / 60.0;
            let snapshot = engine.update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
            if snapshot.icons[2].scale > 1.04 {
                saw_launch_scale = true;
            }
        }
        assert!(saw_launch_scale, "launch was cancelled by the hover exit");
    }

    #[test]
    fn launch_end_waits_then_window_confirmation_reveals_running_dot() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 3);
        let mut now = 0.0;
        engine.begin_launch(now, items[0].id, false);
        for _ in 0..24 {
            now += 1.0 / 60.0;
            engine.update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        let snapshot = engine.update(now, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        assert!(snapshot.icons[0].waiting_opacity.is_some());
        assert!(!snapshot.active || snapshot.icons[0].waiting_opacity.is_some());
        engine.window_detected(items[0].id);
        let snapshot = engine.update(
            now + 0.016,
            0.0,
            &items,
            &geometry,
            &metrics,
            &BTreeSet::new(),
        );
        assert!(snapshot.icons[0].waiting_opacity.is_none());
        assert!(snapshot.icons[0].running_dot_allowed);
    }

    #[test]
    fn reduced_motion_returns_neutral_frames() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let motion = MotionConfig::default();
        let tuning = MotionTuning::new(&Behavior::default(), &motion, false);
        let mut engine = MotionEngine::new(tuning);
        let cursor = Point::new(geometry.slot_centers()[1], geometry.content_base.center().y);
        engine.set_pointer(0.0, Some(cursor), Some(1));
        let snapshot = engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        assert!(!snapshot.active);
        assert!(
            snapshot
                .icons
                .iter()
                .all(|icon| (icon.scale - 1.0).abs() < 0.001)
        );
        assert!(
            snapshot
                .layout_scales
                .iter()
                .all(|scale| (scale - 1.0).abs() < 0.001)
        );
        engine.begin_launch(0.0, items[1].id, false);
        let snapshot = engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        assert_eq!(snapshot.icons[1].waiting_opacity, Some(0.75));
    }

    #[test]
    fn press_and_launch_scales_multiply() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 3);
        engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        engine.pointer_down(0.0, items[0].id);
        engine.update(
            0.075,
            1.0 / 60.0,
            &items,
            &geometry,
            &metrics,
            &BTreeSet::new(),
        );
        engine.pointer_up(0.075, items[0].id);
        let pressed = engine
            .update(0.075, 0.0, &items, &geometry, &metrics, &BTreeSet::new())
            .icons[0]
            .scale;
        assert!((pressed - 0.94).abs() < 0.02, "{pressed}");
        engine.begin_launch(0.075, items[0].id, false);
        let launched = engine
            .update(
                0.175,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            )
            .icons[0]
            .scale;
        assert!(launched > 1.1, "{launched}");
    }

    #[test]
    fn classic_magnification_does_not_jump_on_fast_pointer_moves() {
        let items = items(5);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 5);
        let mut now = 0.0;
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        engine.set_pointer(now, Some(cursor), Some(2));
        for _ in 0..90 {
            now += 1.0 / 60.0;
            engine.update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        let before = engine
            .update(now, 0.0, &items, &geometry, &metrics, &BTreeSet::new())
            .layout_scales[2];
        let far = Point::new(geometry.slot_centers()[0] - 400.0, cursor.y);
        engine.set_pointer(now, Some(far), Some(0));
        now += 1.0 / 60.0;
        let after = engine
            .update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            )
            .layout_scales[2];
        assert!(before > 1.34);
        assert!(after > 1.0 && after < before, "{before} -> {after}");
        assert!(after <= 1.3501);
    }

    #[test]
    fn separator_is_not_animated_by_hover_presets() {
        let mut items = items(3);
        items[1] = DockItem::separator(2);
        let metrics = metrics();
        let geometry = DockGeometry::with_items(&items, &metrics);
        let cursor = Point::new(geometry.slot_centers()[1], geometry.content_base.center().y);
        for preset in [HoverPreset::Bounce, HoverPreset::GlideZoom] {
            let mut engine = engine(preset, LaunchPreset::GlideLaunch, 3);
            engine.set_pointer(0.0, Some(cursor), Some(1));
            let snapshot = engine.update(
                0.2,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
            assert!((snapshot.layout_scales[1] - 1.0).abs() < 0.001);
            assert!(snapshot.icons[1].offset_y.abs() < 0.001);
        }
    }

    #[test]
    fn wave_preset_staggers_neighbours() {
        let items = items(5);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::Wave, LaunchPreset::GlideLaunch, 5);
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        engine.set_pointer(0.0, Some(cursor), Some(2));
        let early = engine.update(0.060, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        let later = engine.update(0.200, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        assert!(early.layout_scales[2] > early.layout_scales[0]);
        assert!(later.layout_scales[1] > early.layout_scales[1]);
    }

    #[test]
    fn bounce_lifts_without_changing_layout() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::Bounce, LaunchPreset::GlideLaunch, 3);
        let cursor = Point::new(geometry.slot_centers()[1], geometry.content_base.center().y);
        engine.set_pointer(0.0, Some(cursor), Some(1));
        let mut snapshot = engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        for frame in 0..20 {
            snapshot = engine.update(
                frame as f32 / 60.0,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        assert!(snapshot.icons[1].offset_y < -1.0);
        assert!((snapshot.layout_scales[1] - 1.0).abs() < 0.001);
    }

    #[test]
    fn glow_and_perspective_populate_their_channels() {
        let items = items(5);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);

        let mut glow = engine(HoverPreset::Glow, LaunchPreset::GlideLaunch, 5);
        glow.set_pointer(0.0, Some(cursor), Some(2));
        let mut snapshot = glow.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        for frame in 0..20 {
            snapshot = glow.update(
                frame as f32 / 60.0,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        assert!(snapshot.icons[2].glow > 0.8);

        let mut perspective = engine(HoverPreset::Perspective, LaunchPreset::GlideLaunch, 5);
        perspective.set_pointer(0.0, Some(cursor), Some(2));
        let mut snapshot =
            perspective.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        for frame in 0..20 {
            snapshot = perspective.update(
                frame as f32 / 60.0,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        assert!(snapshot.icons[2].tilt_y_deg.abs() <= 5.0);
        assert!(
            snapshot
                .icons
                .iter()
                .any(|icon| icon.tilt_y_deg.abs() > 0.1)
        );
    }

    #[test]
    fn frame_rate_does_not_change_elapsed_state() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut sixty = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 3);
        let mut high = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 3);
        let cursor = Point::new(geometry.slot_centers()[1], geometry.content_base.center().y);
        sixty.set_pointer(0.0, Some(cursor), Some(1));
        high.set_pointer(0.0, Some(cursor), Some(1));
        let mut now = 0.0;
        for _ in 0..10 {
            now += 1.0 / 60.0;
            sixty.update(
                now,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        let mut high_now = 0.0;
        for _ in 0..24 {
            high_now += 1.0 / 144.0;
            high.update(
                high_now,
                1.0 / 144.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        let a = sixty
            .update(0.2, 0.0, &items, &geometry, &metrics, &BTreeSet::new())
            .layout_scales[1];
        let b = high
            .update(0.2, 0.0, &items, &geometry, &metrics, &BTreeSet::new())
            .layout_scales[1];
        assert!((a - b).abs() < 0.05, "{a} vs {b}");
    }

    #[test]
    fn disabled_configuration_settles_immediately() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 3);
        let cursor = Point::new(geometry.slot_centers()[1], geometry.content_base.center().y);
        engine.set_pointer(0.0, Some(cursor), Some(1));
        engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        engine.apply_config(MotionTuning::disabled(), 0.1);
        let snapshot = engine.update(0.1, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        assert!(!snapshot.active);
        assert!(
            snapshot
                .layout_scales
                .iter()
                .all(|scale| (scale - 1.0).abs() < 0.001)
        );
    }

    #[test]
    fn magnification_off_neutralizes_every_hover_preset() {
        let items = items(4);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let cursor = Point::new(geometry.slot_centers()[2], geometry.content_base.center().y);
        for preset in HoverPreset::ALL {
            let behavior = Behavior {
                magnification: false,
                ..Behavior::default()
            };
            let motion = MotionConfig {
                hover_preset: preset,
                ..MotionConfig::default()
            };
            let tuning = MotionTuning::new(&behavior, &motion, true);
            let mut engine = MotionEngine::new(tuning);
            engine.set_pointer(0.0, Some(cursor), Some(2));
            let mut snapshot =
                engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
            for frame in 0..20 {
                snapshot = engine.update(
                    frame as f32 / 60.0,
                    1.0 / 60.0,
                    &items,
                    &geometry,
                    &metrics,
                    &BTreeSet::new(),
                );
            }
            assert!(
                snapshot
                    .layout_scales
                    .iter()
                    .all(|scale| (scale - 1.0).abs() < 0.001),
                "{preset:?}"
            );
            assert!(snapshot.icons.iter().all(|icon| icon.offset_y.abs() < 0.01));
            assert!(snapshot.icons.iter().all(|icon| icon.glow < 0.01));
            assert!(
                snapshot
                    .icons
                    .iter()
                    .all(|icon| icon.tilt_x_deg.abs() < 0.01)
            );
        }
    }

    #[test]
    fn launching_uses_the_preset_captured_at_start() {
        let items = items(3);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 3);
        engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        engine.begin_launch(0.0, items[0].id, true);
        engine.set_pointer(0.0, None, None);
        let at_100 = engine.update(0.100, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        assert!((at_100.icons[0].scale - 1.16).abs() < 0.02);
        let new_tuning = tuning(HoverPreset::GlideZoom, LaunchPreset::TripleBounce);
        engine.apply_config(new_tuning, 0.150);
        let at_200 = engine.update(0.200, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        // Glide Launch is at 1.04 at 210 ms; Triple Bounce would be 0.97.
        assert!(
            (at_200.icons[0].scale - 1.04).abs() < 0.03,
            "{}",
            at_200.icons[0].scale
        );
    }

    #[test]
    fn perspective_returns_to_flat_after_leave() {
        let items = items(5);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::Perspective, LaunchPreset::GlideLaunch, 5);
        let cursor = Point::new(
            geometry.slot_centers()[2] - metrics.icon,
            geometry.content_base.center().y,
        );
        engine.set_pointer(0.0, Some(cursor), Some(2));
        for frame in 0..20 {
            engine.update(
                frame as f32 / 60.0,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        engine.set_pointer(0.5, None, None);
        let mut snapshot = engine.update(0.5, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        for frame in 0..40 {
            snapshot = engine.update(
                0.5 + frame as f32 / 60.0,
                1.0 / 60.0,
                &items,
                &geometry,
                &metrics,
                &BTreeSet::new(),
            );
        }
        assert!(
            snapshot
                .icons
                .iter()
                .all(|icon| icon.tilt_x_deg.abs() < 0.01 && icon.tilt_y_deg.abs() < 0.01)
        );
    }

    #[test]
    fn repeated_click_restarts_without_stacking() {
        let items = items(2);
        let metrics = metrics();
        let geometry = DockGeometry::new(items.len(), &metrics);
        let mut engine = engine(HoverPreset::GlideZoom, LaunchPreset::GlideLaunch, 2);
        engine.update(0.0, 0.0, &items, &geometry, &metrics, &BTreeSet::new());
        engine.begin_launch(0.0, items[0].id, true);
        let first = engine
            .update(0.100, 0.0, &items, &geometry, &metrics, &BTreeSet::new())
            .icons[0]
            .scale;
        engine.begin_launch(0.100, items[0].id, true);
        let restarted = engine
            .update(0.100, 0.0, &items, &geometry, &metrics, &BTreeSet::new())
            .icons[0]
            .scale;
        assert!(first > 1.1);
        assert!((restarted - 1.0).abs() < 0.02);
    }
}
