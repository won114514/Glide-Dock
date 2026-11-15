//! Dock application state machine: input → commands → state → pixels → config.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Instant;

use dock_core::events::DockCommand;
use dock_core::geometry::{Point, Rect};
use dock_core::layout::{DockGeometry, FrameLayout, Metrics, insertion_index, insertion_x};
use dock_core::models::{DockConfig, DockItem, DockLayer, ItemKind, LaunchType};
use dock_core::state::{DragSession, MenuAction, MenuEntry, MenuIcon, MenuTarget};
use dock_core::theme::{Palette, ResolvedTheme, ThemeMode};
use dock_core::{DockState, MotionEngine, MotionSnapshot, MotionTuning};
use dock_platform_windows::{
    DropPayload, GlassBackdrop, IconImage, IconService, LayeredSurface, PlatformError,
    ResolveService, ResolvedEntry, StoreAppService, TrayEvent, TrayIcon, WM_APP_LAUNCH_RESULT,
    WM_APP_STORE_APPS, WM_APP_WINDOW_EVENT, WM_ICON_READY, WM_PATHS_RESOLVED, WindowTracker,
    accept_files, animations_enabled, capture_mouse, create_layered_popup, cursor_position,
    is_low_integrity, metrics_for_window, open_path, open_start_menu, parse_drop, pick_store_app,
    place_above_desktop, place_dock, prefers_light_theme, register_class, release_mouse, reveal,
    screen_to_client, set_bounds, set_topmost, show_no_activate, visible_window_paths,
    windows_build,
};
use dock_render::icons::IconMap;
use dock_render::{
    DockScene, DragVisual, IconAnimation, MenuLayout, MenuScene, Renderer, StartMenuStyle,
};
use dock_storage::{ConfigStore, LoadOutcome, StorageError};
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_ESCAPE, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Shell::HDROP;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetForegroundWindow, GetSystemMetrics, HTCLIENT,
    HTTRANSPARENT, HWND_NOTOPMOST, HWND_TOPMOST, KillTimer, MA_NOACTIVATE, PostQuitMessage,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WM_CAPTURECHANGED, WM_CLOSE, WM_COPYDATA, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_DROPFILES, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER,
};

use crate::launcher::LauncherService;
use crate::render_loop::{FrameClock, frame_interval_ms};
use crate::wndproc::dock_wndproc;
mod settings;
use settings::{SettingsHost, SettingsLaunchError};

/// Mouse leave notification (defined in Win32::UI::Controls).
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_FRAME: usize = 1;
const TIMER_TOOLTIP: usize = 2;
const TIMER_SAVE: usize = 3;
const TIMER_ERROR: usize = 4;
const TIMER_MENU: usize = 5;
const TIMER_RUNNING: usize = 6;

const ICON_MASTER_SIZE: u32 = 256;
const MENU_POLL_MS: u32 = 60;
/// Coalesces the burst of WinEvent notifications produced while an
/// application opens (create/show/child events arrive in quick succession).
const RUNNING_DEBOUNCE_MS: u32 = 80;
/// Backstop for a menu left open while the user does something else. The
/// foreground comparison normally closes it earlier.
const MENU_TIMEOUT_SECS: f32 = 30.0;
const TOOLTIP_DELAY_MS: u32 = 420;
const SAVE_DEBOUNCE_MS: u32 = 300;
const ERROR_LIFETIME_MS: u32 = 2_600;
const DRAG_THRESHOLD_DIP: f32 = 8.0;
const DRAG_REMOVE_GAP_DIP: f32 = 24.0;

/// Errors raised while assembling the app.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Platform initialization failed.
    #[error("{0}")]
    Platform(#[from] PlatformError),
    /// Configuration load failed.
    #[error("{0}")]
    Storage(#[from] StorageError),
}

struct MenuView {
    entries: Vec<MenuEntry>,
    layout: MenuLayout,
    position: Point,
    pixmap: Pixmap,
    hovered: Option<usize>,
    closing: bool,
}

impl MenuView {
    fn bubble(&self) -> Rect {
        self.layout
            .bubble
            .translate(self.position.x, self.position.y)
    }

    fn hit(&self, point: Point) -> Option<usize> {
        if self.closing {
            return None;
        }
        self.layout.hit(point - self.position)
    }

    fn contains(&self, point: Point) -> bool {
        !self.closing && self.bubble().contains(point)
    }
}

const TRAY_MENU_CLASS: &str = "Glide-Dock.TrayMenu";
const TIMER_TRAY_MENU: usize = 7;
const TRAY_MENU_POLL_MS: u32 = 40;

struct TraySubmenuView {
    parent: usize,
    entries: Vec<MenuEntry>,
    layout: MenuLayout,
    position: Point,
}

struct TrayMenuView {
    hwnd: HWND,
    surface: LayeredSurface,
    renderer: Renderer,
    origin: Point,
    width: u32,
    height: u32,
    canvas: Pixmap,
    entries: Vec<MenuEntry>,
    layout: MenuLayout,
    position: Point,
    child: Option<TraySubmenuView>,
    hovered: Option<usize>,
    child_hovered: Option<usize>,
    visible: bool,
}

impl TrayMenuView {
    fn create(app: *mut DockApp, dpi_scale: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let scale = dpi_scale.max(0.5);
        let instance = register_class(TRAY_MENU_CLASS, Some(dock_wndproc))?;
        let width = ((unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1) as f32) * scale)
            .round() as u32;
        let height = ((unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1) as f32) * scale)
            .round() as u32;
        let origin = Point::new(
            (unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f32) * scale,
            (unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f32) * scale,
        );
        let hwnd =
            create_layered_popup(instance, TRAY_MENU_CLASS, "Glide-Dock 菜单", width, height)?;
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        }
        let surface = LayeredSurface::new(hwnd)?;
        let canvas = Pixmap::new(width, height).ok_or_else(|| {
            PlatformError::Window("tray menu canvas allocation failed".to_string())
        })?;
        let renderer = Renderer::new();
        let layout = renderer.menu_layout(&[], 1.0);
        Ok(Self {
            hwnd,
            surface,
            renderer,
            origin,
            width,
            height,
            canvas,
            entries: Vec::new(),
            layout,
            position: Point::new(0.0, 0.0),
            child: None,
            hovered: None,
            child_hovered: None,
            visible: false,
        })
    }

    fn root_hit(&self, point: Point) -> Option<usize> {
        if !self.visible {
            return None;
        }
        self.layout.hit(point - self.position)
    }

    fn child_hit(&self, point: Point) -> Option<usize> {
        self.child
            .as_ref()
            .and_then(|child| child.layout.hit(point - child.position))
    }

    fn contains(&self, point: Point) -> bool {
        self.visible
            && (self.layout.inside_bubble(point - self.position)
                || self
                    .child
                    .as_ref()
                    .is_some_and(|child| child.layout.inside_bubble(point - child.position)))
    }

    fn child_contains(&self, point: Point) -> bool {
        self.child
            .as_ref()
            .is_some_and(|child| child.layout.inside_bubble(point - child.position))
    }
}

impl Drop for TrayMenuView {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TRAY_MENU);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The complete dock application.
pub struct DockApp {
    hwnd: HWND,
    surface: LayeredSurface,
    renderer: Renderer,
    store: ConfigStore,
    config: DockConfig,
    state: DockState,
    metrics: Metrics,
    geometry: DockGeometry,
    layout: FrameLayout,
    motion: MotionEngine,
    animation_epoch: Instant,
    system_animations: bool,
    tooltip_spring: dock_core::ScalarSpring,
    error_spring: dock_core::ScalarSpring,
    menu_spring: dock_core::ScalarSpring,
    palette: Palette,
    theme: ResolvedTheme,
    icons: IconMap,
    icon_service: IconService,
    icon_generation: u64,
    resolve_service: ResolveService,
    dpi_scale: f32,
    refresh_hz: u32,
    frame_interval_ms: u32,
    clock: FrameClock,
    frame_timer: bool,
    tooltip_timer: bool,
    save_timer: bool,
    error_timer: bool,
    running_timer: bool,
    error_message: Option<String>,
    menu: Option<MenuView>,
    menu_pressed: Option<usize>,
    menu_right_down: bool,
    menu_opened_at: Option<f32>,
    menu_foreground: Option<HWND>,
    window_visible: bool,
    layer_topmost: bool,
    window_origin: (i32, i32),
    mouse_tracked: bool,
    glass: Option<GlassBackdrop>,
    native_glass: bool,
    glass_failed: bool,
    settings: SettingsHost,
    tray: Option<TrayIcon>,
    tray_menu: Option<TrayMenuView>,
    _window_tracker: Option<WindowTracker>,
    launcher: LauncherService,
    store_apps: StoreAppService,
    store_scan_pending: bool,
    read_only: bool,
    read_only_warned: bool,
    persist: bool,
    shutdown_done: bool,
}

impl DockApp {
    /// Builds the application, creates the surface and renders the first frame.
    pub fn new(
        hwnd: HWND,
        store: ConfigStore,
        outcome: LoadOutcome,
        show: bool,
        persist: bool,
    ) -> Result<Self, AppError> {
        for warning in &outcome.warnings {
            tracing::warn!("{warning}");
        }
        if let Some(backup) = &outcome.backup {
            tracing::warn!(path = %backup.display(), "corrupt configuration backed up");
        }

        let config = outcome.config;
        let monitor = metrics_for_window(hwnd);
        let dpi_scale = monitor.dpi_scale;
        let refresh_hz = monitor.refresh_hz;
        let frame_interval_ms = frame_interval_ms(refresh_hz);
        let theme = resolve_theme(config.appearance.theme);
        let palette = palette_for(&config, theme);
        let metrics = Metrics::new(
            &config.appearance,
            &config.behavior,
            &config.motion,
            dpi_scale,
        );
        let geometry = DockGeometry::with_items(&config.items, &metrics);
        let scales = vec![1.0; config.items.len()];
        let layout = FrameLayout::compute(&geometry, &metrics, &scales);
        let system_animations = animations_enabled();
        let motion = MotionEngine::new(MotionTuning::new(
            &config.behavior,
            &config.motion,
            system_animations,
        ));
        let launcher = LauncherService::new(hwnd);
        let store_apps = StoreAppService::spawn(hwnd);
        let surface = LayeredSurface::new(hwnd)?;
        let icon_service = IconService::spawn(hwnd);
        let resolve_service = ResolveService::spawn(hwnd);
        let window_tracker = WindowTracker::install(hwnd);
        let mut renderer = Renderer::new();
        let build = windows_build();
        renderer.set_start_style(if build == 0 || build >= 22000 {
            StartMenuStyle::Windows11
        } else {
            StartMenuStyle::Windows10
        });

        let mut app = Self {
            hwnd,
            surface,
            renderer,
            store,
            config,
            state: DockState::default(),
            metrics,
            geometry,
            layout,
            motion,
            animation_epoch: Instant::now(),
            system_animations,
            tooltip_spring: dock_core::ScalarSpring::new(0.0, 520.0, 0.86),
            error_spring: dock_core::ScalarSpring::new(0.0, 430.0, 0.86),
            menu_spring: dock_core::ScalarSpring::new(0.0, 540.0, 0.84),
            palette,
            theme,
            icons: IconMap::new(),
            icon_service,
            icon_generation: 0,
            resolve_service,
            dpi_scale,
            refresh_hz,
            frame_interval_ms,
            clock: FrameClock::new(),
            frame_timer: false,
            tooltip_timer: false,
            save_timer: false,
            error_timer: false,
            running_timer: false,
            error_message: None,
            menu: None,
            menu_pressed: None,
            menu_right_down: false,
            menu_opened_at: None,
            menu_foreground: None,
            window_visible: false,
            layer_topmost: false,
            window_origin: (0, 0),
            mouse_tracked: false,
            glass: None,
            native_glass: false,
            glass_failed: false,
            settings: SettingsHost::default(),
            tray: None,
            tray_menu: None,
            _window_tracker: window_tracker,
            launcher,
            store_apps,
            store_scan_pending: false,
            read_only: outcome.read_only,
            read_only_warned: false,
            persist,
            shutdown_done: false,
        };

        app.state.unresolved = compute_unresolved(&app.config);
        app.relayout_window(false);
        app.request_all_icons();
        app.refresh_running();
        if is_low_integrity() {
            tracing::warn!(
                "Glide-Dock is running at Low integrity; child applications may fail to start"
            );
            if show {
                app.set_error(
                    "低完整性运行：应用可能无法启动，请将 Glide-Dock 移到桌面等普通目录"
                        .to_string(),
                );
            }
        }
        app.render();
        if show {
            show_no_activate(hwnd);
            app.window_visible = true;
            app.ensure_native_glass();
            app.apply_dock_layer();
            // Present once more after the window is shown; some compositions
            // ignore the pre-show layered content until the next update.
            app.render();
        }
        accept_files(hwnd);
        if show {
            match TrayIcon::install(hwnd) {
                Ok(tray) => {
                    app.tray = Some(tray);
                }
                Err(error) => tracing::warn!(%error, "could not install notification-area icon"),
            }
        }

        if outcome.migrated && !outcome.read_only {
            app.save_now();
        }
        Ok(app)
    }

    // ----------------------------------------------------------------- input

    /// Handles one window message. Unknown messages go to `DefWindowProc` by
    /// the window procedure wrapper.
    pub fn handle_message(
        &mut self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if self
            .tray_menu
            .as_ref()
            .is_some_and(|menu| menu.hwnd == hwnd)
        {
            return self.handle_tray_menu_message(hwnd, msg, wparam, lparam);
        }
        if let Some(event) = self
            .tray
            .as_ref()
            .and_then(|tray| tray.handle_message(msg, lparam))
        {
            self.on_tray_event(event);
            return LRESULT(0);
        }
        match msg {
            WM_MOUSEMOVE => {
                self.on_mouse_move(lparam);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                self.on_mouse_leave();
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let point = mouse_point(lparam);
                tracing::debug!(x = point.x, y = point.y, "left button down");
                self.on_left_down(point);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let point = mouse_point(lparam);
                tracing::debug!(x = point.x, y = point.y, "left button up");
                self.on_left_up(point);
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                let point = mouse_point(lparam);
                tracing::debug!(x = point.x, y = point.y, "right button down");
                self.on_right_down(point);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let point = mouse_point(lparam);
                tracing::debug!(x = point.x, y = point.y, "right button up");
                self.on_right_up(point);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                self.cancel_pressed_interactions();
                self.state.drag = None;
                self.state.pressed = None;
                self.render();
                LRESULT(0)
            }
            WM_DROPFILES => {
                let payload = parse_drop(HDROP(wparam.0 as *mut c_void));
                self.on_drop(payload);
                LRESULT(0)
            }
            WM_ICON_READY => {
                self.on_icons_ready();
                LRESULT(0)
            }
            WM_PATHS_RESOLVED => {
                self.on_paths_resolved();
                LRESULT(0)
            }
            WM_APP_STORE_APPS => {
                self.on_store_apps();
                LRESULT(0)
            }
            WM_TIMER => {
                self.on_timer(wparam.0);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(self.hit_test(lparam) as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_DPICHANGED => {
                self.relayout_window(self.window_visible);
                self.render();
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                self.relayout_window(self.window_visible);
                self.render();
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                self.system_animations = animations_enabled();
                self.apply_motion_config();
                self.on_theme_changed();
                self.animate();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                unsafe {
                    let _ = BeginPaint(hwnd, &mut paint);
                    let _ = EndPaint(hwnd, &paint);
                }
                LRESULT(0)
            }
            WM_COPYDATA => {
                let handled = self.on_settings_update(wparam, lparam);
                LRESULT(if handled { 1 } else { 0 })
            }
            WM_APP_WINDOW_EVENT => {
                self.on_window_event();
                LRESULT(0)
            }
            WM_APP_LAUNCH_RESULT => {
                self.on_launch_results();
                LRESULT(0)
            }
            WM_CLOSE => {
                self.shutdown();
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                self.kill_all_timers();
                unsafe {
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => unsafe {
                windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam)
            },
        }
    }

    fn now(&self) -> f32 {
        self.animation_epoch.elapsed().as_secs_f32()
    }

    fn apply_dock_layer(&mut self) {
        if !self.window_visible {
            return;
        }
        let topmost = match self.config.appearance.layer {
            DockLayer::Desktop => false,
            DockLayer::Dynamic => {
                self.state.cursor.is_some() || self.menu.is_some() || self.state.drag.is_some()
            }
            DockLayer::Topmost => true,
        };
        if topmost == self.layer_topmost {
            return;
        }
        if topmost {
            set_topmost(self.hwnd);
        } else {
            place_above_desktop(self.hwnd);
        }
        self.layer_topmost = topmost;
        if let Some(glass) = &self.glass {
            if topmost {
                set_topmost(glass.hwnd());
            } else {
                place_above_desktop(glass.hwnd());
            }
            glass.place_below(self.hwnd);
        }
    }

    fn motion_tuning(&self) -> MotionTuning {
        MotionTuning::new(
            &self.config.behavior,
            &self.config.motion,
            self.system_animations,
        )
    }

    fn apply_motion_config(&mut self) {
        let tuning = self.motion_tuning();
        self.motion.apply_config(tuning, self.now());
    }

    fn cancel_pressed_interactions(&mut self) {
        self.motion.cancel_all_presses(self.now());
        self.animate();
    }

    fn cancel_interaction(&mut self, id: u32) {
        self.motion.cancel_press(self.now(), id);
        self.animate();
    }

    fn hit_icon(&self, point: Point) -> Option<usize> {
        for (index, item) in self.config.items.iter().enumerate() {
            if item.kind == ItemKind::Separator {
                if let Some(rect) = self.geometry.slot_rects().get(index) {
                    if rect.contains(point) {
                        return Some(index);
                    }
                }
            }
        }
        self.layout.hit_icon(point)
    }

    fn on_mouse_move(&mut self, lparam: LPARAM) {
        let point = mouse_point(lparam);

        if self.menu.as_ref().is_some_and(|menu| !menu.closing) {
            self.update_menu_hover(point);
            return;
        }

        let entering = self.state.cursor.is_none();
        self.state.cursor = Some(point);
        if entering {
            self.begin_hover();
        }

        if let Some(mut drag) = self.state.drag.take() {
            drag.cursor = point;
            if !drag.active {
                let threshold = DRAG_THRESHOLD_DIP * self.dpi_scale;
                if drag.origin.distance(point) > threshold {
                    drag.active = true;
                    self.state.hovered = None;
                    self.motion.set_pointer(self.now(), Some(point), None);
                    self.cancel_interaction(drag.item_id);
                    self.hide_tooltip(true);
                }
            }
            if drag.active {
                let remaining: Vec<Rect> = self
                    .layout
                    .icons
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != drag.from)
                    .map(|(_, rect)| *rect)
                    .collect();
                drag.insertion = insertion_index(&remaining, point.x);
                self.state.drag = Some(drag);
                self.render();
                return;
            }
            self.state.drag = Some(drag);
        }

        let hovered = self.hit_icon(point);
        if hovered != self.state.hovered {
            self.state.hovered = hovered;
            self.reset_tooltip();
            self.animate();
        }
        self.motion.set_pointer(self.now(), Some(point), hovered);
        self.apply_dock_layer();
        self.animate();
    }

    fn begin_hover(&mut self) {
        self.track_mouse_leave();
        self.refresh_running();
        self.animate();
    }

    fn track_mouse_leave(&mut self) {
        if self.mouse_tracked {
            return;
        }
        let mut event = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        let result = unsafe { TrackMouseEvent(&mut event) };
        if result.is_ok() {
            self.mouse_tracked = true;
        }
    }

    fn on_mouse_leave(&mut self) {
        if self.state.drag.is_some() {
            return;
        }
        self.mouse_tracked = false;
        self.menu_pressed = None;
        self.cancel_pressed_interactions();
        self.state.clear_pointer();
        self.motion.set_pointer(self.now(), None, None);
        self.tooltip_spring.set_target(0.0);
        self.kill_timer(TIMER_TOOLTIP);
        self.tooltip_timer = false;
        self.apply_dock_layer();
        self.animate();
    }

    fn on_left_down(&mut self, point: Point) {
        if self.menu.is_some() {
            if self.menu.as_ref().is_some_and(|menu| menu.closing) {
                return;
            }
            if let Some(entry) = self.menu_hit(point) {
                self.menu_pressed = Some(entry);
            } else if self.menu.as_ref().is_some_and(|menu| menu.contains(point)) {
                // Clicking padding keeps the menu open, like a native menu.
            } else {
                self.close_menu();
            }
            return;
        }

        let Some(index) = self.hit_icon(point) else {
            self.state.pressed = None;
            self.cancel_pressed_interactions();
            return;
        };
        let Some(item) = self.config.items.get(index) else {
            return;
        };
        if item.kind == ItemKind::Separator {
            return;
        }
        let Some(icon) = self.layout.icons.get(index).copied() else {
            return;
        };
        let item_id = item.id;
        let now = self.now();
        self.state.pressed = Some(item_id);
        self.motion.pointer_down(now, item_id);
        self.animate();
        self.state.drag = Some(DragSession {
            item_id,
            from: index,
            grab_dx: point.x - icon.left(),
            origin: point,
            cursor: point,
            insertion: index,
            active: false,
        });
        capture_mouse(self.hwnd);
        self.hide_tooltip(true);
    }

    fn on_left_up(&mut self, point: Point) {
        if let Some(entry) = self.menu_pressed.take() {
            if self.menu_hit(point) == Some(entry) {
                self.activate_menu_entry(entry);
            }
            return;
        }
        let Some(drag) = self.state.drag.take() else {
            release_mouse();
            return;
        };
        self.state.pressed = None;
        release_mouse();

        if drag.active {
            let remove_threshold = self.layout.dock.top() - DRAG_REMOVE_GAP_DIP * self.dpi_scale;
            if point.y < remove_threshold {
                self.dispatch(DockCommand::RemoveItem { id: drag.item_id });
                self.state.hovered = None;
                self.animate();
                return;
            }
            let remaining: Vec<Rect> = self
                .layout
                .icons
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != drag.from)
                .map(|(_, rect)| *rect)
                .collect();
            let to = insertion_index(&remaining, point.x);
            if to != drag.from {
                self.dispatch(DockCommand::MoveItem {
                    from: drag.from,
                    to,
                });
            }
            self.state.hovered = None;
            self.state.tooltip.index = None;
            self.animate();
            self.render();
        } else if self.layout.hit_icon(point) == Some(drag.from) {
            self.motion.pointer_up(self.now(), drag.item_id);
            self.dispatch(DockCommand::Activate { id: drag.item_id });
        } else {
            self.cancel_interaction(drag.item_id);
        }
    }

    fn right_target(&self, point: Point) -> Option<MenuTarget> {
        if let Some(index) = self.hit_icon(point) {
            return self
                .config
                .items
                .get(index)
                .map(|item| MenuTarget::Item(item.id));
        }
        let margin = 6.0 * self.dpi_scale;
        if self.layout.dock.inset(-margin).contains(point) {
            Some(MenuTarget::Background)
        } else {
            None
        }
    }

    fn on_right_down(&mut self, point: Point) {
        tracing::debug!(x = point.x, y = point.y, target = ?self.right_target(point), "right down");
        if self.menu.is_some() {
            self.close_menu();
            self.menu_right_down = true;
            return;
        }
        if let Some(target) = self.right_target(point) {
            self.menu_right_down = true;
            self.dispatch(DockCommand::OpenMenu { target, at: point });
        }
    }

    fn on_right_up(&mut self, point: Point) {
        if self.menu_right_down {
            self.menu_right_down = false;
            return;
        }
        if self.menu.is_some() {
            self.close_menu();
            return;
        }
        if let Some(target) = self.right_target(point) {
            self.dispatch(DockCommand::OpenMenu { target, at: point });
        }
    }

    // ------------------------------------------------------------------ menu

    fn build_menu_entries(&self, target: MenuTarget) -> Vec<MenuEntry> {
        match target {
            MenuTarget::Item(id) => {
                let item = self.config.items.iter().find(|item| item.id == id);
                match item.map(|item| item.kind) {
                    Some(ItemKind::Separator) => {
                        vec![MenuEntry::danger(
                            "从 Dock 移除",
                            MenuAction::Remove(id),
                            MenuIcon::Trash,
                        )]
                    }
                    Some(ItemKind::Widget) => vec![
                        MenuEntry::action("打开", MenuAction::Activate(id), MenuIcon::Open),
                        MenuEntry::Separator,
                        MenuEntry::danger("从 Dock 移除", MenuAction::Remove(id), MenuIcon::Trash),
                    ],
                    _ => vec![
                        MenuEntry::action("打开", MenuAction::Activate(id), MenuIcon::Open),
                        MenuEntry::action(
                            "打开文件位置",
                            MenuAction::OpenLocation(id),
                            MenuIcon::Folder,
                        ),
                        MenuEntry::Separator,
                        MenuEntry::danger("从 Dock 移除", MenuAction::Remove(id), MenuIcon::Trash),
                    ],
                }
            }
            MenuTarget::Background => vec![
                MenuEntry::action("设置...", MenuAction::OpenSettings, MenuIcon::Settings),
                MenuEntry::action("添加开始菜单", MenuAction::AddStartWidget, MenuIcon::Open),
                MenuEntry::action("添加分隔线", MenuAction::AddSeparator, MenuIcon::Edit),
                MenuEntry::action("添加商店应用...", MenuAction::AddStoreApp, MenuIcon::Open),
                MenuEntry::Separator,
                MenuEntry::danger("退出 Glide-Dock", MenuAction::Quit, MenuIcon::Power),
            ],
        }
    }

    fn open_menu(&mut self, target: MenuTarget, point: Point) {
        if self.menu.is_some() {
            self.close_menu_immediately();
        }
        let entries = self.build_menu_entries(target);
        let layout = self.renderer.menu_layout(&entries, self.dpi_scale);
        let Some(frame) = self.renderer.render_menu(&MenuScene {
            entries: &entries,
            layout: &layout,
            hovered: None,
            palette: self.palette,
        }) else {
            return;
        };

        let anchor_x = match target {
            MenuTarget::Item(id) => self
                .config
                .items
                .iter()
                .position(|item| item.id == id)
                .and_then(|index| self.layout.icons.get(index))
                .map_or(point.x, |icon| icon.center().x),
            MenuTarget::Background => point.x,
        };
        let anchor_bottom = match target {
            MenuTarget::Item(_) => self.layout.dock.top() - 8.0 * self.dpi_scale,
            MenuTarget::Background => point.y - 4.0 * self.dpi_scale,
        };

        let window = self.geometry.window;
        let max_x = (window.width - layout.window.width - 2.0).max(2.0);
        let max_y = (window.height - layout.window.height - 2.0).max(2.0);
        let position = Point::new(
            (anchor_x - layout.bubble.center().x).clamp(2.0, max_x),
            (anchor_bottom - layout.bubble.bottom()).clamp(2.0, max_y),
        );

        self.menu = Some(MenuView {
            entries,
            layout: frame.layout,
            position,
            pixmap: frame.pixmap,
            hovered: None,
            closing: false,
        });
        self.menu_spring.set_target(1.0);
        self.state.menu_open = true;
        self.menu_pressed = None;
        self.menu_opened_at = Some(self.now());
        let foreground = unsafe { GetForegroundWindow() };
        self.menu_foreground = (!foreground.0.is_null()).then_some(foreground);
        self.state.tooltip.index = None;
        self.tooltip_spring.set_target(0.0);
        self.kill_timer(TIMER_TOOLTIP);
        self.tooltip_timer = false;
        match target {
            MenuTarget::Item(id) => {
                if let Some(index) = self.config.items.iter().position(|item| item.id == id) {
                    self.state.hovered = Some(index);
                }
            }
            MenuTarget::Background => {
                self.state.hovered = None;
            }
        }
        self.kill_timer(TIMER_MENU);
        unsafe {
            let _ = SetTimer(Some(self.hwnd), TIMER_MENU, MENU_POLL_MS, None);
        }
        self.apply_dock_layer();
        self.animate();
        self.render();
    }

    fn update_menu_hover(&mut self, point: Point) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        if menu.closing {
            return;
        }
        let hovered = menu.layout.hit(point - menu.position);
        if hovered == menu.hovered {
            return;
        }
        let entries = menu.entries.clone();
        let layout = menu.layout.clone();
        let Some(frame) = self.renderer.render_menu(&MenuScene {
            entries: &entries,
            layout: &layout,
            hovered,
            palette: self.palette,
        }) else {
            return;
        };
        if let Some(menu) = self.menu.as_mut() {
            menu.pixmap = frame.pixmap;
            menu.hovered = hovered;
        }
        self.render();
    }

    fn menu_hit(&self, point: Point) -> Option<usize> {
        self.menu.as_ref().and_then(|menu| menu.hit(point))
    }

    fn activate_menu_entry(&mut self, index: usize) {
        let action = self.menu.as_ref().and_then(|menu| {
            menu.entries.get(index).and_then(|entry| match entry {
                MenuEntry::Action { action, .. } => Some(*action),
                MenuEntry::Separator => None,
            })
        });
        let Some(action) = action else {
            return;
        };
        if matches!(action, MenuAction::Quit) {
            self.quit();
            return;
        }
        self.close_menu();
        self.perform_action(action);
    }

    fn perform_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Activate(id) => self.dispatch(DockCommand::Activate { id }),
            MenuAction::OpenLocation(id) => self.dispatch(DockCommand::OpenItemLocation { id }),
            MenuAction::Remove(id) => self.dispatch(DockCommand::RemoveItem { id }),
            MenuAction::OpenSettings => self.open_settings(),
            MenuAction::AddStartWidget => self.add_start_widget(),
            MenuAction::AddSeparator => self.add_separator(),
            MenuAction::AddStoreApp => self.begin_store_scan(),
            MenuAction::OpenConfigDir => self.open_config_dir(),
            MenuAction::ReloadConfig => self.dispatch(DockCommand::ReloadConfig),
            MenuAction::ThemeSystem => self.set_theme(ThemeMode::System),
            MenuAction::ThemeLight => self.set_theme(ThemeMode::Light),
            MenuAction::ThemeDark => self.set_theme(ThemeMode::Dark),
            MenuAction::LayerDesktop => self.set_layer(DockLayer::Desktop),
            MenuAction::LayerDynamic => self.set_layer(DockLayer::Dynamic),
            MenuAction::LayerTopmost => self.set_layer(DockLayer::Topmost),
            MenuAction::Quit => self.quit(),
            MenuAction::Noop => {}
        }
    }

    fn close_menu(&mut self) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if menu.closing {
            return;
        }
        menu.closing = true;
        self.state.menu_open = false;
        self.menu_pressed = None;
        self.menu_opened_at = None;
        self.menu_foreground = None;
        self.menu_right_down = false;
        self.menu_spring.set_target(0.0);
        self.kill_timer(TIMER_MENU);
        self.animate();
    }

    fn close_menu_immediately(&mut self) {
        self.menu = None;
        self.menu_pressed = None;
        self.menu_right_down = false;
        self.menu_opened_at = None;
        self.menu_foreground = None;
        self.state.menu_open = false;
        self.menu_spring.spring.value = 0.0;
        self.menu_spring.spring.target = 0.0;
        self.kill_timer(TIMER_MENU);
        self.apply_dock_layer();
    }

    fn on_menu_timer(&mut self) {
        let Some(menu) = self.menu.as_ref() else {
            self.kill_timer(TIMER_MENU);
            return;
        };
        if menu.closing {
            self.kill_timer(TIMER_MENU);
            return;
        }
        let mut menu_age = 0.0;
        if let Some(opened) = self.menu_opened_at {
            menu_age = self.now() - opened;
            if menu_age >= MENU_TIMEOUT_SECS {
                self.close_menu();
                return;
            }
        }
        // Give the foreground a moment to settle after the opening click; the
        // dock itself never activates, so a change after that means the user
        // switched to another window.
        let foreground = unsafe { GetForegroundWindow() };
        if menu_age >= 0.25 {
            if let Some(expected) = self.menu_foreground {
                if !foreground.0.is_null() && foreground != expected {
                    self.close_menu();
                    return;
                }
            }
        }
        let escape = key_down(VK_ESCAPE);
        if escape {
            self.close_menu();
            return;
        }
        let Some(cursor) = cursor_position() else {
            return;
        };
        let client = screen_to_client(self.hwnd, cursor);
        let point = Point::new(client.x as f32, client.y as f32);
        let over_menu = menu.contains(point);
        let over_dock = self.layout.hit_background(point) || self.hit_icon(point).is_some();
        if over_menu || over_dock {
            return;
        }
        if key_down(VK_LBUTTON) || key_down(VK_RBUTTON) {
            self.close_menu();
        }
    }

    // --------------------------------------------------------------- commands

    /// Single entry point for user intents.
    pub fn dispatch(&mut self, command: DockCommand) {
        match command {
            DockCommand::AddPaths { paths, at } => self.add_paths(paths, at),
            DockCommand::RemoveItem { id } => self.remove_item(id),
            DockCommand::MoveItem { from, to } => self.move_item(from, to),
            DockCommand::Activate { id } => {
                tracing::debug!(id, "activate command");
                self.activate(id);
            }
            DockCommand::OpenItemLocation { id } => self.open_item_location(id),
            DockCommand::SetHovered(index) => {
                self.state.hovered = index;
                self.motion
                    .set_pointer(self.now(), self.state.cursor, index);
                self.animate();
            }
            DockCommand::PointerDown(point) => self.on_left_down(point),
            DockCommand::PointerUp(point) => self.on_left_up(point),
            DockCommand::PointerMove(point) => {
                self.state.cursor = Some(point);
                self.motion
                    .set_pointer(self.now(), Some(point), self.state.hovered);
                self.animate();
            }
            DockCommand::OpenMenu { target, at } => self.open_menu(target, at),
            DockCommand::CloseMenu => self.close_menu(),
            DockCommand::ReloadConfig => self.reload_config(),
            DockCommand::RefreshItems => {
                self.request_all_icons();
                self.render();
            }
            DockCommand::Shutdown => self.quit(),
        }
    }

    fn add_paths(&mut self, paths: Vec<String>, at: Option<usize>) {
        if paths.is_empty() {
            return;
        }
        // `.lnk` resolution runs on a dedicated STA worker; the drop handler
        // must never block the message loop on COM or a slow network path.
        self.resolve_service.request(paths, at);
    }

    fn on_paths_resolved(&mut self) {
        let batches = self.resolve_service.drain();
        if batches.is_empty() {
            return;
        }
        for batch in batches {
            self.add_resolved(batch.at, batch.entries);
        }
    }

    fn begin_store_scan(&mut self) {
        if self.store_scan_pending {
            return;
        }
        self.store_scan_pending = true;
        self.store_apps.request();
        tracing::info!("store app scan requested");
    }

    fn on_store_apps(&mut self) {
        self.store_scan_pending = false;
        let Some(apps) = self.store_apps.drain().into_iter().last() else {
            return;
        };
        if apps.is_empty() {
            self.set_error("未找到可添加的商店应用".to_string());
            return;
        }
        let Some(at) = cursor_position() else {
            return;
        };
        let Some(index) = pick_store_app(self.hwnd, &apps, at) else {
            return;
        };
        let app = &apps[index];
        if self
            .config
            .items
            .iter()
            .any(|item| item.target_path.eq_ignore_ascii_case(&app.aumid))
        {
            self.set_error("该商店应用已在 Dock 中".to_string());
            return;
        }
        let entry = ResolvedEntry::packaged_app(app.display_name.clone(), app.aumid.clone());
        self.add_resolved(None, vec![entry]);
    }

    fn add_start_widget(&mut self) {
        if self
            .config
            .items
            .iter()
            .any(|item| item.kind == ItemKind::Widget && item.target_path == "start_menu")
        {
            self.set_error("开始菜单已经在 Dock 中".to_string());
            return;
        }
        let mut item = DockItem::widget(0, "开始菜单", "start_menu");
        item.id = self.config.allocate_id();
        self.insert_builtin(item);
    }

    fn add_separator(&mut self) {
        let mut item = DockItem::separator(0);
        item.id = self.config.allocate_id();
        self.insert_builtin(item);
    }

    fn insert_builtin(&mut self, item: DockItem) {
        let id = item.id;
        self.config.items.push(item);
        self.state.hovered = None;
        self.state.pressed = None;
        self.state.tooltip.index = None;
        self.state.unresolved.remove(&id);
        self.relayout_window(self.window_visible);
        self.mark_dirty();
        self.apply_dock_layer();
        self.animate();
        self.render();
        tracing::info!(%id, "built-in dock item added");
    }

    fn add_resolved(&mut self, at: Option<usize>, entries: Vec<ResolvedEntry>) {
        let start = at
            .unwrap_or(self.config.items.len())
            .min(self.config.items.len());
        let mut added: Vec<u32> = Vec::new();
        let mut insert_at = start;
        for resolved in entries {
            if resolved.target_path.trim().is_empty() {
                continue;
            }
            if self
                .config
                .items
                .iter()
                .any(|item| item.target_path.eq_ignore_ascii_case(&resolved.target_path))
            {
                continue;
            }
            let id = self.config.allocate_id();
            let item = resolved.into_item(id);
            if item.launch_type != LaunchType::PackagedApp && !Path::new(&item.target_path).exists()
            {
                self.state.unresolved.insert(id);
            }
            self.config
                .items
                .insert(insert_at.min(self.config.items.len()), item);
            added.push(id);
            insert_at += 1;
        }
        if added.is_empty() {
            return;
        }
        self.state.hovered = None;
        self.state.tooltip.index = None;
        self.relayout_window(self.window_visible);
        for id in &added {
            if let Some(item) = self.config.items.iter().find(|item| item.id == *id) {
                self.request_item_icons(item);
            }
        }
        self.mark_dirty();
        self.animate();
        self.render();
        tracing::info!(count = added.len(), "items added");
    }

    fn remove_item(&mut self, id: u32) {
        let Some(index) = self.config.items.iter().position(|item| item.id == id) else {
            return;
        };
        self.config.items.remove(index);
        self.icons.remove(id);
        self.state.running.remove(&id);
        self.state.unresolved.remove(&id);
        self.state.hovered = None;
        self.state.pressed = None;
        self.state.drag = None;
        self.state.tooltip.index = None;
        self.motion.set_pointer(self.now(), self.state.cursor, None);
        self.tooltip_spring.set_target(0.0);
        release_mouse();
        self.relayout_window(self.window_visible);
        self.mark_dirty();
        self.animate();
        self.render();
        tracing::info!(id, "item removed");
    }

    fn move_item(&mut self, from: usize, to: usize) {
        let length = self.config.items.len();
        if from >= length || length == 0 {
            return;
        }
        let hovered_id = self
            .state
            .hovered
            .and_then(|index| self.config.items.get(index).map(|item| item.id));
        let to = to.min(length - 1);
        if from == to {
            return;
        }
        let item = self.config.items.remove(from);
        self.config.items.insert(to, item);

        self.state.hovered =
            hovered_id.and_then(|id| self.config.items.iter().position(|item| item.id == id));
        self.relayout_window(self.window_visible);
        self.motion
            .set_pointer(self.now(), self.state.cursor, self.state.hovered);
        self.mark_dirty();
        self.animate();
        self.render();
    }

    fn activate(&mut self, id: u32) {
        let Some(item) = self.config.items.iter().find(|item| item.id == id).cloned() else {
            return;
        };
        if item.kind == ItemKind::Separator {
            return;
        }
        if item.kind == ItemKind::Widget {
            let result = match item.target_path.as_str() {
                "start_menu" => open_start_menu(),
                _ => Err("未知的小组件".to_string()),
            };
            if let Err(error) = result {
                self.set_error(format!("无法执行「{}」：{error}", item.label()));
            }
            return;
        }
        let confirmed = item.kind != ItemKind::Application
            || self.state.running.contains(&id)
            || WindowTracker::has_visible_window(item.target_path.trim());
        self.motion.begin_launch(self.now(), id, confirmed);
        self.animate();
        self.launcher.request(&item);
    }

    fn on_launch_results(&mut self) {
        let results = self.launcher.drain();
        for (id, result) in results {
            match result {
                Ok(()) => {
                    tracing::info!(id, "launcher accepted request");
                }
                Err(error) => {
                    self.motion.launch_failed(self.now(), id);
                    // A failed relaunch must not clear a dot for an app that is
                    // still running.
                    let still_running = self
                        .config
                        .items
                        .iter()
                        .find(|item| item.id == id)
                        .is_some_and(|item| WindowTracker::has_visible_window(&item.target_path));
                    if !still_running {
                        self.state.running.remove(&id);
                    }
                    let label = self
                        .config
                        .items
                        .iter()
                        .find(|item| item.id == id)
                        .map_or_else(|| "应用".to_string(), |item| item.label().to_string());
                    tracing::error!(id, %error, "launch failed");
                    self.set_error(format!("无法启动「{label}」：{error}"));
                    self.animate();
                }
            }
        }
    }

    fn on_window_event(&mut self) {
        if self.running_timer {
            return;
        }
        let timer = unsafe { SetTimer(Some(self.hwnd), TIMER_RUNNING, RUNNING_DEBOUNCE_MS, None) };
        if timer == 0 {
            // Timer creation failed; fall back to a synchronous refresh rather
            // than silently losing the event.
            self.refresh_running();
        } else {
            self.running_timer = true;
        }
    }

    fn open_item_location(&mut self, id: u32) {
        let Some(item) = self.config.items.iter().find(|item| item.id == id) else {
            return;
        };
        if let Err(error) = reveal(item) {
            self.set_error(format!("无法打开文件位置：{error}"));
        }
    }

    // ------------------------------------------------------------ persistence

    fn mark_dirty(&mut self) {
        if !self.persist {
            return;
        }
        if self.read_only {
            if !self.read_only_warned {
                self.read_only_warned = true;
                tracing::warn!("configuration is read-only; changes will not be saved");
            }
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_SAVE);
            let _ = SetTimer(Some(self.hwnd), TIMER_SAVE, SAVE_DEBOUNCE_MS, None);
        }
        self.save_timer = true;
    }

    fn save_now(&mut self) {
        self.kill_timer(TIMER_SAVE);
        self.save_timer = false;
        if !self.persist || self.read_only {
            return;
        }
        if let Err(error) = self.store.save(&self.config) {
            tracing::error!(%error, "failed to save configuration");
        }
    }

    fn reload_config(&mut self) {
        match self.store.load() {
            Ok(outcome) => {
                for warning in &outcome.warnings {
                    tracing::warn!("{warning}");
                }
                self.read_only = outcome.read_only;
                self.config = outcome.config;
                self.state = DockState::default();
                self.state.unresolved = compute_unresolved(&self.config);
                self.motion.reset_items();
                self.apply_motion_config();
                self.icon_generation = self.icon_generation.wrapping_add(1);
                self.icons = IconMap::new();
                self.close_menu_immediately();
                self.theme = resolve_theme(self.config.appearance.theme);
                self.palette = palette_for(&self.config, self.theme);
                self.refresh_glass_state();
                self.relayout_window(self.window_visible);
                self.request_all_icons();
                self.animate();
                self.render();
                tracing::info!("configuration reloaded");
            }
            Err(error) => self.set_error(format!("重新加载配置失败：{error}")),
        }
    }

    fn open_config_dir(&mut self) {
        let directory = self.store.directory();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.set_error(format!("无法创建配置目录：{error}"));
            return;
        }
        if let Err(error) = open_path(&directory.to_string_lossy()) {
            self.set_error(format!("无法打开配置目录：{error}"));
        }
    }

    /// Opens the settings center on the UI thread.
    pub fn show_settings(&mut self) {
        self.open_settings();
    }

    fn on_tray_event(&mut self, event: TrayEvent) {
        match event {
            TrayEvent::OpenSettings => self.show_settings(),
            TrayEvent::ShowMenu => self.open_tray_menu(),
            TrayEvent::ReAdd => {
                if let Some(tray) = self.tray.as_mut() {
                    if let Err(error) = tray.reinstall() {
                        tracing::warn!(%error, "could not restore notification-area icon");
                    }
                }
            }
        }
    }

    fn tray_root_entries(&self) -> Vec<MenuEntry> {
        vec![
            MenuEntry::submenu("外观", MenuIcon::Appearance),
            MenuEntry::submenu("Dock 层级", MenuIcon::Layers),
            MenuEntry::Separator,
            MenuEntry::action("打开设置", MenuAction::OpenSettings, MenuIcon::Settings),
            MenuEntry::Separator,
            MenuEntry::action("退出", MenuAction::Quit, MenuIcon::Power),
        ]
    }

    fn tray_child_entries(&self, parent: usize) -> Option<Vec<MenuEntry>> {
        match parent {
            0 => Some(vec![
                MenuEntry::option(
                    "跟随系统",
                    MenuAction::ThemeSystem,
                    self.config.appearance.theme == ThemeMode::System,
                ),
                MenuEntry::option(
                    "浅色",
                    MenuAction::ThemeLight,
                    self.config.appearance.theme == ThemeMode::Light,
                ),
                MenuEntry::option(
                    "深色",
                    MenuAction::ThemeDark,
                    self.config.appearance.theme == ThemeMode::Dark,
                ),
            ]),
            1 => Some(vec![
                MenuEntry::option(
                    "桌面同级",
                    MenuAction::LayerDesktop,
                    self.config.appearance.layer == DockLayer::Desktop,
                ),
                MenuEntry::option(
                    "动态层级",
                    MenuAction::LayerDynamic,
                    self.config.appearance.layer == DockLayer::Dynamic,
                ),
                MenuEntry::option(
                    "始终置顶",
                    MenuAction::LayerTopmost,
                    self.config.appearance.layer == DockLayer::Topmost,
                ),
            ]),
            _ => None,
        }
    }

    fn open_tray_menu(&mut self) {
        if self.tray_menu.is_none() {
            match TrayMenuView::create(self as *mut DockApp, self.dpi_scale) {
                Ok(menu) => self.tray_menu = Some(menu),
                Err(error) => {
                    tracing::warn!(%error, "could not create tray menu");
                    return;
                }
            }
        }
        let Some(cursor) = cursor_position() else {
            return;
        };
        let Some(menu) = self.tray_menu.as_ref() else {
            return;
        };
        let origin = menu.origin;
        let entries = self.tray_root_entries();
        let layout = menu.renderer.menu_layout(&entries, self.dpi_scale);
        let local = Point::new(cursor.x as f32 - origin.x, cursor.y as f32 - origin.y);
        let bubble = layout.bubble;
        let mut position = Point::new(
            local.x - bubble.center().x,
            local.y - bubble.height - 4.0 * self.dpi_scale,
        );
        let max_x = (menu.width as f32 - layout.window.width - 2.0).max(2.0);
        let max_y = (menu.height as f32 - layout.window.height - 2.0).max(2.0);
        position.x = position.x.clamp(2.0, max_x);
        position.y = position.y.clamp(2.0, max_y);

        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        menu.entries = entries;
        menu.layout = layout;
        menu.position = position;
        menu.child = None;
        menu.hovered = None;
        menu.child_hovered = None;
        menu.visible = true;
        unsafe {
            let _ = SetWindowPos(
                menu.hwnd,
                Some(HWND_TOPMOST),
                origin.x as i32,
                origin.y as i32,
                menu.width as i32,
                menu.height as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = SetTimer(Some(menu.hwnd), TIMER_TRAY_MENU, TRAY_MENU_POLL_MS, None);
            let _ = SetCapture(menu.hwnd);
        }
        self.render_tray_menu();
    }

    fn open_tray_child(&mut self, parent: usize) {
        let Some(entries) = self.tray_child_entries(parent) else {
            return;
        };
        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        let layout = menu.renderer.menu_layout(&entries, self.dpi_scale);
        let Some(row) = menu.layout.rows.iter().find(|row| row.entry == parent) else {
            return;
        };
        let root_bubble = menu.layout.bubble;
        let child_bubble = layout.bubble;
        let right = menu.position.x + root_bubble.right() + 4.0;
        let mut position = Point::new(
            right - child_bubble.left(),
            menu.position.y + row.rect.top() - child_bubble.top(),
        );
        if position.x + layout.window.width > menu.width as f32 - 2.0 {
            position.x = menu.position.x + root_bubble.left()
                - child_bubble.width
                - 4.0
                - child_bubble.left();
        }
        let max_y = (menu.height as f32 - layout.window.height - 2.0).max(2.0);
        position.y = position.y.clamp(2.0, max_y);
        menu.child = Some(TraySubmenuView {
            parent,
            entries,
            layout,
            position,
        });
        menu.hovered = Some(parent);
        menu.child_hovered = None;
        self.render_tray_menu();
    }

    fn render_tray_menu(&mut self) {
        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        let Some(root) = menu.renderer.render_menu(&MenuScene {
            entries: &menu.entries,
            layout: &menu.layout,
            hovered: menu.hovered,
            palette: self.palette,
        }) else {
            return;
        };
        let child = menu.child.as_ref().and_then(|child| {
            menu.renderer
                .render_menu(&MenuScene {
                    entries: &child.entries,
                    layout: &child.layout,
                    hovered: menu.child_hovered,
                    palette: self.palette,
                })
                .map(|frame| (frame, child.position))
        });
        menu.canvas.fill(tiny_skia::Color::TRANSPARENT);
        let paint = PixmapPaint {
            opacity: 1.0,
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: FilterQuality::Nearest,
        };
        menu.canvas.draw_pixmap(
            menu.position.x.round() as i32,
            menu.position.y.round() as i32,
            root.pixmap.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
        if let Some((frame, position)) = child {
            menu.canvas.draw_pixmap(
                position.x.round() as i32,
                position.y.round() as i32,
                frame.pixmap.as_ref(),
                &paint,
                Transform::identity(),
                None,
            );
        }
        if let Err(error) = menu
            .surface
            .present(menu.width, menu.height, menu.canvas.data())
        {
            tracing::warn!(%error, "could not present tray menu");
        }
    }

    fn close_tray_menu(&mut self) {
        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        menu.visible = false;
        menu.child = None;
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(Some(menu.hwnd), TIMER_TRAY_MENU);
            let _ = ShowWindow(menu.hwnd, SW_HIDE);
            let _ = SetWindowPos(
                menu.hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            );
        }
    }

    fn on_tray_menu_move(&mut self, point: Point) {
        let Some(menu) = self.tray_menu.as_ref() else {
            return;
        };
        let child_hit = menu.child_hit(point);
        let root_hit = menu.root_hit(point);
        if let Some(index) = child_hit {
            if menu.child_hovered != Some(index) {
                if let Some(menu) = self.tray_menu.as_mut() {
                    menu.child_hovered = Some(index);
                }
                self.render_tray_menu();
            }
            return;
        }
        // Keep the open submenu while the pointer crosses the gap/padding
        // between the parent row and the child card. Closing it on the first
        // pixel outside a row would make the submenu impossible to reach.
        if menu.child_contains(point) {
            if menu.child_hovered.is_some() {
                if let Some(menu) = self.tray_menu.as_mut() {
                    menu.child_hovered = None;
                }
                self.render_tray_menu();
            }
            return;
        }
        if let Some(index) = root_hit {
            let chevron = menu
                .entries
                .get(index)
                .is_some_and(|entry| matches!(entry, MenuEntry::Action { chevron: true, .. }));
            if chevron {
                if menu
                    .child
                    .as_ref()
                    .is_none_or(|child| child.parent != index)
                {
                    self.open_tray_child(index);
                }
            } else if menu.child.is_some() || menu.hovered != Some(index) {
                if let Some(menu) = self.tray_menu.as_mut() {
                    menu.child = None;
                    menu.child_hovered = None;
                    menu.hovered = Some(index);
                }
                self.render_tray_menu();
            }
            return;
        }
        if menu.hovered.is_some() || menu.child_hovered.is_some() {
            if let Some(menu) = self.tray_menu.as_mut() {
                menu.child_hovered = None;
                menu.hovered = None;
            }
            self.render_tray_menu();
        }
    }

    fn on_tray_menu_click(&mut self, point: Point) {
        let Some(menu) = self.tray_menu.as_ref() else {
            return;
        };
        let child_hit = menu.child_hit(point);
        let root_hit = menu.root_hit(point);
        if let Some(index) = child_hit {
            let action = menu
                .child
                .as_ref()
                .and_then(|child| child.entries.get(index))
                .and_then(|entry| match entry {
                    MenuEntry::Action { action, .. } => Some(*action),
                    MenuEntry::Separator => None,
                });
            if let Some(action) = action {
                self.close_tray_menu();
                self.perform_action(action);
            }
            return;
        }
        if let Some(index) = root_hit {
            let entry = menu.entries.get(index);
            let chevron =
                entry.is_some_and(|entry| matches!(entry, MenuEntry::Action { chevron: true, .. }));
            let action = entry.and_then(|entry| match entry {
                MenuEntry::Action { action, .. } => Some(*action),
                MenuEntry::Separator => None,
            });
            if chevron {
                self.open_tray_child(index);
            } else if let Some(action) = action {
                self.close_tray_menu();
                self.perform_action(action);
            }
            return;
        }
        self.close_tray_menu();
    }

    fn handle_tray_menu_message(
        &mut self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_MOUSEMOVE => {
                self.on_tray_menu_move(mouse_point(lparam));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                self.on_tray_menu_click(mouse_point(lparam));
                LRESULT(0)
            }
            WM_RBUTTONUP | WM_CLOSE => {
                self.close_tray_menu();
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == TIMER_TRAY_MENU {
                    if key_down(VK_ESCAPE) {
                        self.close_tray_menu();
                    } else if key_down(VK_LBUTTON) || key_down(VK_RBUTTON) {
                        if let (Some(cursor), Some(menu)) =
                            (cursor_position(), self.tray_menu.as_ref())
                        {
                            let local = Point::new(
                                cursor.x as f32 - menu.origin.x,
                                cursor.y as f32 - menu.origin.y,
                            );
                            if !menu.contains(local) {
                                self.close_tray_menu();
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_PAINT => LRESULT(0),
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn set_theme(&mut self, theme: ThemeMode) {
        if self.config.appearance.theme == theme {
            return;
        }
        self.config.appearance.theme = theme;
        self.theme = resolve_theme(theme);
        self.palette = palette_for(&self.config, self.theme);
        self.update_native_glass();
        self.mark_dirty();
        self.save_now();
        self.render();
        self.animate();
    }

    fn set_layer(&mut self, layer: DockLayer) {
        if self.config.appearance.layer == layer {
            return;
        }
        self.config.appearance.layer = layer;
        self.apply_dock_layer();
        self.mark_dirty();
        self.save_now();
        self.render();
    }

    fn open_settings(&mut self) {
        if self.read_only {
            self.set_error("配置为只读，暂时无法打开设置".to_string());
            return;
        }
        if let Err(error) = self.settings.open(self.hwnd, self.store.path()) {
            match error {
                SettingsLaunchError::NotBuilt => self.set_error(
                    "设置组件未构建：请运行 scripts/build-settings.ps1 或把 GlideDock.Settings.exe 放到 settings 目录"
                        .to_string(),
                ),
                error => {
                    tracing::error!(%error, "failed to launch settings");
                    self.set_error(format!("无法启动设置组件：{error}"));
                }
            }
        }
    }

    fn quit(&mut self) {
        self.shutdown();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    /// Saves state and stops timers. Safe to call more than once.
    pub fn shutdown(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        self.save_now();
        self.kill_all_timers();
        release_mouse();
        self.settings.close();
        if let Some(mut tray) = self.tray.take() {
            tray.remove();
        }
        self.tray_menu = None;
        self.glass = None;
        self.native_glass = false;
        self.window_visible = false;
    }

    /// Stops timers without touching the user's configuration. Used by
    /// `--selftest`, which must be side-effect free.
    pub fn shutdown_without_save(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        self.kill_all_timers();
        release_mouse();
        self.settings.close();
        if let Some(mut tray) = self.tray.take() {
            tray.remove();
        }
        self.tray_menu = None;
        self.glass = None;
        self.native_glass = false;
        self.window_visible = false;
    }

    // -------------------------------------------------------------- rendering

    fn relayout_window(&mut self, show: bool) {
        let monitor = metrics_for_window(self.hwnd);
        self.dpi_scale = monitor.dpi_scale;
        self.refresh_hz = monitor.refresh_hz;
        self.frame_interval_ms = frame_interval_ms(self.refresh_hz);
        let mut metrics = Metrics::new(
            &self.config.appearance,
            &self.config.behavior,
            &self.config.motion,
            self.dpi_scale,
        );
        let available_width = (monitor.work.width() as f32 - 2.0 * metrics.shadow).max(1.0);
        if metrics.window_width(self.config.items.len()) > available_width {
            metrics.fit_to_width(available_width, self.config.items.len());
            tracing::warn!(
                icons = self.config.items.len(),
                available_width,
                "dock does not fit the work area; icons were shrunk for display"
            );
        }
        self.metrics = metrics;
        self.geometry = DockGeometry::with_items(&self.config.items, &self.metrics);
        let width = self.geometry.window.width.ceil().max(1.0) as u32;
        let height = self.geometry.window.height.ceil().max(1.0) as u32;
        let margin = self.config.appearance.edge_margin * self.dpi_scale;
        let offset = self.config.monitor.offset * self.dpi_scale;
        let (x, y) = place_dock(monitor.work, width, height, margin, offset);
        tracing::info!(
            dpi = self.dpi_scale,
            width,
            height,
            x,
            y,
            work_left = monitor.work.left,
            work_top = monitor.work.top,
            work_right = monitor.work.right,
            work_bottom = monitor.work.bottom,
            "dock window laid out"
        );
        set_bounds(self.hwnd, x, y, width, height, show);
        self.window_origin = (x, y);
        self.update_native_glass();
        if self.window_visible
            && !self.native_glass
            && !self.glass_failed
            && self.config.appearance.frosted_glass
        {
            self.ensure_native_glass();
        }
        if let Err(error) = self.surface.resize(width, height) {
            tracing::error!(%error, "failed to resize layered surface");
        }
        self.icons.set_capacity(self.config.items.len().max(64));
        self.update_motion_layout(0.0);
        self.close_menu_immediately();
        if self.frame_timer {
            // A DPI/refresh change can alter the timer interval; restart the
            // frame timer so the new sampling density takes effect.
            self.kill_frame_timer();
            self.animate();
        }
    }

    fn glass_rect(&self) -> (i32, i32, u32, u32, f32) {
        let base = self.geometry.dock_base;
        let x = self.window_origin.0 + base.x.round() as i32;
        let y = self.window_origin.1 + base.y.round() as i32;
        let width = base.width.round().max(1.0) as u32;
        let height = base.height.round().max(1.0) as u32;
        (x, y, width, height, self.metrics.radius)
    }

    fn ensure_native_glass(&mut self) {
        if self.native_glass || self.glass_failed || !self.config.appearance.frosted_glass {
            return;
        }
        if let Some(glass) = &self.glass {
            glass.set_visible(true);
            glass.place_below(self.hwnd);
            self.native_glass = true;
            self.update_native_glass();
            self.update_motion_layout(0.0);
            return;
        }
        let (x, y, width, height, radius) = self.glass_rect();
        let dark = self.theme == ResolvedTheme::Dark;
        match GlassBackdrop::create(
            x,
            y,
            width,
            height,
            radius,
            dark,
            self.config.appearance.opacity,
        ) {
            Some(glass) => {
                glass.place_below(self.hwnd);
                self.glass = Some(glass);
                self.native_glass = true;
                self.update_motion_layout(0.0);
                tracing::info!(width, height, "Windows.UI.Composition glass enabled");
            }
            None => {
                self.glass_failed = true;
            }
        }
    }

    fn update_native_glass(&mut self) {
        if !self.native_glass {
            return;
        }
        let (x, y, width, height, radius) = self.glass_rect();
        let dark = self.theme == ResolvedTheme::Dark;
        if let Some(glass) = &mut self.glass {
            glass.update(
                x,
                y,
                width,
                height,
                radius,
                dark,
                self.config.appearance.opacity,
            );
            glass.place_below(self.hwnd);
        }
    }

    fn refresh_glass_state(&mut self) {
        if !self.config.appearance.frosted_glass {
            if let Some(glass) = &self.glass {
                glass.set_visible(false);
            }
            self.native_glass = false;
            self.glass_failed = false;
            return;
        }
        self.glass_failed = false;
        self.ensure_native_glass();
    }
    fn update_motion_layout(&mut self, dt: f32) -> MotionSnapshot {
        let now = self.now();
        let snapshot = self.motion.update(
            now,
            dt,
            &self.config.items,
            &self.geometry,
            &self.metrics,
            &self.state.running,
        );
        self.layout = FrameLayout::compute(&self.geometry, &self.metrics, &snapshot.layout_scales);
        if self.native_glass {
            self.layout.dock = self.geometry.dock_base;
        }
        snapshot
    }

    fn icon_animations(snapshot: &MotionSnapshot) -> Vec<IconAnimation> {
        snapshot
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
            .collect()
    }

    fn drag_visual(&self) -> Option<DragVisual> {
        let drag = self.state.drag.as_ref()?;
        if !drag.active {
            return None;
        }
        let remaining: Vec<Rect> = self
            .layout
            .icons
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != drag.from)
            .map(|(_, rect)| *rect)
            .collect();
        let x = insertion_x(&remaining, drag.insertion, self.geometry.gap);
        Some(DragVisual {
            index: drag.from,
            cursor: drag.cursor,
            grab_dx: drag.grab_dx,
            insertion_x: x,
            active: true,
        })
    }

    fn draw(&mut self) -> Option<Pixmap> {
        let snapshot = self.update_motion_layout(0.0);
        let drag = self.drag_visual();
        let animations = Self::icon_animations(&snapshot);
        let scene = DockScene {
            config: &self.config,
            metrics: &self.metrics,
            geometry: &self.geometry,
            layout: &self.layout,
            animations: &animations,
            state: &self.state,
            palette: self.palette,
            icons: &self.icons,
            native_glass: self.native_glass,
            glass: self.native_glass,
            tooltip_progress: self.tooltip_spring.value(),
            hover_progress: snapshot.hover_progress,
            error: self.error_message.as_deref(),
            error_progress: self.error_spring.value(),
            drag,
        };
        let mut pixmap = self.renderer.render_dock(&scene)?;

        if let Some(menu) = self.menu.as_ref() {
            let progress = self.menu_spring.value().clamp(0.0, 1.0);
            if progress > 0.01 {
                let offset = ((1.0 - progress) * 9.0 * self.dpi_scale).round() as i32;
                let paint = PixmapPaint {
                    opacity: progress,
                    blend_mode: tiny_skia::BlendMode::SourceOver,
                    quality: FilterQuality::Nearest,
                };
                pixmap.draw_pixmap(
                    menu.position.x.round() as i32,
                    menu.position.y.round() as i32 + offset,
                    menu.pixmap.as_ref(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
        }
        Some(pixmap)
    }

    fn render(&mut self) {
        let Some(pixmap) = self.draw() else {
            return;
        };
        if let Err(error) = self
            .surface
            .present(pixmap.width(), pixmap.height(), pixmap.data())
        {
            tracing::error!(%error, "present failed");
        }
    }

    fn on_frame(&mut self) {
        let delta = self.clock.delta();
        let snapshot = self.update_motion_layout(delta);
        let mut animating = snapshot.active;
        animating |= self.tooltip_spring.step(delta);
        animating |= self.error_spring.step(delta);
        animating |= self.menu_spring.step(delta);

        if self.tooltip_spring.value() <= 0.01 && self.state.hovered.is_none() {
            self.state.tooltip.index = None;
        }
        if self.error_spring.value() <= 0.01 && self.error_spring.spring.target <= 0.0 {
            self.error_message = None;
        }
        let menu_finished =
            self.menu.as_ref().is_some_and(|menu| menu.closing) && self.menu_spring.value() <= 0.01;
        if menu_finished {
            self.menu = None;
            self.menu_pressed = None;
            self.menu_opened_at = None;
            self.menu_foreground = None;
            self.state.menu_open = false;
            self.kill_timer(TIMER_MENU);
            self.apply_dock_layer();
        }

        self.render();
        if !animating {
            self.kill_frame_timer();
        }
    }

    fn animate(&mut self) {
        if self.frame_timer {
            return;
        }
        self.clock.reset();
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                TIMER_FRAME,
                self.frame_interval_ms.max(1),
                None,
            );
        }
        self.frame_timer = true;
    }

    fn kill_frame_timer(&mut self) {
        if self.frame_timer {
            self.kill_timer(TIMER_FRAME);
            self.frame_timer = false;
        }
    }

    fn kill_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    fn kill_all_timers(&mut self) {
        for id in [
            TIMER_FRAME,
            TIMER_TOOLTIP,
            TIMER_SAVE,
            TIMER_ERROR,
            TIMER_MENU,
            TIMER_RUNNING,
        ] {
            self.kill_timer(id);
        }
        self.frame_timer = false;
        self.tooltip_timer = false;
        self.save_timer = false;
        self.error_timer = false;
        self.running_timer = false;
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_FRAME => {
                if self.frame_timer {
                    self.on_frame();
                }
            }
            TIMER_TOOLTIP => {
                self.kill_timer(TIMER_TOOLTIP);
                self.tooltip_timer = false;
                if !self.state.menu_open && self.config.behavior.show_tooltips {
                    if let Some(index) = self.state.hovered {
                        self.state.tooltip.index = Some(index);
                        self.tooltip_spring.set_target(1.0);
                        self.animate();
                    }
                }
            }
            TIMER_SAVE => {
                self.kill_timer(TIMER_SAVE);
                self.save_timer = false;
                self.save_now();
            }
            TIMER_ERROR => {
                self.kill_timer(TIMER_ERROR);
                self.error_timer = false;
                self.error_spring.set_target(0.0);
                self.animate();
            }
            TIMER_MENU => self.on_menu_timer(),
            TIMER_RUNNING => {
                self.kill_timer(TIMER_RUNNING);
                self.running_timer = false;
                self.refresh_running();
            }
            _ => {}
        }
    }

    fn on_icons_ready(&mut self) {
        let results = self.icon_service.drain();
        let mut changed = false;
        for result in results {
            if result.generation != self.icon_generation {
                continue;
            }
            if !self
                .config
                .items
                .iter()
                .any(|item| item.id == result.item_id)
            {
                continue;
            }
            if let Some(image) = result.image {
                if let Some(pixmap) = icon_to_pixmap(image) {
                    self.icons.insert(result.item_id, pixmap);
                    changed = true;
                }
            } else if let Some(error) = result.error {
                tracing::debug!(item = result.item_id, %error, "icon unavailable");
            }
        }
        if changed {
            self.render();
        }
    }

    fn request_all_icons(&self) {
        for item in &self.config.items {
            self.request_item_icons(item);
        }
    }

    fn request_item_icons(&self, item: &DockItem) {
        if item.kind == ItemKind::Separator || item.kind == ItemKind::Widget {
            return;
        }
        let mut paths = Vec::new();
        if item.launch_type == LaunchType::PackagedApp {
            let aumid = item.target_path.trim();
            if !aumid.is_empty() {
                paths.push(format!("shell:AppsFolder\\{aumid}"));
            }
        } else {
            if let Some(shortcut) = &item.shortcut_path {
                if !shortcut.trim().is_empty() {
                    paths.push(shortcut.clone());
                }
            }
            if !item.target_path.trim().is_empty() {
                paths.push(item.target_path.clone());
            }
        }
        if paths.is_empty() {
            return;
        }
        self.icon_service
            .request(item.id, paths, ICON_MASTER_SIZE, self.icon_generation);
    }

    fn refresh_running(&mut self) {
        let paths = visible_window_paths();
        let mut running = BTreeSet::new();
        for item in &self.config.items {
            if item.kind != ItemKind::Application {
                continue;
            }
            let target = item.target_path.trim();
            if !target.is_empty() && paths.iter().any(|path| target_matches(target, path)) {
                running.insert(item.id);
            }
        }

        for item in &self.config.items {
            if running.contains(&item.id) {
                self.motion.window_detected(item.id);
            } else {
                self.motion.window_closed(item.id);
            }
        }
        if running != self.state.running {
            self.state.running = running;
            // A running-dot change is a static state change; no frame timer is
            // needed. Launch/waiting animations keep their own timer alive.
            self.render();
        }
    }

    fn on_drop(&mut self, payload: DropPayload) {
        if payload.paths.is_empty() {
            return;
        }
        let at = payload
            .client_point
            .map(|point| insertion_index(&self.layout.icons, point.x));
        self.dispatch(DockCommand::AddPaths {
            paths: payload.paths,
            at,
        });
    }

    fn on_theme_changed(&mut self) {
        let theme = resolve_theme(self.config.appearance.theme);
        if theme != self.theme {
            self.theme = theme;
            self.palette = palette_for(&self.config, theme);
            self.update_native_glass();
            self.render();
        }
    }

    fn reset_tooltip(&mut self) {
        self.kill_timer(TIMER_TOOLTIP);
        self.tooltip_timer = false;
        self.state.tooltip.index = None;
        self.tooltip_spring.set_target(0.0);
        if self.state.hovered.is_some()
            && self.config.behavior.show_tooltips
            && !self.state.menu_open
        {
            unsafe {
                let _ = SetTimer(Some(self.hwnd), TIMER_TOOLTIP, TOOLTIP_DELAY_MS, None);
            }
            self.tooltip_timer = true;
        }
    }

    fn hide_tooltip(&mut self, clear: bool) {
        self.kill_timer(TIMER_TOOLTIP);
        self.tooltip_timer = false;
        if clear {
            self.state.tooltip.index = None;
        }
        self.tooltip_spring.set_target(0.0);
        self.animate();
    }

    fn set_error(&mut self, message: String) {
        tracing::warn!("{message}");
        self.error_message = Some(message);
        self.error_spring.spring.value = 0.0;
        self.error_spring.set_target(1.0);
        self.kill_timer(TIMER_ERROR);
        unsafe {
            let _ = SetTimer(Some(self.hwnd), TIMER_ERROR, ERROR_LIFETIME_MS, None);
        }
        self.error_timer = true;
        self.animate();
        self.render();
    }

    fn hit_test(&self, lparam: LPARAM) -> i32 {
        let screen = screen_point(lparam);
        let client = screen_to_client(self.hwnd, screen);
        let point = Point::new(client.x as f32, client.y as f32);
        if self.menu.as_ref().is_some_and(|menu| menu.contains(point)) {
            return HTCLIENT as i32;
        }
        let margin = 6.0 * self.dpi_scale;
        if self.layout.dock.inset(-margin).contains(point) || self.hit_icon(point).is_some() {
            return HTCLIENT as i32;
        }
        HTTRANSPARENT
    }

    /// Runs a lightweight self test: renders one frame and checks the idle
    /// timer invariant. It writes nothing, including the configuration.
    pub fn selftest(&mut self) -> bool {
        let snapshot = self.update_motion_layout(0.0);
        let Some(pixmap) = self.draw() else {
            tracing::error!("selftest: renderer produced no pixmap");
            return false;
        };
        let timers_idle = !self.frame_timer
            && !self.tooltip_timer
            && !self.save_timer
            && !self.error_timer
            && !self.running_timer
            && !self.state.menu_open;
        let ok = timers_idle
            && !snapshot.active
            && pixmap.width() > 0
            && pixmap.height() > 0
            && pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0)
            && self.layout.icons.len() == self.config.items.len();
        tracing::info!(
            items = self.config.items.len(),
            width = pixmap.width(),
            height = pixmap.height(),
            dpi = self.dpi_scale,
            frame_timer = self.frame_timer,
            tooltip_timer = self.tooltip_timer,
            save_timer = self.save_timer,
            error_timer = self.error_timer,
            running_timer = self.running_timer,
            menu_open = self.state.menu_open,
            "selftest finished"
        );
        ok
    }
}

fn target_matches(target: &str, visible: &str) -> bool {
    if target.eq_ignore_ascii_case(visible) {
        return true;
    }
    // Packaged applications are launched from WindowsApps, while shortcuts may
    // point at an execution alias elsewhere. Only then is a file-name match
    // allowed; otherwise any unrelated process with the same executable name
    // would light up the running dot.
    let same_name = Path::new(target)
        .file_name()
        .zip(Path::new(visible).file_name())
        .is_some_and(|(a, b)| {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        });
    same_name && (is_packaged_path(target) || is_packaged_path(visible))
}

fn is_packaged_path(path: &str) -> bool {
    path.to_ascii_lowercase().contains("\\windowsapps\\")
}

fn resolve_theme(mode: ThemeMode) -> ResolvedTheme {
    mode.resolve(prefers_light_theme())
}

fn palette_for(config: &DockConfig, theme: ResolvedTheme) -> Palette {
    Palette::for_theme(theme).with_accent(config.appearance.accent)
}

/// Resolves a possibly relative configuration path before handing it to the
/// settings process, whose working directory is the settings folder.
fn absolute_config_path(path: &Path) -> PathBuf {
    path.canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn compute_unresolved(config: &DockConfig) -> BTreeSet<u32> {
    config
        .items
        .iter()
        .filter(|item| {
            item.kind != ItemKind::Separator
                && item.kind != ItemKind::Widget
                && item.launch_type != LaunchType::PackagedApp
                && (item.target_path.trim().is_empty() || !Path::new(&item.target_path).exists())
        })
        .map(|item| item.id)
        .collect()
}

fn icon_to_pixmap(image: IconImage) -> Option<Pixmap> {
    let size = IntSize::from_wh(image.width, image.height)?;
    Pixmap::from_vec(image.rgba, size)
}

fn mouse_point(lparam: LPARAM) -> Point {
    let value = lparam.0 as u32;
    let x = (value & 0xFFFF) as u16 as i16 as i32;
    let y = ((value >> 16) & 0xFFFF) as u16 as i16 as i32;
    Point::new(x as f32, y as f32)
}

fn screen_point(lparam: LPARAM) -> POINT {
    let value = lparam.0 as u32;
    POINT {
        x: (value & 0xFFFF) as u16 as i16 as i32,
        y: ((value >> 16) & 0xFFFF) as u16 as i16 as i32,
    }
}

fn key_down(key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
    (state as u16 & 0x8000) != 0
}

#[cfg(test)]
mod tests {
    use super::target_matches;

    #[test]
    fn exact_paths_match_case_insensitively() {
        assert!(target_matches(
            r"C:\Tools\Editor.EXE",
            r"c:\tools\editor.exe"
        ));
    }

    #[test]
    fn same_name_outside_windowsapps_does_not_match() {
        assert!(!target_matches(
            r"C:\Tools\python.exe",
            r"D:\Other\python.exe"
        ));
    }

    #[test]
    fn packaged_alias_matches_windowsapps_executable() {
        assert!(target_matches(
            r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\notepad.exe",
            r"C:\Program Files\WindowsApps\Microsoft.WindowsNotepad_11.0\Notepad.exe"
        ));
        assert!(target_matches(
            r"C:\Tools\app.exe",
            r"C:\Program Files\WindowsApps\Vendor.App_1.0\App.exe"
        ));
    }
}
