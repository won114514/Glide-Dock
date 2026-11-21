use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use dock_core::layout::Metrics;
use dock_core::models::{
    Appearance, Behavior, DockConfig, DockLayer, HoverPreset, LaunchPreset, MonitorConfig,
    MotionConfig,
};
use dock_core::theme::{AccentColor, ThemeMode};
use dock_platform_windows::{
    ComGuard, SingleInstance, SingleInstanceOutcome, is_elevated, is_low_integrity,
    metrics_for_window, open_path, prefers_light_theme,
    registry::Key,
    startup::{self, StartupMode, StartupState},
};
use slint::winit_030::WinitWindowAccessor;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetShellWindow, GetWindowThreadProcessId, PostMessageW, WM_APP,
};

use super::{DockApp, palette_for, resolve_theme};

slint::include_modules!();

pub(super) const WM_APP_SETTINGS_COMMAND: u32 = WM_APP + 7;
const GUARD_MUTEX_NAME: &str = r"Local\Glide-Dock.Guard";
const UAC_REGISTRY_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
const APP_COMPAT_REGISTRY_KEY: &str =
    r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct SettingsValues {
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub motion: MotionConfig,
    pub monitor: MonitorConfig,
}

impl SettingsValues {
    pub(super) fn from_config(config: &DockConfig) -> Self {
        Self {
            appearance: config.appearance.clone(),
            behavior: config.behavior.clone(),
            motion: config.motion.clone(),
            monitor: config.monitor.clone(),
        }
    }

    fn normalize(&mut self) {
        self.appearance.normalize();
        self.behavior.normalize();
        self.motion.normalize();
        self.monitor.normalize();
    }
}

pub(super) enum SettingsCommand {
    Apply(SettingsValues),
    OpenConfigDirectory,
    OpenLogDirectory,
    Reset,
    Reload,
    SetStartup { enabled: bool, mode: StartupMode },
    SetStartupMode(StartupMode),
    SetFont(String),
    RefreshDiagnostics,
    CopyDiagnostics,
    CopyMotionConfig,
    LaunchFailed(String),
}

#[derive(Default)]
pub(super) struct SettingsUpdate {
    status: Option<String>,
    diagnostics: Option<String>,
    values: Option<SettingsValues>,
    startup: Option<(bool, StartupMode)>,
    font_id: Option<String>,
    system_dark: Option<bool>,
    show: bool,
}

#[derive(Default)]
pub(super) struct SettingsHost {
    ui: Option<SettingsUi>,
}

struct SettingsUi {
    thread: JoinHandle<()>,
    commands: Receiver<SettingsCommand>,
    dispatch: Arc<Mutex<SettingsDispatch>>,
    values: Arc<Mutex<SettingsValues>>,
}

#[derive(Default)]
struct SettingsDispatch {
    window: Option<slint::Weak<SettingsWindow>>,
    pending: Vec<SettingsUpdate>,
    stopping: bool,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SettingsLaunchError {
    #[error("无法启动设置窗口：{0}")]
    Spawn(#[from] std::io::Error),
    #[error("设置窗口已停止，请重新启动 Glide-Dock")]
    ThreadExited,
}

impl SettingsUi {
    fn post(&self, update: SettingsUpdate) -> Result<(), SettingsLaunchError> {
        if self.thread.is_finished() {
            return Err(SettingsLaunchError::ThreadExited);
        }
        if let Some(values) = update.values.as_ref() {
            *self.values.lock().unwrap() = values.clone();
        }
        let mut dispatch = self.dispatch.lock().unwrap();
        if let Some(window) = dispatch.window.as_ref() {
            window
                .upgrade_in_event_loop(move |window| apply_update(&window, update))
                .map_err(|_| SettingsLaunchError::ThreadExited)
        } else {
            dispatch.pending.push(update);
            Ok(())
        }
    }
}

impl SettingsHost {
    pub(super) fn open(
        &mut self,
        owner: HWND,
        values: SettingsValues,
        config_path: &Path,
    ) -> Result<(), SettingsLaunchError> {
        if let Some(ui) = self.ui.as_ref() {
            return ui.post(SettingsUpdate {
                values: Some(values),
                show: true,
                ..Default::default()
            });
        }

        let (commands_tx, commands_rx) = mpsc::channel();
        let dispatch = Arc::new(Mutex::new(SettingsDispatch::default()));
        let ui_dispatch = Arc::clone(&dispatch);
        let ui_values = Arc::new(Mutex::new(values.clone()));
        let callback_values = Arc::clone(&ui_values);
        let config_directory = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let window_owner = owner.0 as isize;
        let thread = thread::Builder::new()
            .name("glide-settings-ui".to_string())
            .spawn(move || {
                if let Err(error) = run_settings_window(
                    window_owner,
                    values,
                    config_directory,
                    &commands_tx,
                    &ui_dispatch,
                    callback_values,
                ) {
                    tracing::error!(%error, "settings UI failed");
                    notify_dock(
                        window_owner,
                        &commands_tx,
                        SettingsCommand::LaunchFailed(error.to_string()),
                    );
                }
            })?;
        self.ui = Some(SettingsUi {
            thread,
            commands: commands_rx,
            dispatch,
            values: ui_values,
        });
        Ok(())
    }

    pub(super) fn drain(&self) -> Vec<SettingsCommand> {
        self.ui
            .as_ref()
            .map(|ui| ui.commands.try_iter().collect())
            .unwrap_or_default()
    }

    pub(super) fn shutdown(&mut self) {
        let Some(ui) = self.ui.take() else {
            return;
        };
        let window = {
            let mut dispatch = ui.dispatch.lock().unwrap();
            dispatch.stopping = true;
            dispatch.window.take()
        };
        if let Some(window) = window {
            let _ = window.upgrade_in_event_loop(|window| {
                if let Err(error) = window.hide() {
                    tracing::debug!(%error, "settings window was already hidden during shutdown");
                }
                let _ = slint::quit_event_loop();
            });
        }
        if let Err(payload) = ui.thread.join() {
            tracing::warn!(?payload, "settings UI thread exited with a panic");
        }
    }

    pub(super) fn sync_system_theme(&self) {
        self.update(SettingsUpdate {
            system_dark: Some(!prefers_light_theme()),
            ..Default::default()
        });
    }

    pub(super) fn update(&self, update: SettingsUpdate) {
        if let Some(ui) = self.ui.as_ref()
            && let Err(error) = ui.post(update)
        {
            tracing::debug!(%error, "could not update settings window");
        }
    }
}

fn run_settings_window(
    owner: isize,
    initial: SettingsValues,
    config_directory: PathBuf,
    commands: &Sender<SettingsCommand>,
    dispatch: &Mutex<SettingsDispatch>,
    latest_values: Arc<Mutex<SettingsValues>>,
) -> Result<(), slint::PlatformError> {
    let _com = ComGuard::sta();
    let startup_state = startup::query(&config_directory).unwrap_or_else(|error| {
        tracing::warn!(%error, "could not read startup registrations");
        StartupState {
            enabled: false,
            preferred: StartupMode::Task,
            executable: None,
        }
    });
    let font_id = register_settings_font(&read_font_preference(&config_directory));
    let window = SettingsWindow::new()?;
    populate_window(&window, &initial, &startup_state, &font_id);
    // Size in logical pixels, then center inside the owner's monitor work area.
    // Fixed 980x720 DIP windows otherwise extend behind the taskbar at high DPI.
    let monitor = metrics_for_window(HWND(owner as *mut _));
    let available_width = (monitor.work.width() as f32 / monitor.dpi_scale - 32.0).max(1.0);
    let available_height = (monitor.work.height() as f32 / monitor.dpi_scale - 32.0).max(1.0);
    window.set_available_width(available_width);
    window.set_available_height(available_height);
    let size = slint::LogicalSize::new(
        980.0_f32.min(available_width),
        720.0_f32.min(available_height),
    );
    window.window().set_size(size);
    window.window().set_position(slint::PhysicalPosition::new(
        monitor.work.left
            + ((monitor.work.width() as f32 - size.width * monitor.dpi_scale) / 2.0) as i32,
        monitor.work.top
            + ((monitor.work.height() as f32 - size.height * monitor.dpi_scale) / 2.0) as i32,
    ));
    window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    let weak = window.as_weak();
    let changes_weak = weak.clone();
    let changes = commands.clone();
    let callback_values = latest_values;
    window.on_settings_changed(move || {
        let Some(window) = changes_weak.upgrade() else {
            return;
        };
        let values = {
            let mut latest = callback_values.lock().unwrap();
            let values = values_from_window(&window, &latest);
            *latest = values.clone();
            values
        };
        notify_dock(owner, &changes, SettingsCommand::Apply(values));
    });

    let actions = commands.clone();
    let controls = weak.clone();
    window.on_action(move |action| {
        match action.as_str() {
            "window-minimize" => {
                if let Some(window) = controls.upgrade() {
                    window
                        .window()
                        .with_winit_window(|native| native.set_minimized(true));
                }
                return;
            }
            "window-toggle-maximize" => {
                if let Some(window) = controls.upgrade() {
                    window.window().with_winit_window(|native| {
                        native.set_maximized(!native.is_maximized());
                    });
                }
                return;
            }
            "window-close" => {
                if let Some(window) = controls.upgrade()
                    && let Err(error) = window.hide()
                {
                    tracing::debug!(%error, "could not hide the settings window");
                }
                return;
            }
            _ => {}
        }
        let command = match action.as_str() {
            "open-config" => SettingsCommand::OpenConfigDirectory,
            "open-logs" => SettingsCommand::OpenLogDirectory,
            "reset" => SettingsCommand::Reset,
            "reload" => SettingsCommand::Reload,
            "startup-enable" => SettingsCommand::SetStartup {
                enabled: true,
                mode: startup_mode(&weak),
            },
            "startup-disable" => SettingsCommand::SetStartup {
                enabled: false,
                mode: startup_mode(&weak),
            },
            "startup-update" => SettingsCommand::SetStartup {
                enabled: true,
                mode: startup_mode(&weak),
            },
            "startup-mode" => SettingsCommand::SetStartupMode(startup_mode(&weak)),
            "font-changed" => {
                let font_id = weak
                    .upgrade()
                    .map(|window| window.get_font_id().to_string())
                    .unwrap_or_else(|| "system".into());
                SettingsCommand::SetFont(font_id)
            }
            "refresh-diagnostics" => SettingsCommand::RefreshDiagnostics,
            "copy-diagnostics" => SettingsCommand::CopyDiagnostics,
            "copy-motion" => SettingsCommand::CopyMotionConfig,
            _ => return,
        };
        notify_dock(owner, &actions, command);
    });

    let pending = {
        let mut dispatch = dispatch.lock().unwrap();
        if dispatch.stopping {
            return Ok(());
        }
        dispatch.window = Some(window.as_weak());
        std::mem::take(&mut dispatch.pending)
    };
    for mut update in pending {
        update.show = false;
        apply_update(&window, update);
    }
    window.show()?;
    // Slint owns its platform on this thread for the life of the Dock.
    slint::run_event_loop_until_quit()
}

fn notify_dock(owner: isize, commands: &Sender<SettingsCommand>, command: SettingsCommand) {
    if commands.send(command).is_ok() {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(owner as *mut _)),
                WM_APP_SETTINGS_COMMAND,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

fn apply_update(window: &SettingsWindow, update: SettingsUpdate) {
    if let Some(dark) = update.system_dark {
        window.set_system_dark(dark);
    }
    if let Some(status) = update.status {
        window.set_status_message(status.into());
    }
    if let Some(diagnostics) = update.diagnostics {
        window.set_diagnostics(diagnostics.into());
    }
    if let Some((enabled, mode)) = update.startup {
        window.set_startup_enabled(enabled);
        window.set_startup_mode(startup_mode_id(mode).into());
    }
    if let Some(font_id) = update.font_id {
        window.set_font_id(font_id.into());
    }
    let visible = window.window().is_visible();
    if let Some(values) = update.values {
        populate_values(window, &values);
    }
    if update.show {
        if !visible && let Err(error) = window.show() {
            tracing::error!(%error, "could not show the settings window");
            return;
        }
        window.window().with_winit_window(|native| {
            if native.is_minimized() == Some(true) {
                native.set_minimized(false);
            }
            native.focus_window();
        });
    }
}

fn populate_window(
    window: &SettingsWindow,
    values: &SettingsValues,
    startup: &StartupState,
    font_id: &str,
) {
    window.set_app_version(env!("CARGO_PKG_VERSION").into());
    populate_values(window, values);
    window.set_startup_enabled(startup.enabled);
    window.set_startup_mode(startup_mode_id(startup.preferred).into());
    window.set_font_id(font_id.into());
    window.set_status_message("设置已连接到 Glide-Dock 主程序。".into());
    window.set_diagnostics("选择“刷新诊断”以查看运行信息。".into());
}

fn populate_values(window: &SettingsWindow, values: &SettingsValues) {
    let appearance = &values.appearance;
    let behavior = &values.behavior;
    let motion = &values.motion;
    window.set_system_dark(!prefers_light_theme());
    window.set_theme(theme_id(appearance.theme).into());
    window.set_accent(accent_id(appearance.accent).into());
    window.set_layer(layer_id(appearance.layer).into());
    window.set_icon_size(appearance.icon_size);
    window.set_spacing(appearance.spacing);
    window.set_dock_padding(appearance.padding);
    window.set_corner_radius(appearance.corner_radius);
    window.set_dock_opacity(appearance.opacity * 100.0);
    window.set_frosted_glass(appearance.frosted_glass);
    window.set_edge_margin(appearance.edge_margin);
    window.set_monitor_offset(values.monitor.offset);
    window.set_magnification(behavior.magnification);
    window.set_launch_animation(behavior.launch_animation);
    window.set_show_tooltips(behavior.show_tooltips);
    window.set_show_running_indicator(behavior.show_running_indicator);
    window.set_motion_enabled(motion.enabled);
    window.set_respect_reduced_motion(motion.respect_reduced_motion);
    window.set_hover_preset(motion.hover_preset.id().into());
    window.set_launch_preset(motion.launch_preset.id().into());
    window.set_max_scale(motion.max_scale);
    window.set_range_slots(motion.range_slots);
    window.set_hover_ms(motion.hover_ms as i32);
    window.set_elasticity(motion.elasticity * 100.0);
}

fn values_from_window(window: &SettingsWindow, previous: &SettingsValues) -> SettingsValues {
    let mut values = previous.clone();
    values.appearance.theme = match window.get_theme().as_str() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::System,
    };
    values.appearance.accent = match window.get_accent().as_str() {
        "teal" => AccentColor::Teal,
        "violet" => AccentColor::Violet,
        "orange" => AccentColor::Orange,
        "pink" => AccentColor::Pink,
        _ => AccentColor::Blue,
    };
    values.appearance.layer = match window.get_layer().as_str() {
        "dynamic" => DockLayer::Dynamic,
        "topmost" => DockLayer::Topmost,
        _ => DockLayer::Desktop,
    };
    values.appearance.icon_size = window.get_icon_size();
    values.appearance.spacing = window.get_spacing();
    values.appearance.padding = window.get_dock_padding();
    values.appearance.corner_radius = window.get_corner_radius();
    values.appearance.opacity = window.get_dock_opacity() / 100.0;
    values.appearance.frosted_glass = window.get_frosted_glass();
    values.appearance.edge_margin = window.get_edge_margin();
    values.behavior.magnification = window.get_magnification();
    values.behavior.launch_animation = window.get_launch_animation();
    values.behavior.show_tooltips = window.get_show_tooltips();
    values.behavior.show_running_indicator = window.get_show_running_indicator();
    values.motion.enabled = window.get_motion_enabled();
    values.motion.respect_reduced_motion = window.get_respect_reduced_motion();
    values.motion.hover_preset =
        HoverPreset::from_id(window.get_hover_preset().as_str()).unwrap_or_default();
    values.motion.launch_preset =
        LaunchPreset::from_id(window.get_launch_preset().as_str()).unwrap_or_default();
    values.motion.max_scale = window.get_max_scale();
    values.motion.range_slots = window.get_range_slots();
    values.motion.hover_ms = window.get_hover_ms().max(0) as u32;
    values.motion.elasticity = window.get_elasticity() / 100.0;
    values.monitor.offset = window.get_monitor_offset();
    values.normalize();
    values
}

fn theme_id(theme: ThemeMode) -> &'static str {
    match theme {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

fn accent_id(accent: AccentColor) -> &'static str {
    match accent {
        AccentColor::Blue => "blue",
        AccentColor::Teal => "teal",
        AccentColor::Violet => "violet",
        AccentColor::Orange => "orange",
        AccentColor::Pink => "pink",
    }
}

fn layer_id(layer: DockLayer) -> &'static str {
    match layer {
        DockLayer::Desktop => "desktop",
        DockLayer::Dynamic => "dynamic",
        DockLayer::Topmost => "topmost",
    }
}

fn startup_mode(weak: &slint::Weak<SettingsWindow>) -> StartupMode {
    weak.upgrade()
        .map(|window| StartupMode::parse(&window.get_startup_mode()))
        .unwrap_or(StartupMode::Task)
}

fn startup_mode_id(mode: StartupMode) -> &'static str {
    match mode {
        StartupMode::Task => "task",
        StartupMode::Run => "run",
    }
}

fn startup_mode_label(mode: StartupMode) -> &'static str {
    match mode {
        StartupMode::Task => "登录任务",
        StartupMode::Run => "兼容启动",
    }
}

fn font_preference_path(directory: &Path) -> std::path::PathBuf {
    directory.join("settings-ui.json")
}

fn read_font_preference(directory: &Path) -> String {
    std::fs::read(font_preference_path(directory))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("FontId")
                .or_else(|| value.get("fontId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|font| font == "system" || font == "noto-sans-sc")
        .unwrap_or_else(|| "noto-sans-sc".into())
}

fn write_font_preference(directory: &Path, font_id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let path = font_preference_path(directory);
    let temporary = path.with_extension("json.tmp");
    let mut value = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let object = value
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("settings preference is not an object"))?;
    object.insert("Version".into(), serde_json::json!(1));
    object.insert(
        "FontId".into(),
        serde_json::json!(if font_id == "system" {
            "system"
        } else {
            "noto-sans-sc"
        }),
    );
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    std::fs::write(&temporary, bytes)?;
    match std::fs::rename(&temporary, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn register_settings_font(preferred: &str) -> String {
    static FONT_REGISTERED: OnceLock<bool> = OnceLock::new();
    let available = *FONT_REGISTERED.get_or_init(|| {
        let executable_font = std::env::current_exe().ok()
            .and_then(|path| path.parent().map(|parent| parent.join("assets/fonts/NotoSansSC-VF.ttf")));
        let source_font = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(r"..\settings-app\Assets\Fonts\NotoSansSC-VF.ttf");
        let path = executable_font.filter(|path| path.is_file()).unwrap_or(source_font);
        let Ok(bytes) = std::fs::read(&path) else {
            tracing::warn!(path = %path.display(), "bundled settings font is unavailable");
            return false;
        };
        let mut collection = slint::fontique_012::fontique::Collection::new(
            slint::fontique_012::fontique::CollectionOptions { shared: true, system_fonts: true },
        );
        let fonts = collection.register_fonts(
            slint::fontique_012::fontique::Blob::new(std::sync::Arc::new(bytes)), None,
        );
        if fonts.is_empty() {
            tracing::warn!(path = %path.display(), "bundled settings font contains no usable faces");
            return false;
        }
        if let Err(error) = slint::fontique_012::set_shared_collection(collection) {
            tracing::warn!(%error, "could not register the bundled settings font");
            return false;
        }
        true
    });
    if preferred == "noto-sans-sc" && available {
        "noto-sans-sc".into()
    } else {
        "system".into()
    }
}

fn process_memory_bytes() -> Option<usize> {
    let mut counters = windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        windows::Win32::System::ProcessStatus::GetProcessMemoryInfo(
            windows::Win32::System::Threading::GetCurrentProcess(),
            &mut counters,
            size_of_val(&counters) as u32,
        )
        .ok()?;
    }
    Some(counters.WorkingSetSize)
}

fn copy_diagnostics(owner: HWND, text: &str) -> std::io::Result<()> {
    set_clipboard_text(owner, text)
}

struct ClipboardSession;

impl ClipboardSession {
    fn open(owner: HWND) -> std::io::Result<Self> {
        unsafe { OpenClipboard(Some(owner)) }.map_err(std::io::Error::other)?;
        Ok(Self)
    }
}

impl Drop for ClipboardSession {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

struct ClipboardMemory(Option<HGLOBAL>);

impl Drop for ClipboardMemory {
    fn drop(&mut self) {
        if let Some(memory) = self.0.take() {
            unsafe {
                let _ = windows::Win32::Foundation::GlobalFree(Some(memory));
            }
        }
    }
}

fn set_clipboard_text(owner: HWND, text: &str) -> std::io::Result<()> {
    let _clipboard = ClipboardSession::open(owner)?;
    unsafe { EmptyClipboard() }.map_err(std::io::Error::other)?;

    let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, text.len() * std::mem::size_of::<u16>()) }
        .map_err(std::io::Error::other)?;
    let mut memory = ClipboardMemory(Some(memory));
    let handle = memory.0.expect("clipboard memory is owned");
    let buffer = unsafe { GlobalLock(handle) };
    if buffer.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            text.as_ptr().cast::<u8>(),
            buffer.cast(),
            text.len() * std::mem::size_of::<u16>(),
        );
        let _ = GlobalUnlock(handle);
    }
    unsafe { SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(handle.0))) }
        .map_err(std::io::Error::other)?;
    memory.0 = None;
    Ok(())
}

impl DockApp {
    pub(super) fn on_settings_commands(&mut self) {
        for command in self.settings.drain() {
            match command {
                SettingsCommand::Apply(values) => self.apply_settings(values),
                SettingsCommand::OpenConfigDirectory => self.open_config_dir(),
                SettingsCommand::OpenLogDirectory => {
                    let directory = dock_storage::log_dir();
                    if let Err(error) = std::fs::create_dir_all(&directory) {
                        self.set_error(format!("无法创建日志目录：{error}"));
                    } else if let Err(error) = open_path(&directory.to_string_lossy()) {
                        self.set_error(format!("无法打开日志目录：{error}"));
                    }
                }
                SettingsCommand::Reset => {
                    let mut values = SettingsValues::from_config(&self.config);
                    values.appearance = Appearance::default();
                    values.behavior = Behavior::default();
                    values.motion = MotionConfig::default();
                    values.monitor = MonitorConfig::default();
                    self.apply_settings(values);
                    self.settings.update(SettingsUpdate {
                        values: Some(SettingsValues::from_config(&self.config)),
                        ..Default::default()
                    });
                }
                SettingsCommand::Reload => {
                    if self.reload_config() {
                        self.settings.update(SettingsUpdate {
                            values: Some(SettingsValues::from_config(&self.config)),
                            status: Some("配置已重新加载。".into()),
                            ..Default::default()
                        });
                    } else {
                        self.settings.update(SettingsUpdate {
                            status: Some(
                                "读取配置失败，Dock 保留了原设置。请查看日志了解详情。".into(),
                            ),
                            ..Default::default()
                        });
                    }
                }
                SettingsCommand::SetStartup { enabled, mode } => self.set_startup(enabled, mode),
                SettingsCommand::SetStartupMode(mode) => self.set_startup_mode(mode),
                SettingsCommand::SetFont(font_id) => self.set_settings_font(font_id),
                SettingsCommand::RefreshDiagnostics => {
                    let diagnostics = self.settings_diagnostics();
                    self.settings.update(SettingsUpdate {
                        diagnostics: Some(diagnostics),
                        status: Some("诊断信息已更新。".into()),
                        ..Default::default()
                    });
                }
                SettingsCommand::CopyDiagnostics => {
                    let diagnostics = self.settings_diagnostics();
                    match copy_diagnostics(self.hwnd, &diagnostics) {
                        Ok(()) => self.settings.update(SettingsUpdate {
                            status: Some("诊断信息已复制到剪贴板。".into()),
                            ..Default::default()
                        }),
                        Err(error) => self.settings.update(SettingsUpdate {
                            status: Some(format!("复制失败：{error}")),
                            ..Default::default()
                        }),
                    }
                }
                SettingsCommand::CopyMotionConfig => {
                    let result = serde_json::to_string_pretty(&self.config.motion)
                        .map_err(std::io::Error::other)
                        .and_then(|text| copy_diagnostics(self.hwnd, &text));
                    let status = result.map_or_else(
                        |error| format!("复制动画配置失败：{error}"),
                        |_| "动画配置 JSON 已复制到剪贴板。".into(),
                    );
                    self.settings.update(SettingsUpdate {
                        status: Some(status),
                        ..Default::default()
                    });
                }
                SettingsCommand::LaunchFailed(error) => {
                    self.set_error(format!("设置窗口启动失败：{error}"));
                }
            }
        }
    }

    fn apply_settings(&mut self, mut values: SettingsValues) {
        if self.read_only {
            self.set_error("配置为只读，设置不会保存".to_string());
            return;
        }
        values.normalize();
        let previous_metrics = self.metrics;
        let previous_offset = self.config.monitor.offset;
        let previous_glass = self.config.appearance.frosted_glass;
        self.config.appearance = values.appearance;
        self.config.behavior = values.behavior;
        self.config.motion = values.motion;
        self.config.monitor = values.monitor;
        self.apply_motion_config();
        self.theme = resolve_theme(self.config.appearance.theme);
        self.palette = palette_for(&self.config, self.theme);
        self.update_native_glass();
        self.apply_dock_layer();
        let next_metrics = Metrics::new(
            &self.config.appearance,
            &self.config.behavior,
            &self.config.motion,
            self.dpi_scale,
        );
        if self.config.appearance.frosted_glass != previous_glass {
            self.refresh_glass_state();
        }
        if next_metrics != previous_metrics
            || (self.config.monitor.offset - previous_offset).abs() > f32::EPSILON
        {
            self.relayout_window(self.window_visible);
        }
        self.animate();
        self.render();
        self.mark_dirty();
    }

    fn set_startup(&mut self, enabled: bool, mode: StartupMode) {
        let directory = self.store.path().parent().unwrap_or_else(|| Path::new("."));
        let result = if enabled {
            std::env::current_exe().and_then(|exe| startup::enable(mode, &exe, directory))
        } else {
            startup::disable().map(|()| None)
        };
        let state = startup::query(directory);
        let (status, enabled, mode) = match result {
            Ok(Some(warning)) => (
                warning,
                state.as_ref().is_ok_and(|s| s.enabled),
                state.map_or(mode, |s| s.preferred),
            ),
            Ok(None) => (
                if enabled {
                    "已设置开机启动。"
                } else {
                    "已关闭开机启动。"
                }
                .into(),
                state.as_ref().is_ok_and(|s| s.enabled),
                state.map_or(mode, |s| s.preferred),
            ),
            Err(error) => (
                format!("开机启动设置失败：{error}"),
                state.as_ref().is_ok_and(|s| s.enabled),
                state.map_or(mode, |s| s.preferred),
            ),
        };
        self.settings.update(SettingsUpdate {
            status: Some(status),
            startup: Some((enabled, mode)),
            ..Default::default()
        });
    }

    fn set_startup_mode(&mut self, mode: StartupMode) {
        let directory = self.store.path().parent().unwrap_or_else(|| Path::new("."));
        let result = startup::set_preferred_mode(mode, directory);
        let status = result.map_or_else(
            |error| format!("启动方式偏好无法保存：{error}"),
            |_| "启动方式偏好已保存。".into(),
        );
        self.settings.update(SettingsUpdate {
            status: Some(status),
            startup: Some((false, mode)),
            ..Default::default()
        });
    }

    fn set_settings_font(&mut self, font_id: String) {
        let directory = self.store.path().parent().unwrap_or_else(|| Path::new("."));
        match write_font_preference(directory, &font_id) {
            Ok(()) => self.settings.update(SettingsUpdate {
                font_id: Some(font_id),
                status: Some("字体偏好已保存。".into()),
                ..Default::default()
            }),
            Err(error) => self.settings.update(SettingsUpdate {
                status: Some(format!("字体偏好无法保存：{error}")),
                font_id: Some(read_font_preference(directory)),
                ..Default::default()
            }),
        }
    }

    fn settings_diagnostics(&self) -> String {
        let executable = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "不可用".into());
        let config = self.store.path().display().to_string();
        let memory = process_memory_bytes()
            .map(|bytes| format!("{:.1} MiB", bytes as f64 / 1_048_576.0))
            .unwrap_or_else(|| "不可用".into());
        let startup = startup::query(self.store.path().parent().unwrap_or_else(|| Path::new(".")))
            .map(|state| {
                if state.enabled {
                    let executable = state
                        .executable
                        .as_deref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "目标路径未知".into());
                    format!(
                        "已启用（{}；{executable}）",
                        startup_mode_label(state.preferred)
                    )
                } else {
                    "未启用".into()
                }
            })
            .unwrap_or_else(|error| format!("读取失败：{error}"));
        let privilege = if is_low_integrity() {
            "低完整性；拖放和配置写入可能受限"
        } else if is_elevated() {
            "管理员权限；拖入普通用户文件可能被 Windows 拦截"
        } else {
            "普通用户权限"
        };
        let mut explorer_pid = 0;
        unsafe {
            let _ = GetWindowThreadProcessId(GetShellWindow(), Some(&mut explorer_pid));
        }
        let guard = match SingleInstance::acquire_named(GUARD_MUTEX_NAME) {
            SingleInstanceOutcome::Acquired(instance) => {
                drop(instance);
                "未运行"
            }
            SingleInstanceOutcome::AlreadyRunning => "运行中",
            SingleInstanceOutcome::Failed(_) => "无法读取",
        };
        let uac = diagnostic_dword(HKEY_LOCAL_MACHINE, UAC_REGISTRY_KEY, "EnableLUA").map_or_else(
            |error| format!("读取失败：{error}"),
            |value| match value {
                Some(1) => "已开启".into(),
                Some(0) => "已关闭".into(),
                Some(_) => "值异常".into(),
                None => "未配置".into(),
            },
        );
        let compatibility = diagnostic_executable_registry_value();
        let shortcut = diagnostic_start_menu_shortcut();
        let log_directory = dock_storage::log_dir();
        format!(
            "版本：{}\n进程 ID：{}\n内存工作集：{}\n运行权限：{}\nExplorer 进程 ID：{}\n守护进程：{}\nUAC：{}\n开机启动：{}\n开始菜单快捷方式：{}\n兼容性标记：{}\n程序路径：{}\n配置文件：{}\n日志目录：{}\n",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            memory,
            privilege,
            if explorer_pid == 0 {
                "不可用".into()
            } else {
                explorer_pid.to_string()
            },
            guard,
            uac,
            startup,
            shortcut,
            compatibility,
            executable,
            config,
            log_directory.display()
        )
    }
}

fn diagnostic_dword(root: HKEY, path: &str, name: &str) -> std::io::Result<Option<u32>> {
    let Some(key) = Key::open(root, path, KEY_QUERY_VALUE)? else {
        return Ok(None);
    };
    key.dword(name)
}

fn diagnostic_executable_registry_value() -> String {
    let Ok(executable) = std::env::current_exe() else {
        return "无法读取程序路径".into();
    };
    match Key::open(HKEY_CURRENT_USER, APP_COMPAT_REGISTRY_KEY, KEY_QUERY_VALUE).and_then(|key| {
        key.map(|key| key.text(&executable.to_string_lossy()))
            .transpose()
            .map(Option::flatten)
    }) {
        Ok(Some(value)) if !value.trim().is_empty() => value,
        Ok(_) => "未设置".into(),
        Err(error) => format!("读取失败：{error}"),
    }
}

fn diagnostic_start_menu_shortcut() -> String {
    ["APPDATA", "PROGRAMDATA"]
        .into_iter()
        .filter_map(|variable| std::env::var_os(variable).map(PathBuf::from))
        .map(|root| root.join("Microsoft/Windows/Start Menu/Programs/Glide-Dock/Glide-Dock.lnk"))
        .find(|path| path.is_file())
        .map_or_else(|| "未找到".into(), |path| path.display().to_string())
}
