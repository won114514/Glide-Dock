//! Dock application state machine: input → commands → state → pixels → config.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use dock_core::events::DockCommand;
use dock_core::geometry::{Point, Rect};
use dock_core::layout::{DockGeometry, FrameLayout, Metrics, insertion_index, insertion_x};
use dock_core::models::{DockConfig, DockFont, DockItem, DockLayer, ItemKind, LaunchType};
use dock_core::settings_ipc::{COPYDATA_UI_MAGIC, MAX_COPYDATA_BYTES, NoticeKind, SettingsValues};
use dock_core::state::{DragSession, MenuAction, MenuEntry, MenuIcon, MenuTarget};
use dock_core::theme::{Palette, ResolvedTheme, ThemeMode};
use dock_core::{DockState, MotionEngine, MotionSnapshot, MotionTuning};
use dock_platform_windows::{
    DropPayload, GlassBackdrop, IconImage, IconService, LayeredSurface, MENU_WINDOW_CLASS,
    MonitorMetrics, PlatformError, ResolveService, ResolvedEntry, StoreAppService, TrayEvent,
    TrayIcon, WM_APP_LAUNCH_RESULT, WM_APP_STORE_APPS, WM_APP_WINDOW_EVENT, WM_ICON_READY,
    WM_PATHS_RESOLVED, WindowTracker, accept_files, animations_enabled, capture_mouse,
    clamp_window, copydata_payload, create_layered_popup, cursor_position,
    is_builtin_admin_without_approval, is_elevated, is_low_integrity, metrics_for_window,
    open_path, open_start_menu, parse_drop, pick_store_app, place_above_desktop, place_dock,
    prefers_light_theme, register_class, release_mouse, reveal, screen_to_client, set_bounds,
    set_topmost, show_no_activate, trim_working_set, visible_window_paths, windows_build,
};
use dock_render::icons::IconMap;
use dock_render::{
    DockFrame, DockScene, DragVisual, HitShape, IconAnimation, MenuLayout, MenuScene, Renderer,
    StartMenuStyle,
};
use dock_storage::{ConfigStore, LoadOutcome, StorageError};
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_ESCAPE, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Shell::HDROP;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetForegroundWindow, GetSystemMetrics,
    GetWindowLongPtrW, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, KillTimer, MA_NOACTIVATE,
    PostQuitMessage, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_HIDE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetTimer, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_COPYDATA, WM_DESTROY,
    WM_DROPFILES, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER,
};

use crate::launcher::LauncherService;
use crate::presenter::{DockPresenter, Submission};
use crate::render_loop::{FrameScheduler, TIMER_FRAME};
use crate::wndproc::dock_wndproc;
mod frame_stats;
mod settings;
use frame_stats::FrameStats;
use settings::SettingsHost;

/// Mouse leave notification (defined in Win32::UI::Controls).
const WM_MOUSELEAVE: u32 = 0x02A3;
pub const WM_APP_OPEN_SETTINGS: u32 = WM_APP + 8;
pub const WM_APP_ENSURE_TRAY: u32 = WM_APP + 9;
pub(crate) const WM_APP_SETTINGS_COMMAND: u32 = WM_APP + 7;

const TIMER_TOOLTIP: usize = 2;
const TIMER_SAVE: usize = 3;
const TIMER_ERROR: usize = 4;
const TIMER_MENU: usize = 5;
const TIMER_RUNNING: usize = 6;
const TIMER_TRAY_RETRY: usize = 8;
const TRAY_RETRY_DELAYS_MS: [u32; 6] = [1_000, 2_000, 4_000, 8_000, 16_000, 29_000];

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
    hwnd: HWND,
    surface: LayeredSurface,
    entries: Vec<MenuEntry>,
    layout: MenuLayout,
    screen_origin: POINT,
    pixmap: Arc<Pixmap>,
    hovered: Option<usize>,
    dirty: bool,
    last_alpha: u8,
    last_offset: i32,
    closing: bool,
}

impl MenuView {
    fn hit(&self, point: Point) -> Option<usize> {
        if self.closing {
            return None;
        }
        self.layout.hit(point)
    }

    fn contains(&self, point: Point) -> bool {
        !self.closing
            && HitShape {
                rect: self.layout.bubble,
                radius: self.layout.radius,
            }
            .contains(point)
    }
}

impl Drop for MenuView {
    fn drop(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

const TRAY_MENU_CLASS: &str = "Glide-Dock.TrayMenu";
const TIMER_TRAY_MENU: usize = 7;
const TRAY_MENU_POLL_MS: u32 = 40;

struct TraySubmenuView {
    parent: usize,
    entries: Vec<MenuEntry>,
    layout: MenuLayout,
    screen_position: Point,
}

struct TrayMenuView {
    hwnd: HWND,
    surface: LayeredSurface,
    origin: Point,
    width: u32,
    height: u32,
    canvas: Option<Pixmap>,
    entries: Vec<MenuEntry>,
    layout: MenuLayout,
    screen_position: Point,
    child: Option<TraySubmenuView>,
    hovered: Option<usize>,
    child_hovered: Option<usize>,
    visible: bool,
}

impl TrayMenuView {
    fn create(owner: HWND) -> Result<Self, Box<dyn std::error::Error>> {
        let instance = register_class(TRAY_MENU_CLASS, Some(dock_wndproc))?;
        let hwnd = create_layered_popup(instance, TRAY_MENU_CLASS, "Glide-Dock 菜单", 1, 1)?;
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, GetWindowLongPtrW(owner, GWLP_USERDATA));
        }
        let surface = LayeredSurface::new(hwnd)?;
        Ok(Self {
            hwnd,
            surface,
            origin: Point::new(0.0, 0.0),
            width: 1,
            height: 1,
            canvas: None,
            entries: Vec::new(),
            layout: dock_render::menu_layout(&dock_render::text::FontStack::empty(), &[], 1.0),
            screen_position: Point::new(0.0, 0.0),
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
        let screen = Point::new(self.origin.x + point.x, self.origin.y + point.y);
        self.layout.hit(screen - self.screen_position)
    }

    fn child_hit(&self, point: Point) -> Option<usize> {
        let screen = Point::new(self.origin.x + point.x, self.origin.y + point.y);
        self.child
            .as_ref()
            .and_then(|child| child.layout.hit(screen - child.screen_position))
    }

    fn contains(&self, point: Point) -> bool {
        let screen = Point::new(self.origin.x + point.x, self.origin.y + point.y);
        self.visible
            && (self.layout.inside_bubble(screen - self.screen_position)
                || self.child.as_ref().is_some_and(|child| {
                    child.layout.inside_bubble(screen - child.screen_position)
                }))
    }

    fn child_contains(&self, point: Point) -> bool {
        let screen = Point::new(self.origin.x + point.x, self.origin.y + point.y);
        self.child
            .as_ref()
            .is_some_and(|child| child.layout.inside_bubble(screen - child.screen_position))
    }
}

impl Drop for TrayMenuView {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TRAY_MENU);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The complete dock application.
pub struct DockApp {
    hwnd: HWND,
    presenter: DockPresenter,
    hit_shapes: Vec<HitShape>,
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
    active_dock_font: DockFont,
    icons: IconMap,
    icon_service: IconService,
    icon_generation: u64,
    requested_icon_size: u32,
    resolve_service: ResolveService,
    monitor: MonitorMetrics,
    frames: FrameScheduler,
    frame_stats: Option<FrameStats>,
    tooltip_timer: bool,
    save_timer: bool,
    error_timer: bool,
    running_timer: bool,
    tray_retry_timer: bool,
    tray_retry_index: usize,
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
    updater: crate::update::UpdateService,
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
        let presenter = DockPresenter::new(hwnd)?;
        let icon_service = IconService::spawn(hwnd);
        let resolve_service = ResolveService::spawn(hwnd);
        let window_tracker = WindowTracker::install(hwnd);
        let updater = crate::update::UpdateService::new(persist, hwnd.0 as isize);
        let dock_font = config.appearance.dock_font;
        let mut renderer = Renderer::with_text_backend(Arc::new(
            crate::text::NativeText::for_dock_font(dock_font),
        ));
        let build = windows_build();
        renderer.set_start_style(if build == 0 || build >= 22000 {
            StartMenuStyle::Windows11
        } else {
            StartMenuStyle::Windows10
        });

        let mut app = Self {
            hwnd,
            presenter,
            hit_shapes: Vec::new(),
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
            active_dock_font: dock_font,
            icons: IconMap::new(),
            icon_service,
            icon_generation: 0,
            requested_icon_size: Self::desired_icon_size(&metrics),
            resolve_service,
            monitor,
            frames: FrameScheduler::new(hwnd, monitor.refresh_hz),
            frame_stats: FrameStats::from_env(),
            tooltip_timer: false,
            save_timer: false,
            error_timer: false,
            running_timer: false,
            tray_retry_timer: false,
            tray_retry_index: 0,
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
            updater,
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
        if show {
            show_no_activate(hwnd);
            app.window_visible = true;
            app.ensure_native_glass();
            app.apply_dock_layer();
            app.on_frame();
        }
        accept_files(hwnd);
        if show {
            let tray = TrayIcon::install(hwnd);
            let installed = tray.is_installed();
            app.tray = Some(tray);
            if !installed {
                app.schedule_tray_retry();
            }
            app.check_startup_privilege_notice();
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
        if self.menu.as_ref().is_some_and(|menu| menu.hwnd == hwnd) {
            return self.handle_menu_window_message(hwnd, msg, wparam, lparam);
        }
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
                // Releasing capture on a normal mouse-up is synchronous. The
                // drag has already been taken and the click should complete.
                if self.state.drag.take().is_some() || self.state.pressed.is_some() {
                    self.cancel_pressed_interactions();
                    self.state.pressed = None;
                }
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
            WM_SETTINGCHANGE => {
                self.system_animations = animations_enabled();
                self.apply_motion_config();
                self.on_theme_changed();
                self.settings.sync_system_theme();
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
                if self.presenter.recover_on_paint() {
                    self.animate();
                }
                LRESULT(0)
            }
            WM_COPYDATA => {
                if let Some(bytes) =
                    unsafe { copydata_payload(lparam, COPYDATA_UI_MAGIC, MAX_COPYDATA_BYTES) }
                {
                    self.on_settings_copydata(&bytes);
                    return LRESULT(1);
                }
                LRESULT(0)
            }
            WM_APP_SETTINGS_COMMAND => {
                self.on_update_worker_ready();
                LRESULT(0)
            }
            WM_APP_OPEN_SETTINGS => {
                self.show_settings();
                LRESULT(1)
            }
            WM_APP_ENSURE_TRAY => {
                if self.tray.is_some() {
                    self.ensure_tray_installed();
                    LRESULT(1)
                } else {
                    LRESULT(0)
                }
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
                self.quit();
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
        if topmost != self.layer_topmost {
            if topmost {
                set_topmost(self.hwnd);
            } else {
                place_above_desktop(self.hwnd);
            }
            self.layer_topmost = topmost;
            self.place_glass_below_dock();
        }
    }

    fn place_glass_below_dock(&self) {
        if let Some(glass) = &self.glass {
            if self.layer_topmost {
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
        self.layout.hit_icon(point)
    }

    fn on_mouse_move(&mut self, lparam: LPARAM) {
        let point = mouse_point(lparam);

        if self.menu.is_some() {
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
                let threshold = DRAG_THRESHOLD_DIP * self.monitor.dpi_scale;
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
                self.animate();
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
        if self.mouse_tracked
            || self
                .frame_stats
                .as_ref()
                .is_some_and(FrameStats::uses_posted_pointer)
        {
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
        let _ = point;
        if self.menu.is_some() {
            self.close_menu();
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
        let Some(drag) = self.state.drag.take() else {
            release_mouse();
            return;
        };
        self.state.pressed = None;
        release_mouse();

        if drag.active {
            let remove_threshold =
                self.layout.dock.top() - DRAG_REMOVE_GAP_DIP * self.monitor.dpi_scale;
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
        if self
            .hit_shapes
            .first()
            .is_some_and(|shape| shape.contains(point))
        {
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
        let layout = self.renderer.menu_layout(&entries, self.monitor.dpi_scale);
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
            MenuTarget::Item(_) => self.layout.dock.top() - 8.0 * self.monitor.dpi_scale,
            MenuTarget::Background => point.y - 4.0 * self.monitor.dpi_scale,
        };

        let win_w = frame.pixmap.width();
        let win_h = frame.pixmap.height();
        let screen_anchor_x = self.window_origin.0 + anchor_x.round() as i32;
        let screen_anchor_y = self.window_origin.1 + anchor_bottom.round() as i32;
        let desired_x = screen_anchor_x - layout.bubble.center().x.round() as i32;
        let desired_y = screen_anchor_y - layout.bubble.bottom().round() as i32;
        let (x, y) = clamp_window(desired_x, desired_y, win_w, win_h, self.monitor.work);

        let Ok(instance) = register_class(MENU_WINDOW_CLASS, Some(dock_wndproc)) else {
            return;
        };
        let Ok(hwnd) = create_layered_popup(
            instance,
            MENU_WINDOW_CLASS,
            "Glide-Dock 上下文菜单",
            win_w,
            win_h,
        ) else {
            return;
        };
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                GetWindowLongPtrW(self.hwnd, GWLP_USERDATA),
            );
        }
        let Ok(mut surface) = LayeredSurface::new(hwnd) else {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(hwnd);
            }
            return;
        };
        let _ = surface.present_alpha(win_w, win_h, frame.pixmap.data(), 0);
        let initial_offset = (9.0 * self.monitor.dpi_scale).round() as i32;
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y + initial_offset,
                win_w as i32,
                win_h as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }

        self.menu = Some(MenuView {
            hwnd,
            surface,
            entries,
            layout: frame.layout,
            screen_origin: POINT { x, y },
            pixmap: Arc::new(frame.pixmap),
            hovered: None,
            dirty: true,
            last_alpha: 0,
            last_offset: initial_offset,
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
    }

    fn update_menu_hover(&mut self, point: Point) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        if menu.closing {
            return;
        }
        let hovered = menu.hit(point);
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
            menu.pixmap = Arc::new(frame.pixmap);
            menu.hovered = hovered;
            menu.dirty = true;
        }
        self.animate();
    }

    fn handle_menu_window_message(
        &mut self,
        hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_MOUSEMOVE => {
                self.update_menu_hover(mouse_point(lparam));
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if self.menu.as_ref().is_some_and(|menu| menu.closing) {
                    return LRESULT(0);
                }
                let point = mouse_point(lparam);
                if let Some(entry) = self.menu_hit(point) {
                    self.menu_pressed = Some(entry);
                } else if !self.menu.as_ref().is_some_and(|menu| menu.contains(point)) {
                    self.close_menu();
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(entry) = self.menu_pressed.take()
                    && self.menu_hit(mouse_point(lparam)) == Some(entry)
                {
                    self.activate_menu_entry(entry);
                }
                LRESULT(0)
            }
            WM_RBUTTONUP | WM_CLOSE => {
                self.close_menu();
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_NCHITTEST => {
                let client = screen_to_client(hwnd, screen_point(lparam));
                let point = Point::new(client.x as f32, client.y as f32);
                if self.menu.as_ref().is_some_and(|menu| menu.contains(point)) {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                }
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) },
        }
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
        if menu_age >= 0.25
            && let Some(expected) = self.menu_foreground
            && !foreground.0.is_null()
            && foreground != expected
        {
            self.close_menu();
            return;
        }
        let escape = key_down(VK_ESCAPE);
        if escape {
            self.close_menu();
            return;
        }
        let Some(cursor) = cursor_position() else {
            return;
        };
        let menu_client = screen_to_client(menu.hwnd, cursor);
        let menu_point = Point::new(menu_client.x as f32, menu_client.y as f32);
        let over_menu = menu.contains(menu_point);
        let client = screen_to_client(self.hwnd, cursor);
        let point = Point::new(client.x as f32, client.y as f32);
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
            DockCommand::ReloadConfig => {
                self.reload_config();
            }
            DockCommand::RefreshItems => {
                self.request_all_icons();
                self.animate();
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
                    self.motion.launch_failed(self.now(), id, &self.metrics);
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

    fn reload_config(&mut self) -> bool {
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
                self.sync_dock_font();
                self.theme = resolve_theme(self.config.appearance.theme);
                self.palette = palette_for(&self.config, self.theme);
                self.refresh_glass_state();
                self.relayout_window(self.window_visible);
                self.request_all_icons();
                self.animate();
                tracing::info!("configuration reloaded");
                true
            }
            Err(error) => {
                self.set_error(format!("重新加载配置失败：{error}"));
                false
            }
        }
    }

    pub(super) fn sync_dock_font(&mut self) {
        let next = self.config.appearance.dock_font;
        if next == self.active_dock_font {
            return;
        }
        self.close_menu_immediately();
        self.tray_menu = None;
        self.renderer
            .set_text_backend(Arc::new(crate::text::NativeText::for_dock_font(next)));
        self.renderer.trim_caches();
        self.active_dock_font = next;
        if next == DockFont::System {
            trim_working_set();
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
            TrayEvent::ReAdd => self.restore_tray_after_taskbar(),
        }
    }

    fn ensure_tray_installed(&mut self) {
        if self.shutdown_done || self.tray.is_none() {
            return;
        }
        if self.tray.as_ref().is_some_and(TrayIcon::is_installed) {
            self.reset_tray_retry();
            return;
        }
        let result = self
            .tray
            .as_mut()
            .expect("tray existence checked")
            .ensure_installed();
        match result {
            Ok(()) => {
                tracing::info!("notification-area icon restored");
                self.reset_tray_retry();
            }
            Err(error) => {
                tracing::warn!(%error, "could not restore notification-area icon");
                if !self.tray_retry_timer {
                    self.schedule_tray_retry();
                }
            }
        }
    }

    fn restore_tray_after_taskbar(&mut self) {
        self.reset_tray_retry();
        let Some(tray) = self.tray.as_mut() else {
            return;
        };
        match tray.reinstall() {
            Ok(()) => tracing::info!("notification-area icon restored after Explorer restart"),
            Err(error) => {
                tracing::warn!(%error, "could not restore notification-area icon after Explorer restart");
                self.schedule_tray_retry();
            }
        }
    }

    fn schedule_tray_retry(&mut self) {
        if self.shutdown_done
            || self.tray_retry_timer
            || self.tray.as_ref().is_none_or(TrayIcon::is_installed)
        {
            return;
        }
        let Some(delay) = tray_retry_delay(self.tray_retry_index) else {
            tracing::warn!(
                "notification-area retries exhausted; waiting for Explorer restart or autostart"
            );
            return;
        };
        let created = unsafe { SetTimer(Some(self.hwnd), TIMER_TRAY_RETRY, delay, None) };
        if created == 0 {
            tracing::warn!(delay, "could not schedule notification-area retry");
            return;
        }
        self.tray_retry_timer = true;
    }

    fn reset_tray_retry(&mut self) {
        if self.tray_retry_timer {
            self.kill_timer(TIMER_TRAY_RETRY);
        }
        self.tray_retry_timer = false;
        self.tray_retry_index = 0;
    }

    fn on_tray_retry(&mut self) {
        self.tray_retry_timer = false;
        if self.shutdown_done || self.tray.is_none() {
            self.tray_retry_index = 0;
            return;
        }
        if self.tray.as_ref().is_some_and(TrayIcon::is_installed) {
            self.reset_tray_retry();
            return;
        }
        self.tray_retry_index += 1;
        self.ensure_tray_installed();
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
            match TrayMenuView::create(self.hwnd) {
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
        let entries = self.tray_root_entries();
        let layout = self.renderer.menu_layout(&entries, self.monitor.dpi_scale);
        let bubble = layout.bubble;
        let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f32;
        let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f32;
        let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1) as f32;
        let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1) as f32;
        let mut screen_position = Point::new(
            cursor.x as f32 - bubble.center().x,
            cursor.y as f32 - bubble.height - 4.0 * self.monitor.dpi_scale,
        );
        screen_position.x = screen_position.x.clamp(
            vx + 2.0,
            (vx + vw - layout.window.width - 2.0).max(vx + 2.0),
        );
        screen_position.y = screen_position.y.clamp(
            vy + 2.0,
            (vy + vh - layout.window.height - 2.0).max(vy + 2.0),
        );

        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        menu.entries = entries;
        menu.layout = layout;
        menu.screen_position = screen_position;
        menu.child = None;
        menu.hovered = None;
        menu.child_hovered = None;
        menu.visible = true;
        let menu_hwnd = menu.hwnd;
        self.render_tray_menu();
        unsafe {
            let _ = SetTimer(Some(menu_hwnd), TIMER_TRAY_MENU, TRAY_MENU_POLL_MS, None);
            let _ = SetCapture(menu_hwnd);
        }
    }

    fn open_tray_child(&mut self, parent: usize) {
        let Some(entries) = self.tray_child_entries(parent) else {
            return;
        };
        let layout = self.renderer.menu_layout(&entries, self.monitor.dpi_scale);
        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        let Some(row) = menu.layout.rows.iter().find(|row| row.entry == parent) else {
            return;
        };
        let root_bubble = menu.layout.bubble;
        let child_bubble = layout.bubble;
        let right = menu.screen_position.x + root_bubble.right() + 4.0;
        let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f32;
        let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f32;
        let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1) as f32;
        let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1) as f32;
        let mut screen_position = Point::new(
            right - child_bubble.left(),
            menu.screen_position.y + row.rect.top() - child_bubble.top(),
        );
        if screen_position.x + layout.window.width > vx + vw - 2.0 {
            screen_position.x = menu.screen_position.x + root_bubble.left()
                - child_bubble.width
                - 4.0
                - child_bubble.left();
        }
        screen_position.x = screen_position.x.clamp(
            vx + 2.0,
            (vx + vw - layout.window.width - 2.0).max(vx + 2.0),
        );
        screen_position.y = screen_position.y.clamp(
            vy + 2.0,
            (vy + vh - layout.window.height - 2.0).max(vy + 2.0),
        );
        menu.child = Some(TraySubmenuView {
            parent,
            entries,
            layout,
            screen_position,
        });
        menu.hovered = Some(parent);
        menu.child_hovered = None;
        self.render_tray_menu();
    }

    fn render_tray_menu(&mut self) {
        let Some(menu) = self.tray_menu.as_ref() else {
            return;
        };
        let Some(root) = self.renderer.render_menu(&MenuScene {
            entries: &menu.entries,
            layout: &menu.layout,
            hovered: menu.hovered,
            palette: self.palette,
        }) else {
            return;
        };
        let child = menu.child.as_ref().and_then(|child| {
            self.renderer
                .render_menu(&MenuScene {
                    entries: &child.entries,
                    layout: &child.layout,
                    hovered: menu.child_hovered,
                    palette: self.palette,
                })
                .map(|frame| (frame, child.screen_position))
        });

        let mut min_x = menu.screen_position.x;
        let mut min_y = menu.screen_position.y;
        let mut max_x = min_x + root.pixmap.width() as f32;
        let mut max_y = min_y + root.pixmap.height() as f32;
        if let Some((ref frame, pos)) = child {
            min_x = min_x.min(pos.x);
            min_y = min_y.min(pos.y);
            max_x = max_x.max(pos.x + frame.pixmap.width() as f32);
            max_y = max_y.max(pos.y + frame.pixmap.height() as f32);
        }
        let origin_x = min_x.floor() as i32;
        let origin_y = min_y.floor() as i32;
        let width = (max_x.ceil() as i32 - origin_x).max(1) as u32;
        let height = (max_y.ceil() as i32 - origin_y).max(1) as u32;

        let Some(menu) = self.tray_menu.as_mut() else {
            return;
        };
        menu.origin = Point::new(origin_x as f32, origin_y as f32);
        menu.width = width;
        menu.height = height;
        let needs_alloc = menu
            .canvas
            .as_ref()
            .is_none_or(|canvas| canvas.width() != width || canvas.height() != height);
        if needs_alloc {
            let Some(new_canvas) = Pixmap::new(width, height) else {
                tracing::warn!("tray menu canvas allocation failed");
                return;
            };
            menu.canvas = Some(new_canvas);
        }
        let Some(canvas) = menu.canvas.as_mut() else {
            return;
        };
        canvas.fill(tiny_skia::Color::TRANSPARENT);
        let paint = PixmapPaint {
            opacity: 1.0,
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: FilterQuality::Nearest,
        };
        canvas.draw_pixmap(
            (menu.screen_position.x - origin_x as f32).round() as i32,
            (menu.screen_position.y - origin_y as f32).round() as i32,
            root.pixmap.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
        if let Some((frame, position)) = child {
            canvas.draw_pixmap(
                (position.x - origin_x as f32).round() as i32,
                (position.y - origin_y as f32).round() as i32,
                frame.pixmap.as_ref(),
                &paint,
                Transform::identity(),
                None,
            );
        }
        unsafe {
            let _ = SetWindowPos(
                menu.hwnd,
                Some(HWND_TOPMOST),
                origin_x,
                origin_y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        if let Err(error) = menu.surface.present(width, height, canvas.data()) {
            tracing::warn!(%error, "could not present tray menu");
        }
    }

    fn close_tray_menu(&mut self) {
        if self.tray_menu.is_none() {
            return;
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        self.tray_menu = None;
        self.renderer.trim_caches();
        trim_working_set();
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
                    } else if (key_down(VK_LBUTTON) || key_down(VK_RBUTTON))
                        && let (Some(cursor), Some(menu)) =
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
        self.animate();
    }

    fn check_startup_privilege_notice(&mut self) {
        let notice = if is_builtin_admin_without_approval() {
            tracing::warn!("built-in Administrator account is running without Admin Approval Mode");
            Some(NoticeKind::BuiltinAdminApproval)
        } else if is_elevated() {
            tracing::warn!("Glide-Dock is running elevated; UIPI will block drag-and-drop");
            Some(NoticeKind::ElevatedWarning)
        } else {
            None
        };
        if let Some(kind) = notice {
            let values = SettingsValues::from_config(&self.config);
            let snapshot = self.updater.snapshot();
            if let Err(error) =
                self.settings
                    .open_notice(self.hwnd, kind, values, self.store.path(), &snapshot)
            {
                tracing::error!(%error, "failed to open privilege notice dialog");
            }
        }
    }

    fn open_settings(&mut self) {
        if self.read_only {
            self.set_error("配置为只读，暂时无法打开设置".to_string());
            return;
        }
        let values = SettingsValues::from_config(&self.config);
        let snapshot = self.updater.snapshot();
        if let Err(error) = self
            .settings
            .open(self.hwnd, values, self.store.path(), &snapshot)
        {
            tracing::error!(%error, "failed to open settings");
            self.set_error(format!("无法打开设置窗口：{error}"));
        }
    }

    fn quit(&self) {
        unsafe { PostQuitMessage(0) };
    }

    /// Saves state and stops timers. Safe to call more than once.
    pub fn shutdown(&mut self) {
        if !self.shutdown_done {
            self.save_now();
            self.shutdown_without_save();
        }
    }

    /// Stops timers without touching the user's configuration. Used by
    /// `--selftest`, which must be side-effect free.
    pub fn shutdown_without_save(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        self.kill_all_timers();
        self.presenter.detach();
        release_mouse();
        self.settings.shutdown();
        if let Some(mut tray) = self.tray.take() {
            tray.remove();
        }
        self.tray_menu = None;
        self.glass = None;
        self.native_glass = false;
        self.window_visible = false;
    }

    // -------------------------------------------------------------- rendering

    pub(crate) fn on_display_changed(&mut self) {
        let monitor = metrics_for_window(self.hwnd);
        if monitor.work == self.monitor.work && monitor.dpi_scale == self.monitor.dpi_scale {
            self.monitor = monitor;
            self.frames.set_refresh_rate(monitor.refresh_hz);
            return;
        }

        // Input state stores client pixels relative to the old geometry.
        if self.state.drag.take().is_some() {
            release_mouse();
        }
        self.on_mouse_leave();
        if self.tray_menu.as_ref().is_some_and(|menu| menu.visible) {
            self.close_tray_menu();
        }
        self.tray_menu = None;
        self.relayout_for_monitor(monitor, self.window_visible);
    }

    fn relayout_window(&mut self, show: bool) {
        self.relayout_for_monitor(metrics_for_window(self.hwnd), show);
    }

    fn relayout_for_monitor(&mut self, monitor: MonitorMetrics, show: bool) {
        self.monitor = monitor;
        self.frames.set_refresh_rate(monitor.refresh_hz);
        let mut metrics = Metrics::new(
            &self.config.appearance,
            &self.config.behavior,
            &self.config.motion,
            self.monitor.dpi_scale,
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
        let margin = self.config.appearance.edge_margin * self.monitor.dpi_scale;
        let offset = self.config.monitor.offset * self.monitor.dpi_scale;
        let (x, y) = place_dock(monitor.work, width, height, margin, offset);
        tracing::info!(
            dpi = self.monitor.dpi_scale,
            refresh_hz = monitor.refresh_hz,
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
        self.apply_dock_layer();
        self.update_native_glass();
        if self.window_visible
            && !self.native_glass
            && !self.glass_failed
            && self.config.appearance.frosted_glass
        {
            self.ensure_native_glass();
        }
        self.icons.set_capacity(self.config.items.len().max(16));
        let needed_icon_size = Self::desired_icon_size(&self.metrics);
        if needed_icon_size > self.requested_icon_size {
            self.requested_icon_size = needed_icon_size;
            self.request_all_icons();
        }
        self.close_menu_immediately();
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
                self.glass = Some(glass);
                self.native_glass = true;
                self.place_glass_below_dock();
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
    fn update_motion_layout(&mut self, dt: f32) -> (MotionSnapshot, FrameLayout) {
        let now = self.now();
        let snapshot = self.motion.update(
            now,
            dt,
            &self.config.items,
            &self.geometry,
            &self.metrics,
            &self.state.running,
        );
        let mut layout =
            FrameLayout::compute(&self.geometry, &self.metrics, &snapshot.layout_scales);
        if self.native_glass {
            layout.dock = self.geometry.dock_base;
        }
        (snapshot, layout)
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

    fn drag_visual(&self, layout: &FrameLayout) -> Option<DragVisual> {
        let drag = self.state.drag.as_ref()?;
        if !drag.active {
            return None;
        }
        let remaining: Vec<Rect> = layout
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

    fn prepare_frame(&mut self, snapshot: &MotionSnapshot, layout: &FrameLayout) -> DockFrame {
        let drag = self.drag_visual(layout);
        let animations = Self::icon_animations(snapshot);
        let scene = DockScene {
            config: &self.config,
            metrics: &self.metrics,
            geometry: &self.geometry,
            layout,
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
        self.renderer.prepare_frame(&scene, None)
    }

    fn present(&mut self, snapshot: &MotionSnapshot, layout: FrameLayout) -> Submission {
        let frame = self.prepare_frame(snapshot, &layout);
        let result = self.presenter.present(
            &mut self.renderer,
            &frame,
            self.config.appearance.gpu_acceleration,
        );
        if result == Submission::Presented {
            self.layout = layout;
            self.hit_shapes = frame.hit_shapes;
        }
        self.present_menu();
        result
    }

    fn present_menu(&mut self) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        let opacity = self.menu_spring.value().clamp(0.0, 1.0);
        let alpha = (opacity * 255.0).round() as u8;
        let offset = ((1.0 - opacity) * 9.0 * self.monitor.dpi_scale).round() as i32;
        if offset != menu.last_offset {
            menu.last_offset = offset;
            unsafe {
                let _ = SetWindowPos(
                    menu.hwnd,
                    None,
                    menu.screen_origin.x,
                    menu.screen_origin.y + offset,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
                );
            }
        }
        if menu.dirty || menu.last_alpha != alpha {
            if let Err(error) = menu.surface.present_alpha(
                menu.pixmap.width(),
                menu.pixmap.height(),
                menu.pixmap.data(),
                alpha,
            ) {
                tracing::warn!(%error, "could not present menu");
            }
            menu.dirty = false;
            menu.last_alpha = alpha;
        }
    }

    pub fn frame_wait_handle(&self) -> Option<HANDLE> {
        self.frames.wait_handle()
    }

    pub fn use_frame_timer_fallback(&mut self) {
        self.frames.use_fallback();
    }

    pub fn on_frame(&mut self) {
        if let Some(stats) = &mut self.frame_stats {
            stats.begin_frame();
        }
        let delta = self.frames.begin_frame(Instant::now());
        let (snapshot, layout) = self.update_motion_layout(delta);
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

        let submission = self.present(&snapshot, layout);
        if let Some(stats) = &mut self.frame_stats {
            stats.record(submission);
        }
        if animating || submission == Submission::Busy {
            self.frames.continue_frames();
        } else {
            self.stop_frames();
        }
    }

    fn stop_frames(&mut self) {
        self.frames.stop();
        self.renderer.trim_caches();
        trim_working_set();
        if let Some(stats) = &mut self.frame_stats {
            stats.finish(self.presenter.gpu_active());
        }
    }

    fn animate(&mut self) {
        self.frames.start();
    }

    fn kill_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    fn kill_all_timers(&mut self) {
        self.stop_frames();
        for id in [
            TIMER_TOOLTIP,
            TIMER_SAVE,
            TIMER_ERROR,
            TIMER_MENU,
            TIMER_RUNNING,
            TIMER_TRAY_RETRY,
        ] {
            self.kill_timer(id);
        }
        self.tooltip_timer = false;
        self.save_timer = false;
        self.error_timer = false;
        self.running_timer = false;
        self.tray_retry_timer = false;
        self.tray_retry_index = 0;
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_FRAME => {
                if self.frames.fallback_active() {
                    self.on_frame();
                }
            }
            TIMER_TOOLTIP => {
                self.kill_timer(TIMER_TOOLTIP);
                self.tooltip_timer = false;
                if !self.state.menu_open
                    && self.config.behavior.show_tooltips
                    && let Some(index) = self.state.hovered
                {
                    self.state.tooltip.index = Some(index);
                    self.tooltip_spring.set_target(1.0);
                    self.animate();
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
            TIMER_TRAY_RETRY => self.on_tray_retry(),
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
            self.animate();
        }
    }

    fn desired_icon_size(metrics: &Metrics) -> u32 {
        (metrics.icon * metrics.max_factor)
            .ceil()
            .clamp(64.0, 256.0) as u32
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
            if let Some(shortcut) = &item.shortcut_path
                && !shortcut.trim().is_empty()
            {
                paths.push(shortcut.clone());
            }
            if !item.target_path.trim().is_empty() {
                paths.push(item.target_path.clone());
            }
        }
        if paths.is_empty() {
            return;
        }
        self.icon_service.request(
            item.id,
            paths,
            self.requested_icon_size,
            self.icon_generation,
        );
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
            self.animate();
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
            self.animate();
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
    }

    fn hit_test(&self, lparam: LPARAM) -> i32 {
        let screen = screen_point(lparam);
        let client = screen_to_client(self.hwnd, screen);
        let point = Point::new(client.x as f32, client.y as f32);
        if self.hit_shapes.iter().any(|shape| shape.contains(point)) {
            return HTCLIENT as i32;
        }
        HTTRANSPARENT
    }

    /// Runs a lightweight self test: renders one frame and checks the idle
    /// timer invariant. It writes nothing, including the configuration.
    pub fn selftest(&mut self) -> bool {
        let (snapshot, layout) = self.update_motion_layout(0.0);
        let frame = self.prepare_frame(&snapshot, &layout);
        let Some(pixmap) = self.renderer.render_frame(&frame) else {
            tracing::error!("selftest: renderer produced no pixmap");
            return false;
        };
        let timers_idle = !self.frames.active()
            && !self.tooltip_timer
            && !self.save_timer
            && !self.error_timer
            && !self.running_timer
            && !self.tray_retry_timer
            && !self.state.menu_open;
        let gpu_requested = self.config.appearance.gpu_acceleration;
        let mut submitted = self
            .presenter
            .present(&mut self.renderer, &frame, gpu_requested);
        if submitted == Submission::Busy {
            submitted = self
                .presenter
                .present(&mut self.renderer, &frame, gpu_requested);
        }
        let ok = timers_idle
            && submitted == Submission::Presented
            && !snapshot.active
            && pixmap.width() > 0
            && pixmap.height() > 0
            && pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0)
            && layout.icons.len() == self.config.items.len();
        tracing::info!(
            items = self.config.items.len(),
            width = pixmap.width(),
            height = pixmap.height(),
            dpi = self.monitor.dpi_scale,
            gpu = self.presenter.gpu_active(),
            ?submitted,
            frame_timer = self.frames.active(),
            tooltip_timer = self.tooltip_timer,
            save_timer = self.save_timer,
            error_timer = self.error_timer,
            running_timer = self.running_timer,
            tray_retry_timer = self.tray_retry_timer,
            menu_open = self.state.menu_open,
            "selftest finished"
        );
        ok
    }
}

fn tray_retry_delay(index: usize) -> Option<u32> {
    TRAY_RETRY_DELAYS_MS.get(index).copied()
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
    use super::{TRAY_RETRY_DELAYS_MS, target_matches, tray_retry_delay};

    #[test]
    fn tray_retries_are_bounded_to_one_minute() {
        assert_eq!(
            (0..TRAY_RETRY_DELAYS_MS.len())
                .map(|index| tray_retry_delay(index).expect("scheduled delay"))
                .sum::<u32>(),
            60_000
        );
        assert_eq!(tray_retry_delay(TRAY_RETRY_DELAYS_MS.len()), None);
    }

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
