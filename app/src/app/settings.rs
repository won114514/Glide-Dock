use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dock_core::layout::Metrics;
use dock_core::models::{Appearance, Behavior, MonitorConfig, MotionConfig};
use dock_core::settings_ipc::{
    COPYDATA_UI_MAGIC, DockToUiEvent, NoticeKind, NoticeUpdate, SettingsCommand, SettingsUpdate,
    SettingsUpdateView, SettingsValues, StartupChoice, UiBootstrap, UiLaunchMode,
};
use dock_platform_windows::{
    SingleInstance, SingleInstanceOutcome, is_elevated, is_low_integrity, is_window, open_path,
    prefers_light_theme,
    registry::Key,
    send_copydata,
    startup::{self, StartupMode, StartupState},
};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

use super::{DockApp, palette_for, resolve_theme};

const GUARD_MUTEX_NAME: &str = r"Local\Glide-Dock.Guard";
const UAC_REGISTRY_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
const APP_COMPAT_REGISTRY_KEY: &str =
    r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

#[derive(Default)]
pub(super) struct SettingsHost {
    child: Option<Child>,
    ui_hwnd: Option<HWND>,
    pending_events: Vec<DockToUiEvent>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SettingsLaunchError {
    #[error("无法启动设置窗口：{0}")]
    Spawn(#[from] std::io::Error),
    #[error("找不到独立设置程序：{}", .0.display())]
    MissingHelper(PathBuf),
}

fn startup_choice(mode: StartupMode) -> StartupChoice {
    match mode {
        StartupMode::Task => StartupChoice::Task,
        StartupMode::Run => StartupChoice::Run,
    }
}

fn startup_mode_from(choice: StartupChoice) -> StartupMode {
    match choice {
        StartupChoice::Task => StartupMode::Task,
        StartupChoice::Run => StartupMode::Run,
    }
}

pub(super) fn update_view_from_snapshot(
    snapshot: &crate::update::UpdateSnapshot,
) -> SettingsUpdateView {
    SettingsUpdateView {
        managed: snapshot.managed,
        checking: snapshot.checking,
        downloading: snapshot.downloading,
        ready: snapshot.ready,
        launching: snapshot.launching,
        progress: snapshot.progress,
        status: snapshot.status.clone(),
        current_version: snapshot.current_version.clone(),
        latest_version: snapshot.latest_version.clone(),
        release_notes: snapshot.release_notes.clone(),
        source: snapshot.source.clone(),
        cnb_status: snapshot.cnb_status.clone(),
        github_status: snapshot.github_status.clone(),
        last_check_text: snapshot.last_check_text.clone(),
    }
}

impl SettingsHost {
    fn poll_child(&mut self) {
        let exited = match self.child.as_mut() {
            Some(child) => !matches!(child.try_wait(), Ok(None)),
            None => false,
        };
        if exited {
            self.child = None;
            self.ui_hwnd = None;
            self.pending_events.clear();
        }
    }

    fn send_event(&mut self, event: DockToUiEvent) -> bool {
        self.poll_child();
        if self.child.is_none() {
            return false;
        }
        if let Some(hwnd) = self.ui_hwnd {
            if !is_window(hwnd) {
                self.ui_hwnd = None;
                self.child = None;
                self.pending_events.clear();
                return false;
            }
            if let Ok(payload) = serde_json::to_vec(&event) {
                if send_copydata(hwnd, COPYDATA_UI_MAGIC, &payload).is_ok() {
                    return true;
                }
                self.ui_hwnd = None;
                self.child = None;
                self.pending_events.clear();
                return false;
            }
        }
        self.pending_events.push(event);
        true
    }

    fn spawn_helper(
        &mut self,
        owner: HWND,
        mode: UiLaunchMode,
        values: SettingsValues,
        config_path: &Path,
        update_snapshot: &crate::update::UpdateSnapshot,
    ) -> Result<(), SettingsLaunchError> {
        let helper = std::env::current_exe()?.with_file_name("glide-settings.exe");
        if !helper.is_file() {
            return Err(SettingsLaunchError::MissingHelper(helper));
        }
        let config_directory = config_path.parent().unwrap_or_else(|| Path::new("."));
        let startup_state = startup::query(config_directory).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read startup registrations");
            StartupState {
                enabled: false,
                preferred: StartupMode::Task,
                executable: None,
            }
        });
        let boot = UiBootstrap {
            mode,
            dock_hwnd: owner.0 as isize,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            font_id: read_font_preference(config_directory),
            values,
            startup_enabled: startup_state.enabled,
            startup_mode: startup_choice(startup_state.preferred),
            system_dark: !prefers_light_theme(),
            update_view: update_view_from_snapshot(update_snapshot),
        };
        let payload = serde_json::to_vec(&boot).map_err(std::io::Error::other)?;
        let mut child = Command::new(&helper)
            .arg("--dock-hwnd")
            .arg((owner.0 as isize).to_string())
            .stdin(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&payload)?;
        }
        self.child = Some(child);
        self.ui_hwnd = None;
        self.pending_events.clear();
        Ok(())
    }

    pub(super) fn register_ui_window(&mut self, ui_hwnd: HWND) {
        self.poll_child();
        if self.child.is_none() || !is_window(ui_hwnd) {
            return;
        }
        self.ui_hwnd = Some(ui_hwnd);
        let queued = std::mem::take(&mut self.pending_events);
        for event in queued {
            if let Ok(payload) = serde_json::to_vec(&event) {
                let _ = send_copydata(ui_hwnd, COPYDATA_UI_MAGIC, &payload);
            }
        }
    }

    pub(super) fn open(
        &mut self,
        owner: HWND,
        values: SettingsValues,
        config_path: &Path,
        update_snapshot: &crate::update::UpdateSnapshot,
    ) -> Result<(), SettingsLaunchError> {
        let update = SettingsUpdate {
            values: Some(values.clone()),
            update_view: Some(update_view_from_snapshot(update_snapshot)),
            show: true,
            ..Default::default()
        };
        if self.send_event(DockToUiEvent::UpdateSettings(Box::new(update))) {
            return Ok(());
        }
        self.spawn_helper(
            owner,
            UiLaunchMode::Settings,
            values,
            config_path,
            update_snapshot,
        )
    }

    pub(super) fn open_notice(
        &mut self,
        owner: HWND,
        kind: NoticeKind,
        values: SettingsValues,
        config_path: &Path,
        update_snapshot: &crate::update::UpdateSnapshot,
    ) -> Result<(), SettingsLaunchError> {
        let update = NoticeUpdate {
            kind: Some(kind),
            theme: Some(values.appearance.theme),
            system_dark: Some(!prefers_light_theme()),
            feedback: Some(String::new()),
            show: true,
        };
        if self.send_event(DockToUiEvent::UpdateNotice(update)) {
            return Ok(());
        }
        self.spawn_helper(
            owner,
            UiLaunchMode::Notice(kind),
            values,
            config_path,
            update_snapshot,
        )
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(hwnd) = self.ui_hwnd.take()
            && is_window(hwnd)
            && let Ok(payload) = serde_json::to_vec(&DockToUiEvent::CloseUi)
        {
            let _ = send_copydata(hwnd, COPYDATA_UI_MAGIC, &payload);
        }
        self.pending_events.clear();
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_millis(400);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
            }
        }
    }

    pub(super) fn sync_system_theme(&mut self) {
        self.poll_child();
        if self.child.is_none() {
            return;
        }
        let dark = !prefers_light_theme();
        self.update(SettingsUpdate {
            system_dark: Some(dark),
            ..Default::default()
        });
        let _ = self.send_event(DockToUiEvent::UpdateNotice(NoticeUpdate {
            system_dark: Some(dark),
            ..Default::default()
        }));
    }

    pub(super) fn update(&mut self, update: SettingsUpdate) {
        self.poll_child();
        if self.child.is_some() {
            let _ = self.send_event(DockToUiEvent::UpdateSettings(Box::new(update)));
        }
    }

    fn update_notice_feedback(&mut self, feedback: impl Into<String>) {
        self.poll_child();
        if self.child.is_some() {
            let _ = self.send_event(DockToUiEvent::UpdateNotice(NoticeUpdate {
                feedback: Some(feedback.into()),
                ..Default::default()
            }));
        }
    }

    pub(super) fn update_update_view(&mut self, snapshot: &crate::update::UpdateSnapshot) {
        self.poll_child();
        if self.child.is_some() {
            self.update(SettingsUpdate {
                update_view: Some(update_view_from_snapshot(snapshot)),
                ..Default::default()
            });
        }
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
    pub(super) fn on_update_worker_ready(&mut self) {
        let snapshot = self.updater.snapshot();
        self.settings.update_update_view(&snapshot);
    }

    pub(super) fn on_settings_copydata(&mut self, bytes: &[u8]) {
        let Ok(command) = serde_json::from_slice::<SettingsCommand>(bytes) else {
            tracing::warn!("ignoring malformed settings IPC payload");
            return;
        };
        match command {
            SettingsCommand::RegisterUiWindow(raw_hwnd) => {
                self.settings.register_ui_window(HWND(raw_hwnd as *mut _));
            }
            other => {
                self.handle_settings_command(other);
                let snapshot = self.updater.snapshot();
                self.settings.update_update_view(&snapshot);
            }
        }
    }

    fn handle_settings_command(&mut self, command: SettingsCommand) {
        match command {
            SettingsCommand::RegisterUiWindow(_) => {}
            SettingsCommand::Apply(values) => self.apply_settings(*values),
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
            SettingsCommand::SetStartup { enabled, mode } => {
                self.set_startup(enabled, startup_mode_from(mode));
            }
            SettingsCommand::SetStartupMode(mode) => {
                self.set_startup_mode(startup_mode_from(mode));
            }
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
            SettingsCommand::CheckForUpdates => {
                self.updater.check_now();
            }
            SettingsCommand::InstallUpdate => {
                self.updater.launch_update();
            }
            SettingsCommand::OpenCnbReleases => {
                let _ = open_path("https://cnb.cool/won114514/Glide-Dock/-/releases");
            }
            SettingsCommand::OpenGithubReleases => {
                let _ = open_path("https://github.com/won114514/Glide-Dock/releases");
            }
            SettingsCommand::OpenLocalSecurityPolicy => {
                if let Err(secpol_error) = open_path("secpol.msc")
                    && let Err(gpedit_error) = open_path("gpedit.msc")
                {
                    tracing::warn!(
                        %secpol_error,
                        %gpedit_error,
                        "could not open local security or group policy editor"
                    );
                    self.settings.update_notice_feedback(
                        "无法打开 secpol.msc 或 gpedit.msc（Windows 家庭版可参考上方方法二执行命令）。",
                    );
                }
            }
            SettingsCommand::CopyNoticeCommand => {
                match set_clipboard_text(self.hwnd, BUILTIN_ADMIN_COMMAND_TEXT) {
                    Ok(()) => {
                        self.settings.update_notice_feedback("命令已复制到剪贴板。");
                    }
                    Err(error) => {
                        self.settings
                            .update_notice_feedback(format!("复制命令失败：{error}"));
                    }
                }
            }
            SettingsCommand::QuitDock => {
                self.quit();
            }
            SettingsCommand::LaunchFailed(error) => {
                self.set_error(format!("设置窗口启动失败：{error}"));
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
        self.sync_dock_font();
        let next_metrics = Metrics::new(
            &self.config.appearance,
            &self.config.behavior,
            &self.config.motion,
            self.monitor.dpi_scale,
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
            startup: Some((enabled, startup_choice(mode))),
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
            startup: Some((false, startup_choice(mode))),
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
            "版本：{}\n进程 ID：{}\n内存工作集：{}\nDock 字体：{}\n运行权限：{}\nExplorer 进程 ID：{}\n守护进程：{}\nUAC：{}\n开机启动：{}\n开始菜单快捷方式：{}\n兼容性标记：{}\n程序路径：{}\n配置文件：{}\n日志目录：{}\n",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            memory,
            self.active_dock_font.label(),
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

const BUILTIN_ADMIN_COMMAND_TEXT: &str = r#"reg add "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System" /v FilterAdministratorToken /t REG_DWORD /d 1 /f"#;
