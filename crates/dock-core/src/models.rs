//! Persistent domain model: items, appearance, behavior and the top level
//! [`DockConfig`].
//!
//! Everything here is OS agnostic. Paths are opaque strings; the platform layer
//! decides how they are resolved.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::theme::{AccentColor, ThemeMode};

/// Current configuration schema version.
pub const CONFIG_VERSION: u32 = 3;

/// Largest supported hover magnification.
pub const MAX_HOVER_SCALE: f32 = 3.5;

/// How a dock entry should be started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchType {
    /// Resolve from the target and shortcut on load; never persisted after
    /// normalization.
    #[default]
    Auto,
    /// A regular Win32 executable, started with `CreateProcessW`.
    Exe,
    /// A `.lnk` that must be handed to the shell.
    #[serde(alias = "lnk")]
    Shortcut,
    /// A file opened through `ShellExecuteExW`.
    File,
    /// A folder opened through `ShellExecuteExW`.
    Folder,
    /// A URL opened through `ShellExecuteExW`.
    Url,
    /// A packaged / Store app activated through its AppUserModelID.
    #[serde(alias = "packaged", alias = "uwp", alias = "store")]
    PackagedApp,
}

/// The kind of a dock entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// An executable, shortcut or packaged application.
    #[default]
    Application,
    /// A regular file opened through the shell.
    File,
    /// A directory opened in Explorer.
    Folder,
    /// A built-in action button such as the Windows Start menu.
    Widget,
    /// A visual separator.
    Separator,
}

/// A single dock entry.
///
/// Both `target_path` and `shortcut_path` are kept: the former is used for
/// display and de-duplication, the latter preserves the full launch semantics of
/// the original `.lnk` file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockItem {
    /// Stable identifier. Never changes when items are reordered.
    pub id: u32,
    /// Entry kind.
    pub kind: ItemKind,
    /// How the entry should be started.
    #[serde(default)]
    pub launch_type: LaunchType,
    /// Label rendered in tooltips and menus.
    pub display_name: String,
    /// Resolved target path (`.exe`, folder or file).
    pub target_path: String,
    /// Original `.lnk` path, when the entry came from a shortcut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_path: Option<String>,
    /// Arguments carried by the original shortcut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// Working directory carried by the original shortcut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

impl DockItem {
    /// Creates an application item from a path.
    #[must_use]
    pub fn application(id: u32, name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id,
            kind: ItemKind::Application,
            launch_type: LaunchType::Exe,
            display_name: name.into(),
            target_path: path.into(),
            ..Self::default()
        }
    }

    /// Creates a packaged / Store application entry from its AppUserModelID.
    #[must_use]
    pub fn packaged_app(id: u32, name: impl Into<String>, aumid: impl Into<String>) -> Self {
        Self {
            id,
            kind: ItemKind::Application,
            launch_type: LaunchType::PackagedApp,
            display_name: name.into(),
            target_path: aumid.into(),
            ..Self::default()
        }
    }

    /// Creates a built-in widget action.
    #[must_use]
    pub fn widget(id: u32, name: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            id,
            kind: ItemKind::Widget,
            launch_type: LaunchType::File,
            display_name: name.into(),
            target_path: action.into(),
            ..Self::default()
        }
    }

    /// Creates a visual separator.
    #[must_use]
    pub fn separator(id: u32) -> Self {
        Self {
            id,
            kind: ItemKind::Separator,
            launch_type: LaunchType::File,
            display_name: "分隔线".to_string(),
            ..Self::default()
        }
    }

    /// Resolves an [`LaunchType::Auto`] entry from its target/shortcut.
    #[must_use]
    pub fn infer_launch_type(&self) -> LaunchType {
        if self.launch_type != LaunchType::Auto {
            return self.launch_type;
        }
        if self.kind == ItemKind::Separator || self.kind == ItemKind::Widget {
            return LaunchType::File;
        }
        let target = self.target_path.trim();
        if is_packaged_target(target) {
            return LaunchType::PackagedApp;
        }
        if is_url(target) {
            return LaunchType::Url;
        }
        let extension = file_extension(target);
        if extension == "lnk" {
            return LaunchType::Shortcut;
        }
        if self.shortcut_path.is_some() && target.is_empty() {
            return LaunchType::Shortcut;
        }
        if extension == "exe" || extension == "com" {
            return LaunchType::Exe;
        }
        if self.kind == ItemKind::Folder {
            return LaunchType::Folder;
        }
        if self.kind == ItemKind::Application && extension.is_empty() {
            // A shell-namespace shortcut with no target path.
            return LaunchType::Shortcut;
        }
        LaunchType::File
    }

    /// Normalizes fields that can be inferred.
    pub fn normalize(&mut self) {
        self.launch_type = self.infer_launch_type();
    }

    /// The original `.lnk` path, when present.
    #[must_use]
    pub fn shortcut(&self) -> Option<&str> {
        self.shortcut_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
    }

    /// The path that should be handed to the shell when launching.
    #[must_use]
    pub fn launch_path(&self) -> &str {
        match &self.shortcut_path {
            Some(path) if !path.is_empty() => path,
            _ => &self.target_path,
        }
    }

    /// Returns `true` when this item has a usable path.
    #[must_use]
    pub fn has_path(&self) -> bool {
        self.kind == ItemKind::Separator
            || !self.target_path.trim().is_empty()
            || !self.launch_path().trim().is_empty()
    }

    /// Human readable name, falling back to the file name.
    #[must_use]
    pub fn label(&self) -> &str {
        if self.display_name.trim().is_empty() {
            file_stem(&self.target_path)
        } else {
            &self.display_name
        }
    }
}

/// Dock placement. Only [`DockPosition::Bottom`] is implemented in v0.1 but the
/// field is persisted so future versions can migrate without data loss.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    /// Dock along the bottom edge of the work area.
    #[default]
    Bottom,
    /// Dock along the top edge.
    Top,
    /// Dock along the left edge.
    Left,
    /// Dock along the right edge.
    Right,
}

/// Z-order behavior of the dock window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockLayer {
    /// Just above the wallpaper/desktop, below normal application windows.
    #[default]
    Desktop,
    /// Desktop level at rest, raised above normal windows during interaction.
    Dynamic,
    /// Always above normal windows.
    Topmost,
}

/// Font backend preference for text rendered inside the Dock itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DockFont {
    /// Use Windows DirectWrite shared system font cache (`Segoe UI` + OS CJK fallback).
    ///
    /// Keeps resident Dock memory minimal (typically under 5 MB).
    #[default]
    System,
    /// Load the bundled variable font (`assets/fonts/NotoSansSC-VF.ttf`).
    ///
    /// Increases Dock memory usage noticeably compared to the system font.
    NotoSansSc,
}

impl DockFont {
    /// Stable identifier used in config, settings UI, and diagnostics.
    #[must_use]
    pub const fn as_id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::NotoSansSc => "noto-sans-sc",
        }
    }

    /// Parses a UI or configuration identifier, falling back to [`Self::System`].
    #[must_use]
    pub fn from_id(value: &str) -> Self {
        match value {
            "noto-sans-sc" => Self::NotoSansSc,
            _ => Self::System,
        }
    }

    /// Human-readable label shown in diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "Windows 系统字体（DirectWrite 共享缓存 · 省内存）",
            Self::NotoSansSc => "内置中文字体（Noto Sans SC · 额外映射与缓存开销）",
        }
    }
}

impl Serialize for DockFont {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_id())
    }
}

impl<'de> Deserialize<'de> for DockFont {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_id(&value))
    }
}

/// Visual appearance, expressed in device independent pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Icon edge length in DIP.
    pub icon_size: f32,
    /// Gap between two icon slots in DIP.
    pub spacing: f32,
    /// Inner padding of the dock background in DIP.
    pub padding: f32,
    /// Dock background corner radius in DIP.
    pub corner_radius: f32,
    /// Background opacity, `0.0..=1.0`.
    pub opacity: f32,
    /// Color theme selection.
    pub theme: ThemeMode,
    /// Accent color used for indicators and selection.
    pub accent: AccentColor,
    /// Dock position.
    pub position: DockPosition,
    /// Z-order behavior.
    #[serde(default)]
    pub layer: DockLayer,
    /// Distance from the work-area edge in DIP.
    pub edge_margin: f32,
    /// Use the Windows.UI.Composition host-backdrop material behind the dock
    /// glass.
    ///
    /// Falls back to the regular translucent gradient when the system does not
    /// support `HostBackdropBrush` (for example Windows 10). The fallback never
    /// captures the screen.
    #[serde(default = "default_true")]
    pub frosted_glass: bool,
    /// Prefer hardware rendering for the main Dock window.
    pub gpu_acceleration: bool,
    /// Font used for labels and context menus rendered inside the Dock.
    #[serde(default)]
    pub dock_font: DockFont,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            icon_size: 48.0,
            spacing: 18.0,
            padding: 14.0,
            corner_radius: 20.0,
            opacity: 0.78,
            theme: ThemeMode::System,
            accent: AccentColor::Blue,
            position: DockPosition::Bottom,
            layer: DockLayer::Desktop,
            edge_margin: 10.0,
            frosted_glass: true,
            gpu_acceleration: false,
            dock_font: DockFont::System,
        }
    }
}

impl Appearance {
    /// Clamps every field into its supported range.
    pub fn normalize(&mut self) {
        self.icon_size = clamp_finite(self.icon_size, 24.0, 128.0, 48.0);
        self.spacing = clamp_finite(self.spacing, 0.0, 48.0, 10.0);
        self.padding = clamp_finite(self.padding, 4.0, 48.0, 14.0);
        self.corner_radius = clamp_finite(self.corner_radius, 0.0, 64.0, 20.0);
        self.opacity = clamp_finite(self.opacity, 0.2, 1.0, 0.78);
        self.edge_margin = clamp_finite(self.edge_margin, 0.0, 200.0, 10.0);
    }
}

/// One of the six hover animation presets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HoverPreset {
    /// Apple-style distance falloff magnification (default).
    #[default]
    GlideZoom,
    /// Gentle vertical hop on hover.
    Bounce,
    /// Staggered wave travelling from the hovered icon outwards.
    Wave,
    /// Press-led interaction with a subtle hover cue.
    FluidPress,
    /// Soft accent halo with a breathing pulse.
    Glow,
    /// Small 3D-looking tilt that follows the pointer.
    Perspective,
}

impl HoverPreset {
    /// All presets in settings order.
    pub const ALL: [Self; 6] = [
        Self::GlideZoom,
        Self::Bounce,
        Self::Wave,
        Self::FluidPress,
        Self::Glow,
        Self::Perspective,
    ];

    /// Stable identifier used by the configuration file and the settings app.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::GlideZoom => "glide_zoom",
            Self::Bounce => "bounce",
            Self::Wave => "wave",
            Self::FluidPress => "fluid_press",
            Self::Glow => "glow",
            Self::Perspective => "perspective",
        }
    }

    /// Short human readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::GlideZoom => "Glide Zoom",
            Self::Bounce => "Bounce",
            Self::Wave => "Wave",
            Self::FluidPress => "Fluid Press",
            Self::Glow => "Glow",
            Self::Perspective => "Perspective",
        }
    }

    /// Parses a persisted identifier. Unknown values are rejected so the caller
    /// can fall back to the default instead of failing the whole configuration.
    #[must_use]
    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.id() == value)
    }

    /// Suggested launch preset shown as a hint in the settings app.
    #[must_use]
    pub fn recommended_launch(self) -> LaunchPreset {
        match self {
            Self::Bounce => LaunchPreset::TripleBounce,
            Self::Wave | Self::Glow => LaunchPreset::RippleLaunch,
            Self::GlideZoom | Self::FluidPress | Self::Perspective => LaunchPreset::GlideLaunch,
        }
    }
}

impl Serialize for HoverPreset {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for HoverPreset {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_id(&value).unwrap_or_default())
    }
}

/// One of the four application launch animation presets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LaunchPreset {
    /// Quick grow, lift and settle (default).
    #[default]
    GlideLaunch,
    /// Three decreasing bounces.
    TripleBounce,
    /// Expanding accent ring with a small icon scale.
    RippleLaunch,
    /// Stronger halo and scale pulse.
    PulseLaunch,
}

impl LaunchPreset {
    /// All presets in settings order.
    pub const ALL: [Self; 4] = [
        Self::GlideLaunch,
        Self::TripleBounce,
        Self::RippleLaunch,
        Self::PulseLaunch,
    ];

    /// Stable identifier used by the configuration file and the settings app.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::GlideLaunch => "glide_launch",
            Self::TripleBounce => "triple_bounce",
            Self::RippleLaunch => "ripple_launch",
            Self::PulseLaunch => "pulse_launch",
        }
    }

    /// Short human readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::GlideLaunch => "Glide Launch",
            Self::TripleBounce => "Triple Bounce",
            Self::RippleLaunch => "Ripple Launch",
            Self::PulseLaunch => "Pulse Launch",
        }
    }

    /// Parses a persisted identifier.
    #[must_use]
    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.id() == value)
    }
}

impl Serialize for LaunchPreset {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for LaunchPreset {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_id(&value).unwrap_or_default())
    }
}

/// All persisted motion settings.
///
/// The UI writes this section; the runtime derives spring constants and
/// preset-specific timelines from it inside `dock_core::motion`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionConfig {
    /// Master switch for every visual animation.
    pub enabled: bool,
    /// Honour the Windows "animation effects" accessibility setting.
    pub respect_reduced_motion: bool,
    /// Hover animation preset.
    pub hover_preset: HoverPreset,
    /// Launch animation preset.
    pub launch_preset: LaunchPreset,
    /// Maximum icon scale under the pointer.
    pub max_scale: f32,
    /// Magnification attenuation radius in icon slots.
    pub range_slots: f32,
    /// Baseline hover transition duration in milliseconds (180 ms = 1.0×).
    pub hover_ms: u32,
    /// Spring elasticity in `0..=1`; higher means more bounce.
    pub elasticity: f32,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            respect_reduced_motion: true,
            hover_preset: HoverPreset::GlideZoom,
            launch_preset: LaunchPreset::GlideLaunch,
            max_scale: 1.35,
            range_slots: 1.7,
            hover_ms: 180,
            elasticity: 0.75,
        }
    }
}

impl MotionConfig {
    /// Clamps every field into its supported range.
    pub fn normalize(&mut self) {
        self.max_scale = clamp_finite(self.max_scale, 1.05, MAX_HOVER_SCALE, 1.35);
        self.range_slots = clamp_finite(self.range_slots, 0.5, 8.0, 1.7);
        self.hover_ms = clamp_u32(self.hover_ms, 80, 400, 180);
        self.elasticity = clamp_finite(self.elasticity, 0.0, 1.0, 0.75);
    }
}

/// Interaction and animation behavior toggles.
///
/// Numeric motion parameters live in [`MotionConfig`]; this struct only keeps
/// user-facing behaviour switches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    /// Enables the cursor driven hover layer.
    pub magnification: bool,
    /// Auto hide (M3, persisted but inert in v0.1).
    pub auto_hide: bool,
    /// Delay before hiding, in milliseconds.
    pub auto_hide_delay_ms: u32,
    /// Play the selected launch animation when an item starts.
    pub launch_animation: bool,
    /// Show the hovered item's name above the dock.
    pub show_tooltips: bool,
    /// Show a dot below running applications.
    pub show_running_indicator: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            magnification: true,
            auto_hide: false,
            auto_hide_delay_ms: 450,
            launch_animation: true,
            show_tooltips: true,
            show_running_indicator: true,
        }
    }
}

impl Behavior {
    /// Clamps every field into its supported range.
    pub fn normalize(&mut self) {
        self.auto_hide_delay_ms = self.auto_hide_delay_ms.min(10_000);
    }
}

/// Monitor selection and offset. Consumed by the multi-monitor milestone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// Display device id; empty selects the primary monitor.
    pub display_id: String,
    /// Horizontal offset from the centered position, in DIP.
    pub offset: f32,
}

impl MonitorConfig {
    /// Clamps every field into its supported range.
    pub fn normalize(&mut self) {
        self.offset = clamp_finite(self.offset, -500.0, 500.0, 0.0);
    }
}

/// The complete persisted configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockConfig {
    /// Schema version.
    pub version: u32,
    /// Visual appearance.
    pub appearance: Appearance,
    /// Behavior.
    pub behavior: Behavior,
    /// Motion presets and timing parameters.
    pub motion: MotionConfig,
    /// Monitor selection.
    pub monitor: MonitorConfig,
    /// Ordered dock entries.
    pub items: Vec<DockItem>,
    /// Next id to hand out.
    pub next_item_id: u32,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            appearance: Appearance::default(),
            behavior: Behavior::default(),
            motion: MotionConfig::default(),
            monitor: MonitorConfig::default(),
            items: Vec::new(),
            next_item_id: 1,
        }
    }
}

impl DockConfig {
    /// Repairs the configuration in place: clamped ranges, unique ids, unique
    /// target paths, trimmed labels and a consistent `next_item_id`.
    ///
    /// The repair is intentionally conservative: entries are never dropped
    /// because their target is missing, only entries without any path at all.
    pub fn normalize(&mut self) {
        if self.version == 0 {
            self.version = CONFIG_VERSION;
        }
        self.appearance.normalize();
        self.behavior.normalize();
        self.motion.normalize();
        self.monitor.normalize();
        for item in &mut self.items {
            item.normalize();
        }

        self.items.retain(|item| item.has_path());
        for item in &mut self.items {
            if item.kind == ItemKind::Separator {
                item.display_name = "分隔线".to_string();
                item.target_path.clear();
                item.shortcut_path = None;
                item.arguments = None;
                item.working_directory = None;
                continue;
            }
            item.display_name = item.display_name.trim().to_string();
            if item.display_name.is_empty() {
                item.display_name = file_stem(&item.target_path).to_string();
            }
            if item.display_name.is_empty() {
                item.display_name = "Untitled".to_string();
            }
            if let Some(arguments) = &item.arguments
                && arguments.trim().is_empty()
            {
                item.arguments = None;
            }
            if let Some(dir) = &item.working_directory
                && dir.trim().is_empty()
            {
                item.working_directory = None;
            }
        }

        // De-duplicate on the resolved target path, case-insensitively, keeping
        // the first occurrence so the user's ordering wins.
        let mut seen: Vec<String> = Vec::with_capacity(self.items.len());
        self.items.retain(|item| {
            if item.kind == ItemKind::Separator {
                return true;
            }
            let key = item.target_path.trim().to_lowercase();
            if seen.iter().any(|existing| existing == &key) {
                return false;
            }
            seen.push(key);
            true
        });

        // Make sure every item owns a unique, non-zero id. The first occurrence
        // of a duplicated id keeps it; every other entry gets the smallest free
        // value that is not reserved by a uniquely identified item.
        let mut counts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
        for item in &self.items {
            if item.id != 0 {
                *counts.entry(item.id).or_insert(0) += 1;
            }
        }
        let mut assigned: Vec<u32> = Vec::with_capacity(self.items.len());
        for item in &mut self.items {
            let uniquely_kept = item.id != 0 && counts.get(&item.id).copied() == Some(1);
            if uniquely_kept {
                assigned.push(item.id);
                continue;
            }
            if item.id != 0 && !assigned.contains(&item.id) {
                assigned.push(item.id);
                continue;
            }
            let mut candidate = 1u32;
            while counts.contains_key(&candidate) || assigned.contains(&candidate) {
                candidate += 1;
            }
            item.id = candidate;
            assigned.push(candidate);
        }

        let max_id = self.items.iter().map(|item| item.id).max().unwrap_or(0);
        self.next_item_id = self.next_item_id.max(max_id + 1).max(1);
    }

    /// Allocates the next unused id.
    pub fn allocate_id(&mut self) -> u32 {
        let mut id = self.next_item_id.max(1);
        while self.items.iter().any(|item| item.id == id) {
            id += 1;
        }
        self.next_item_id = id + 1;
        id
    }

    /// Returns `true` when the configuration was written by a newer version.
    #[must_use]
    pub fn is_from_future(&self) -> bool {
        self.version > CONFIG_VERSION
    }

    /// Appends an item unless its target path is already present.
    ///
    /// Returns the index of the inserted (or existing) entry.
    pub fn upsert_item(&mut self, item: DockItem) -> usize {
        if item.kind == ItemKind::Separator {
            self.items.push(item);
            return self.items.len() - 1;
        }
        let key = item.target_path.trim().to_lowercase();
        if let Some(index) = self
            .items
            .iter()
            .position(|existing| existing.target_path.trim().to_lowercase() == key)
        {
            return index;
        }
        self.items.push(item);
        self.items.len() - 1
    }
}

/// Extracts the file stem from a Windows or POSIX style path.
#[must_use]
pub fn file_stem(path: &str) -> &str {
    let name = path
        .rsplit(['\\', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path);
    match name.rfind('.') {
        Some(index) if index > 0 => &name[..index],
        _ => name,
    }
}

/// Lowercase extension of a path or URL, without the dot.
#[must_use]
pub fn file_extension(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Returns `true` when the target is a package AUMID or an AppsFolder path.
#[must_use]
pub fn is_packaged_target(target: &str) -> bool {
    let target = target.trim();
    if target
        .to_ascii_lowercase()
        .starts_with("shell:appsfolder\\")
    {
        return true;
    }
    // AUMIDs look like `PackageFamilyName!AppId`: no slash, backslash or colon.
    target.contains('!') && !target.contains('\\') && !target.contains('/') && !target.contains(':')
}

/// Returns `true` when the string is an http(s) URL.
#[must_use]
pub fn is_url(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn clamp_finite(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn clamp_u32(value: u32, min: u32, max: u32, fallback: u32) -> u32 {
    if value == 0 {
        fallback
    } else {
        value.clamp(min, max)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item(id: u32, name: &str, path: &str) -> DockItem {
        DockItem {
            id,
            kind: ItemKind::Application,
            display_name: name.to_string(),
            target_path: path.to_string(),
            ..DockItem::default()
        }
    }

    #[test]
    fn separators_survive_normalization_and_are_not_deduplicated() {
        let mut config = DockConfig::default();
        config.items.push(DockItem::separator(2));
        config.items.push(DockItem::separator(3));
        config.normalize();
        assert_eq!(config.items.len(), 2);
        assert!(
            config
                .items
                .iter()
                .all(|item| item.kind == ItemKind::Separator && item.display_name == "分隔线")
        );
    }

    #[test]
    fn start_widget_is_stable_and_deduplicated_by_action() {
        let mut config = DockConfig::default();
        config
            .items
            .push(DockItem::widget(1, "开始菜单", "start_menu"));
        config
            .items
            .push(DockItem::widget(2, "开始菜单", "start_menu"));
        config.normalize();
        assert_eq!(config.items.len(), 1);
        assert_eq!(config.items[0].kind, ItemKind::Widget);
        assert_eq!(config.items[0].target_path, "start_menu");
    }

    #[test]
    fn dock_layer_defaults_to_desktop_and_parses_snake_case() {
        assert_eq!(Appearance::default().layer, DockLayer::Desktop);
        let appearance: Appearance =
            serde_json::from_str(r#"{ "layer": "dynamic" }"#).expect("layer");
        assert_eq!(appearance.layer, DockLayer::Dynamic);
    }

    #[test]
    fn launch_types_are_inferred_from_targets() {
        let exe = sample_item(1, "Notepad", "C:\\Windows\\notepad.exe");
        assert_eq!(exe.infer_launch_type(), LaunchType::Exe);

        let folder = DockItem {
            kind: ItemKind::Folder,
            target_path: "D:\\Projects".into(),
            ..DockItem::default()
        };
        assert_eq!(folder.infer_launch_type(), LaunchType::Folder);

        let shortcut = DockItem {
            target_path: "C:\\Users\\me\\Desktop\\App.lnk".into(),
            shortcut_path: Some("C:\\Users\\me\\Desktop\\App.lnk".into()),
            ..DockItem::default()
        };
        assert_eq!(shortcut.infer_launch_type(), LaunchType::Shortcut);

        let packaged = DockItem::packaged_app(
            2,
            "Calculator",
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
        );
        assert_eq!(packaged.infer_launch_type(), LaunchType::PackagedApp);

        let mut explicit = exe;
        explicit.launch_type = LaunchType::File;
        assert_eq!(explicit.infer_launch_type(), LaunchType::File);
    }

    #[test]
    fn default_config_is_empty_and_versioned() {
        let config = DockConfig::default();
        assert_eq!(config.version, CONFIG_VERSION);
        assert!(config.items.is_empty());
        assert_eq!(config.next_item_id, 1);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let json = r#"{ "items": [ { "id": 7, "display_name": "Notepad", "target_path": "C:\\Windows\\notepad.exe" } ] }"#;
        let mut config: DockConfig = serde_json::from_str(json).expect("valid json");
        config.normalize();
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.appearance.icon_size, 48.0);
        assert_eq!(config.items.len(), 1);
        assert_eq!(config.items[0].kind, ItemKind::Application);
        assert_eq!(config.items[0].launch_type, LaunchType::Exe);
    }

    #[test]
    fn round_trip_preserves_items() {
        let mut config = DockConfig::default();
        config.items.push(DockItem {
            id: 3,
            kind: ItemKind::Folder,
            launch_type: LaunchType::Folder,
            display_name: "Projects".into(),
            target_path: "D:\\Projects".into(),
            shortcut_path: Some("C:\\Users\\me\\Desktop\\Projects.lnk".into()),
            arguments: Some("--flag".into()),
            working_directory: Some("D:\\Projects".into()),
        });
        config.normalize();
        let json = serde_json::to_string(&config).expect("serialize");
        let restored: DockConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, config);
    }

    #[test]
    fn deduplicates_case_insensitively() {
        let mut config = DockConfig::default();
        config.items.push(sample_item(1, "A", "C:\\Apps\\Tool.exe"));
        config.items.push(sample_item(2, "B", "c:\\apps\\tool.EXE"));
        config.normalize();
        assert_eq!(config.items.len(), 1);
        assert_eq!(config.items[0].display_name, "A");
    }

    #[test]
    fn fixes_duplicate_and_zero_ids() {
        let mut config = DockConfig::default();
        config.items.push(sample_item(4, "A", "C:\\a.exe"));
        config.items.push(sample_item(4, "B", "C:\\b.exe"));
        config.items.push(sample_item(0, "C", "C:\\c.exe"));
        config.normalize();
        let mut ids: Vec<u32> = config.items.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2, 4]);
        assert_eq!(config.next_item_id, 5);
    }

    #[test]
    fn clamps_invalid_ranges() {
        let mut appearance = Appearance {
            icon_size: f32::NAN,
            spacing: -4.0,
            padding: 999.0,
            opacity: 4.0,
            ..Appearance::default()
        };
        appearance.normalize();
        assert_eq!(appearance.icon_size, 48.0);
        assert_eq!(appearance.spacing, 0.0);
        assert_eq!(appearance.padding, 48.0);
        assert_eq!(appearance.opacity, 1.0);
    }

    #[test]
    fn gpu_acceleration_preference_is_backwards_compatible() {
        let mut config: DockConfig =
            serde_json::from_str(r#"{"appearance":{"frosted_glass":false}}"#)
                .expect("load an existing configuration");
        assert!(!config.appearance.gpu_acceleration);

        config.appearance.gpu_acceleration = true;
        let saved = serde_json::to_string(&config).expect("save the enabled preference");
        let reloaded: DockConfig = serde_json::from_str(&saved).expect("reload the preference");
        assert!(reloaded.appearance.gpu_acceleration);
    }

    #[test]
    fn label_falls_back_to_file_stem() {
        let item = sample_item(1, "", "C:\\Program Files\\My App\\my app.exe");
        assert_eq!(item.label(), "my app");
    }

    #[test]
    fn upsert_keeps_existing_entry() {
        let mut config = DockConfig::default();
        config.upsert_item(sample_item(1, "A", "C:\\a.exe"));
        let index = config.upsert_item(sample_item(2, "A again", "c:\\A.exe"));
        assert_eq!(index, 0);
        assert_eq!(config.items.len(), 1);
    }

    #[test]
    fn allocate_id_skips_used_values() {
        let mut config = DockConfig::default();
        config.items.push(sample_item(1, "A", "C:\\a.exe"));
        config.next_item_id = 1;
        assert_eq!(config.allocate_id(), 2);
        assert_eq!(config.next_item_id, 3);
    }

    #[test]
    fn motion_defaults_match_the_spec() {
        let motion = MotionConfig::default();
        assert_eq!(motion.hover_preset, HoverPreset::GlideZoom);
        assert_eq!(motion.launch_preset, LaunchPreset::GlideLaunch);
        assert!((motion.max_scale - 1.35).abs() < f32::EPSILON);
        assert!((motion.range_slots - 1.7).abs() < f32::EPSILON);
        assert_eq!(motion.hover_ms, 180);
        assert!((motion.elasticity - 0.75).abs() < f32::EPSILON);
        assert!(motion.enabled);
        assert!(motion.respect_reduced_motion);
    }

    #[test]
    fn motion_normalizes_invalid_values() {
        let mut motion = MotionConfig {
            max_scale: f32::NAN,
            range_slots: -1.0,
            hover_ms: 0,
            elasticity: 4.0,
            ..MotionConfig::default()
        };
        motion.normalize();
        assert!((motion.max_scale - 1.35).abs() < f32::EPSILON);
        assert!((motion.range_slots - 0.5).abs() < f32::EPSILON);
        assert_eq!(motion.hover_ms, 180);
        assert!((motion.elasticity - 1.0).abs() < f32::EPSILON);

        motion.max_scale = 10.0;
        motion.normalize();
        assert!((motion.max_scale - MAX_HOVER_SCALE).abs() < f32::EPSILON);
    }

    #[test]
    fn preset_ids_round_trip_and_unknowns_fall_back() {
        for preset in HoverPreset::ALL {
            let json = serde_json::to_string(&preset).expect("serialize");
            let restored: HoverPreset = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, preset);
        }
        for preset in LaunchPreset::ALL {
            let json = serde_json::to_string(&preset).expect("serialize");
            let restored: LaunchPreset = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, preset);
        }
        let unknown: HoverPreset = serde_json::from_str("\"disco\"").expect("fallback");
        assert_eq!(unknown, HoverPreset::GlideZoom);
        let unknown: LaunchPreset = serde_json::from_str("\"moon\"").expect("fallback");
        assert_eq!(unknown, LaunchPreset::GlideLaunch);
    }

    #[test]
    fn recommendations_cover_every_hover_preset() {
        for preset in HoverPreset::ALL {
            let _ = preset.recommended_launch();
            assert!(!preset.label().is_empty());
        }
        for preset in LaunchPreset::ALL {
            assert!(!preset.label().is_empty());
        }
    }

    #[test]
    fn dock_font_defaults_to_system_and_unknowns_fall_back() {
        assert_eq!(Appearance::default().dock_font, DockFont::System);

        let legacy: Appearance = serde_json::from_str("{}").expect("legacy appearance");
        assert_eq!(legacy.dock_font, DockFont::System);

        let mut config = DockConfig::default();
        config.appearance.dock_font = DockFont::NotoSansSc;
        let saved = serde_json::to_string(&config).expect("serialize dock font");
        let reloaded: DockConfig = serde_json::from_str(&saved).expect("deserialize dock font");
        assert_eq!(reloaded.appearance.dock_font, DockFont::NotoSansSc);

        let unknown: DockFont = serde_json::from_str("\"comic-sans\"").expect("fallback");
        assert_eq!(unknown, DockFont::System);
        assert_eq!(DockFont::from_id("unknown"), DockFont::System);
        assert_eq!(DockFont::from_id("noto-sans-sc"), DockFont::NotoSansSc);
    }
}
