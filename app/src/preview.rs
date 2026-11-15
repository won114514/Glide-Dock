//! Offline preview rendering with real application icons.
//!
//! `glide-dock --render-preview [dir]` uses the same renderer as the live dock but
//! composites the result over a generated wallpaper so it can be reviewed
//! without a desktop session.

use std::collections::BTreeSet;
use std::path::Path;

use dock_core::layout::{FrameLayout, Metrics, target_scales};
use dock_core::models::DockConfig;
use dock_core::state::{DockState, MenuAction, MenuEntry, MenuIcon};
use dock_core::{
    DockGeometry, HoverPreset, LaunchPreset, MotionEngine, MotionTuning, Palette, Point, Rect,
    ResolvedTheme,
};
use dock_platform_windows::{expand_environment, extract_icon, resolve};
use dock_render::icons::IconMap;
use dock_render::preview;
use dock_render::{DockScene, DragVisual, IconAnimation, MenuScene, Renderer};
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

/// Curated demo applications (name, candidate paths in priority order).
const SAMPLE_APPS: &[(&str, &[&str])] = &[
    ("文件资源管理器", &["C:\\Windows\\explorer.exe"]),
    (
        "Microsoft Edge",
        &[
            "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
            "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
        ],
    ),
    (
        "终端",
        &[
            "%LOCALAPPDATA%\\Microsoft\\WindowsApps\\wt.exe",
            "C:\\Windows\\System32\\cmd.exe",
        ],
    ),
    (
        "PowerShell",
        &["C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"],
    ),
    ("计算器", &["C:\\Windows\\System32\\calc.exe"]),
    ("记事本", &["C:\\Windows\\System32\\notepad.exe"]),
    ("任务管理器", &["C:\\Windows\\System32\\Taskmgr.exe"]),
    (
        "设置",
        &[
            "C:\\Windows\\ImmersiveControlPanel\\SystemSettings.exe",
            "C:\\Windows\\System32\\control.exe",
        ],
    ),
];

/// Builds the sample configuration used by `--demo` and the previews.
#[must_use]
pub fn demo_config() -> DockConfig {
    let mut config = DockConfig::default();
    for (name, candidates) in SAMPLE_APPS {
        let path = candidates
            .iter()
            .map(|candidate| expand_environment(candidate))
            .find(|candidate| Path::new(candidate).exists())
            .unwrap_or_else(|| expand_environment(candidates[0]));
        let resolved = resolve(&path);
        let mut item = resolved.into_item(0);
        item.display_name = (*name).to_string();
        item.id = config.allocate_id();
        config.items.push(item);
    }
    config
}

/// Renders the full preview gallery into `directory`.
pub fn render_previews(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(directory)?;
    let config = demo_config();
    let icons = load_icons(&config);
    let mut renderer = Renderer::new();

    let dark = ResolvedTheme::Dark;
    let light = ResolvedTheme::Light;

    // 1. Dark dock with hover magnification and running indicators.
    let running = [config.items[0].id, config.items[1].id];
    let dark_dock = build_dock(
        &mut renderer,
        &config,
        &icons,
        dark,
        Some(2),
        false,
        None,
        &running,
    )
    .ok_or("failed to render dark dock")?;
    let dark_desktop = compose(&dark_dock, dark)?;
    dark_desktop.save_png(directory.join("01-dock-dark.png"))?;

    // 2. Light theme.
    let light_dock = build_dock(
        &mut renderer,
        &config,
        &icons,
        light,
        Some(2),
        false,
        None,
        &running,
    )
    .ok_or("failed to render light dock")?;
    let light_desktop = compose(&light_dock, light)?;
    light_desktop.save_png(directory.join("02-dock-light.png"))?;

    // 3. Empty state.
    let empty = DockConfig::default();
    let empty_dock = build_dock(
        &mut renderer,
        &empty,
        &IconMap::new(),
        dark,
        None,
        false,
        None,
        &[],
    )
    .ok_or("failed to render empty dock")?;
    let empty_desktop = compose(&empty_dock, dark)?;
    empty_desktop.save_png(directory.join("03-dock-empty.png"))?;

    // 4. Tooltip.
    let tooltip_dock = build_dock(
        &mut renderer,
        &config,
        &icons,
        dark,
        Some(1),
        true,
        None,
        &running,
    )
    .ok_or("failed to render tooltip")?;
    let tooltip_desktop = compose(&tooltip_dock, dark)?;
    tooltip_desktop.save_png(directory.join("04-dock-tooltip.png"))?;

    // 5. Context menu.
    let menu_index = 1;
    let mut menu_dock = build_dock(
        &mut renderer,
        &config,
        &icons,
        dark,
        Some(menu_index),
        false,
        None,
        &running,
    )
    .ok_or("failed to render menu dock")?;
    add_menu_overlay(&mut renderer, &mut menu_dock, &config, dark, menu_index)?;
    let menu_desktop = compose(&menu_dock, dark)?;
    menu_desktop.save_png(directory.join("05-dock-menu.png"))?;

    // 6. Close-up of the menu, best for design review.
    let (dock_x, dock_y) = desktop_dock_origin(&menu_dock);
    let closeup = crop(
        &menu_desktop,
        (dock_x - 40).max(0),
        (dock_y + menu_dock.height() as i32 - 360).max(0),
        (menu_dock.width() + 80).min(menu_desktop.width()),
        380.min(menu_desktop.height()),
    );
    if let Some(closeup) = closeup {
        closeup.save_png(directory.join("06-menu-closeup.png"))?;
    }

    // 7. Drag reorder.
    let drag_dock = build_dock(
        &mut renderer,
        &config,
        &icons,
        dark,
        None,
        false,
        Some(2),
        &running,
    )
    .ok_or("failed to render drag")?;
    let drag_desktop = compose(&drag_dock, dark)?;
    drag_desktop.save_png(directory.join("07-dock-drag.png"))?;

    // 8. Tooltip close-up.
    let (tooltip_x, tooltip_y) = desktop_dock_origin(&tooltip_dock);
    let tooltip_closeup = crop(
        &tooltip_desktop,
        (tooltip_x - 30).max(0),
        (tooltip_y + tooltip_dock.height() as i32 - 180).max(0),
        (tooltip_dock.width() + 60).min(tooltip_desktop.width()),
        210,
    );
    if let Some(closeup) = tooltip_closeup {
        closeup.save_png(directory.join("08-tooltip-closeup.png"))?;
    }

    // 9. Failure feedback: unresolved badge plus error bubble.
    if let Some(error_dock) = build_error_dock(&mut renderer, &config, &icons, dark) {
        let error_desktop = compose(&error_dock, dark)?;
        error_desktop.save_png(directory.join("09-error-dark.png"))?;
        let (error_x, error_y) = desktop_dock_origin(&error_dock);
        let error_closeup = crop(
            &error_desktop,
            (error_x - 30).max(0),
            (error_y + error_dock.height() as i32 - 300).max(0),
            (error_dock.width() + 60).min(error_desktop.width()),
            340,
        );
        if let Some(closeup) = error_closeup {
            closeup.save_png(directory.join("10-error-closeup.png"))?;
        }
    }

    // 15-24. Motion presets, rendered through the same engine as the live dock.
    render_motion_gallery(directory, &config, &icons, dark, &mut renderer)?;

    tracing::info!(directory = %directory.display(), items = config.items.len(), "previews rendered");
    Ok(())
}

fn render_motion_gallery(
    directory: &Path,
    config: &DockConfig,
    icons: &IconMap,
    theme: ResolvedTheme,
    renderer: &mut Renderer,
) -> Result<(), Box<dyn std::error::Error>> {
    let hover_cases = [
        (HoverPreset::GlideZoom, "15-motion-glide-zoom.png"),
        (HoverPreset::Bounce, "16-motion-bounce.png"),
        (HoverPreset::Wave, "17-motion-wave.png"),
        (HoverPreset::FluidPress, "18-motion-fluid-press.png"),
        (HoverPreset::Glow, "19-motion-glow.png"),
        (HoverPreset::Perspective, "20-motion-perspective.png"),
    ];
    for (preset, name) in hover_cases {
        let mut preset_config = config.clone();
        preset_config.motion.hover_preset = preset;
        preset_config.motion.launch_preset = LaunchPreset::GlideLaunch;
        if let Some(dock) = build_motion_dock(renderer, &preset_config, icons, theme, None) {
            let desktop = compose(&dock, theme)?;
            desktop.save_png(directory.join(name))?;
        }
    }

    let launch_cases = [
        (LaunchPreset::GlideLaunch, 0.100, "21-launch-glide.png"),
        (
            LaunchPreset::TripleBounce,
            0.100,
            "22-launch-triple-bounce.png",
        ),
        (LaunchPreset::RippleLaunch, 0.220, "23-launch-ripple.png"),
        (LaunchPreset::PulseLaunch, 0.130, "24-launch-pulse.png"),
    ];
    for (preset, elapsed, name) in launch_cases {
        let mut preset_config = config.clone();
        preset_config.motion.hover_preset = HoverPreset::GlideZoom;
        preset_config.motion.launch_preset = preset;
        if let Some(dock) = build_motion_dock(renderer, &preset_config, icons, theme, Some(elapsed))
        {
            let desktop = compose(&dock, theme)?;
            desktop.save_png(directory.join(name))?;
        }
    }
    Ok(())
}

/// Renders one deterministic motion frame through the live engine.
///
/// `launch_elapsed` selects a launch snapshot; `None` renders the hover preset
/// at its settled peak.
fn build_motion_dock(
    renderer: &mut Renderer,
    config: &DockConfig,
    icons: &IconMap,
    theme: ResolvedTheme,
    launch_elapsed: Option<f32>,
) -> Option<Pixmap> {
    let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 1.0);
    let geometry = DockGeometry::new(config.items.len(), &metrics);
    let mut engine = MotionEngine::new(MotionTuning::new(&config.behavior, &config.motion, true));
    let running = BTreeSet::new();
    let index = config.items.len().min(4).saturating_sub(1);
    let mut now = 0.0;
    let mut snapshot = engine.update(now, 0.0, &config.items, &geometry, &metrics, &running);

    if let Some(target_elapsed) = launch_elapsed {
        engine.set_pointer(0.0, None, None);
        engine.update(0.0, 0.0, &config.items, &geometry, &metrics, &running);
        let id = config.items.get(index)?.id;
        engine.pointer_down(0.0, id);
        now = 0.075;
        engine.update(
            now,
            1.0 / 60.0,
            &config.items,
            &geometry,
            &metrics,
            &running,
        );
        engine.pointer_up(now, id);
        engine.update(now, 0.0, &config.items, &geometry, &metrics, &running);
        engine.begin_launch(now, id, true);
        while now < 0.075 + target_elapsed {
            now += 1.0 / 60.0;
            snapshot = engine.update(
                now,
                1.0 / 60.0,
                &config.items,
                &geometry,
                &metrics,
                &running,
            );
        }
    } else {
        let cursor = Point::new(
            geometry.slot_centers().get(index).copied()?,
            geometry.content_base.center().y,
        );
        engine.set_pointer(0.0, Some(cursor), Some(index));
        let frames = if config.motion.hover_preset == HoverPreset::Wave {
            7
        } else {
            30
        };
        for _ in 0..frames {
            now += 1.0 / 60.0;
            snapshot = engine.update(
                now,
                1.0 / 60.0,
                &config.items,
                &geometry,
                &metrics,
                &running,
            );
        }
    }

    let layout = FrameLayout::compute(&geometry, &metrics, &snapshot.layout_scales);
    let animations: Vec<IconAnimation> = snapshot
        .icons
        .iter()
        .map(|motion| IconAnimation {
            scale: Some(motion.scale),
            offset_x: motion.offset_x,
            offset_y: motion.offset_y,
            tilt_x_deg: motion.tilt_x_deg,
            tilt_y_deg: motion.tilt_y_deg,
            glow: motion.glow,
            ripple: motion.ripple,
            waiting: motion.waiting_opacity,
            running_dot_allowed: motion.running_dot_allowed,
        })
        .collect();
    let mut state = DockState::default();
    if launch_elapsed.is_none() {
        state.hovered = Some(index);
        state.cursor = Some(Point::new(
            geometry.slot_centers().get(index).copied()?,
            geometry.content_base.center().y,
        ));
        if let Some(item) = config.items.first() {
            state.running.insert(item.id);
        }
    }

    let scene = DockScene {
        config,
        metrics: &metrics,
        geometry: &geometry,
        layout: &layout,
        animations: &animations,
        state: &state,
        palette: palette_for(config, theme),
        icons,
        native_glass: false,
        tooltip_progress: 0.0,
        hover_progress: snapshot.hover_progress,
        error: None,
        error_progress: 0.0,
        drag: None,
        glass: false,
    };
    renderer.render_dock(&scene)
}

fn build_error_dock(
    renderer: &mut Renderer,
    config: &DockConfig,
    icons: &IconMap,
    theme: ResolvedTheme,
) -> Option<Pixmap> {
    let mut config = config.clone();
    let id = config.allocate_id();
    config.items.push(dock_core::DockItem {
        id,
        kind: dock_core::ItemKind::File,
        display_name: "损坏的快捷方式".to_string(),
        target_path: "C:\\ProgramData\\Missing\\broken-app.exe".to_string(),
        ..dock_core::DockItem::default()
    });
    let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 1.0);
    let geometry = DockGeometry::new(config.items.len(), &metrics);
    let index = config.items.len() - 1;
    let cursor = Point::new(
        geometry.slot_centers()[index],
        geometry.content_base.center().y,
    );
    let mut state = DockState {
        cursor: Some(cursor),
        hovered: Some(index),
        unresolved: std::iter::once(id).collect(),
        running: std::iter::once(config.items[0].id).collect(),
        ..DockState::default()
    };
    state.tooltip.index = None;
    let scales = target_scales(
        config.items.len(),
        Some(cursor),
        &geometry,
        &metrics,
        &config.behavior,
    );
    let layout = FrameLayout::compute(&geometry, &metrics, &scales);
    let scene = DockScene {
        config: &config,
        metrics: &metrics,
        geometry: &geometry,
        layout: &layout,
        animations: &[],
        state: &state,
        palette: palette_for(&config, theme),
        icons,
        native_glass: false,
        tooltip_progress: 0.0,
        hover_progress: 1.0,
        error: Some("无法启动「损坏的快捷方式」：系统找不到指定的文件（错误码 2）"),
        error_progress: 1.0,
        drag: None,
        glass: false,
    };
    renderer.render_dock(&scene)
}

fn load_icons(config: &DockConfig) -> IconMap {
    let mut icons = IconMap::new();
    for item in &config.items {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(shortcut) = &item.shortcut_path {
            candidates.push(shortcut.clone());
        }
        candidates.push(item.target_path.clone());
        let mut loaded = false;
        for path in candidates {
            if path.trim().is_empty() {
                continue;
            }
            match extract_icon(&path, 256) {
                Ok(image) => {
                    if let Some(pixmap) = icon_to_pixmap(image) {
                        icons.insert(item.id, pixmap);
                        loaded = true;
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!(%path, %error, "preview icon extraction failed");
                }
            }
        }
        if !loaded {
            tracing::warn!(name = item.display_name, "using generated placeholder icon");
        }
    }
    icons
}

fn icon_to_pixmap(image: dock_platform_windows::IconImage) -> Option<Pixmap> {
    let size = IntSize::from_wh(image.width, image.height)?;
    Pixmap::from_vec(image.rgba, size)
}

fn palette_for(config: &DockConfig, theme: ResolvedTheme) -> Palette {
    Palette::for_theme(theme).with_accent(config.appearance.accent)
}

#[allow(clippy::too_many_arguments)]
fn build_dock(
    renderer: &mut Renderer,
    config: &DockConfig,
    icons: &IconMap,
    theme: ResolvedTheme,
    hover_index: Option<usize>,
    tooltip: bool,
    drag_index: Option<usize>,
    running: &[u32],
) -> Option<Pixmap> {
    let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 1.0);
    let geometry = DockGeometry::new(config.items.len(), &metrics);
    let mut state = DockState::default();
    let cursor = hover_index.and_then(|index| {
        geometry
            .slot_centers()
            .get(index)
            .map(|center| Point::new(*center, geometry.content_base.center().y))
    });
    state.cursor = cursor;
    state.hovered = hover_index;
    if tooltip {
        state.tooltip.index = hover_index;
        state.tooltip.progress = 1.0;
    }
    for id in running {
        state.running.insert(*id);
    }
    let scales = target_scales(
        config.items.len(),
        cursor,
        &geometry,
        &metrics,
        &config.behavior,
    );
    let layout = FrameLayout::compute(&geometry, &metrics, &scales);

    let drag = drag_index.and_then(|index| {
        let icon = layout.icons.get(index)?;
        let remaining: Vec<Rect> = layout
            .icons
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, rect)| *rect)
            .collect();
        let insertion = dock_core::insertion_x(&remaining, 1, geometry.gap);
        Some(DragVisual {
            index,
            cursor: Point::new(icon.center().x + 24.0, layout.dock.top() - 26.0),
            grab_dx: 18.0,
            insertion_x: insertion,
            active: true,
        })
    });

    let scene = DockScene {
        config,
        metrics: &metrics,
        geometry: &geometry,
        layout: &layout,
        animations: &[],
        state: &state,
        palette: palette_for(config, theme),
        icons,
        native_glass: false,
        tooltip_progress: if tooltip { 1.0 } else { 0.0 },
        hover_progress: if hover_index.is_some() { 1.0 } else { 0.0 },
        error: None,
        error_progress: 0.0,
        drag,
        glass: false,
    };
    renderer.render_dock(&scene)
}

fn add_menu_overlay(
    renderer: &mut Renderer,
    dock_pixmap: &mut Pixmap,
    config: &DockConfig,
    theme: ResolvedTheme,
    index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(item) = config.items.get(index) else {
        return Ok(());
    };
    let entries = vec![
        MenuEntry::action("打开", MenuAction::Activate(item.id), MenuIcon::Open),
        MenuEntry::action(
            "打开文件位置",
            MenuAction::OpenLocation(item.id),
            MenuIcon::Folder,
        ),
        MenuEntry::Separator,
        MenuEntry::danger("从 Dock 移除", MenuAction::Remove(item.id), MenuIcon::Trash),
    ];
    let palette = palette_for(config, theme);
    let layout = renderer.menu_layout(&entries, 1.0);
    let Some(frame) = renderer.render_menu(&MenuScene {
        entries: &entries,
        layout: &layout,
        hovered: Some(1),
        palette,
    }) else {
        return Ok(());
    };

    let metrics = Metrics::new(&config.appearance, &config.behavior, &config.motion, 1.0);
    let geometry = DockGeometry::new(config.items.len(), &metrics);
    let scales = target_scales(
        config.items.len(),
        Some(Point::new(
            geometry.slot_centers()[index],
            geometry.content_base.center().y,
        )),
        &geometry,
        &metrics,
        &config.behavior,
    );
    let frame_layout = FrameLayout::compute(&geometry, &metrics, &scales);
    let Some(icon) = frame_layout.icons.get(index) else {
        return Ok(());
    };
    let anchor_x = icon.center().x;
    let anchor_bottom = frame_layout.dock.top() - 8.0;
    let max_x = (dock_pixmap.width() as f32 - layout.window.width - 2.0).max(2.0);
    let max_y = (dock_pixmap.height() as f32 - layout.window.height - 2.0).max(2.0);
    let x = (anchor_x - layout.bubble.center().x).clamp(2.0, max_x);
    let y = (anchor_bottom - layout.bubble.bottom()).clamp(2.0, max_y);

    let paint = PixmapPaint {
        opacity: 1.0,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: FilterQuality::Nearest,
    };
    dock_pixmap.draw_pixmap(
        x.round() as i32,
        y.round() as i32,
        frame.pixmap.as_ref(),
        &paint,
        Transform::identity(),
        None,
    );
    Ok(())
}

fn compose(dock: &Pixmap, theme: ResolvedTheme) -> Result<Pixmap, Box<dyn std::error::Error>> {
    preview::desktop_with_dock(dock, theme, 1440, 900, 52)
        .ok_or_else(|| "failed to compose wallpaper".into())
}

fn desktop_dock_origin(dock: &Pixmap) -> (i32, i32) {
    (
        ((1440i32 - dock.width() as i32) / 2).max(0),
        (900i32 - dock.height() as i32 - 52).max(0),
    )
}

fn crop(source: &Pixmap, x: i32, y: i32, width: u32, height: u32) -> Option<Pixmap> {
    let width = width
        .min(source.width().saturating_sub(x.max(0) as u32))
        .max(1);
    let height = height
        .min(source.height().saturating_sub(y.max(0) as u32))
        .max(1);
    let mut cropped = Pixmap::new(width, height)?;
    let paint = PixmapPaint {
        opacity: 1.0,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: FilterQuality::Nearest,
    };
    cropped.draw_pixmap(
        -x.max(0),
        -y.max(0),
        source.as_ref(),
        &paint,
        Transform::identity(),
        None,
    );
    Some(cropped)
}
