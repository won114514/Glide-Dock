//! On-demand Slint UI process for Glide-Dock (`SettingsWindow` and `NoticeDialog`).
//!
//! Keeping Slint, Winit, Parley, ICU, and Noto Sans SC isolated inside this
//! short-lived child process keeps `glide-dock.exe` completely free of UI
//! framework and font overhead while idle, and releases all UI resources back
//! to Windows as soon as the window closes.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use dock_core::models::{DockFont, DockLayer, HoverPreset, LaunchPreset};
use dock_core::settings_ipc::{
    COPYDATA_UI_MAGIC, DockToUiEvent, MAX_COPYDATA_BYTES, NoticeKind, NoticeUpdate,
    SettingsCommand, SettingsUpdate, SettingsUpdateView, SettingsValues, StartupChoice,
    UiBootstrap, UiLaunchMode,
};
use dock_core::theme::{AccentColor, ThemeMode};
use dock_platform_windows::{
    ComGuard, copydata_payload, create_message_window, is_window, metrics_for_window,
    prefers_light_theme, register_class, send_copydata,
};
use slint::winit_030::WinitWindowAccessor;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, MSG, PostQuitMessage, SetTimer,
    TranslateMessage, WM_CLOSE, WM_COPYDATA, WM_DESTROY, WM_TIMER,
};

slint::include_modules!();

const UI_IPC_CLASS: &str = "Glide-Dock.SettingsIpc";
const TIMER_WATCH_DOCK: usize = 1;
const WATCH_DOCK_MS: u32 = 500;

thread_local! {
    static UI_STATE: RefCell<Option<UiAppState>> = const { RefCell::new(None) };
}

static IPC_OWNER_HWND: OnceLock<isize> = OnceLock::new();
static IPC_CLOSE_PENDING: OnceLock<Arc<Mutex<Option<isize>>>> = OnceLock::new();

struct UiAppState {
    dock_hwnd: isize,
    commands: Sender<SettingsCommand>,
    latest_values: Arc<Mutex<SettingsValues>>,
    settings: Option<SettingsWindow>,
    notice: Option<NoticeDialog>,
    settings_open: bool,
    notice_open: bool,
    font_id: String,
    app_version: String,
    startup_enabled: bool,
    startup_mode: StartupChoice,
}

impl UiAppState {
    fn maybe_quit_if_all_closed(&self) {
        if !self.settings_open && !self.notice_open {
            if let Some(slot) = IPC_CLOSE_PENDING.get()
                && let Some(raw_hwnd) = *slot.lock().unwrap()
            {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(HWND(raw_hwnd as *mut _)),
                        WM_CLOSE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            let _ = slint::quit_event_loop();
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--selftest") {
        return run_selftest();
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _com = ComGuard::sta();

    let mut boot_bytes = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut boot_bytes) {
        eprintln!("failed to read bootstrap payload: {error}");
        return ExitCode::FAILURE;
    }
    let boot: UiBootstrap = match serde_json::from_slice(&boot_bytes) {
        Ok(boot) => boot,
        Err(error) => {
            eprintln!("invalid bootstrap payload: {error}");
            return ExitCode::FAILURE;
        }
    };

    let dock_hwnd_arg = parse_dock_hwnd(&args).unwrap_or(boot.dock_hwnd);
    let _ = IPC_OWNER_HWND.set(dock_hwnd_arg);
    let _ = IPC_CLOSE_PENDING.set(Arc::new(Mutex::new(None)));

    let font_id = ensure_font_registered(&boot.font_id);
    let (cmd_tx, cmd_rx) = mpsc::channel::<SettingsCommand>();

    // Off-thread WM_COPYDATA sender so rapid slider drags never block or deadlock UI rendering.
    std::thread::spawn(move || {
        let target = HWND(dock_hwnd_arg as *mut _);
        while let Ok(command) = cmd_rx.recv() {
            let Ok(payload) = serde_json::to_vec(&command) else {
                continue;
            };
            let _ = send_copydata(target, COPYDATA_UI_MAGIC, &payload);
        }
    });

    // Dedicated message-only IPC listener receiving live updates from glide-dock.exe.
    let ipc_commands = cmd_tx.clone();
    std::thread::spawn(move || run_ipc_listener(dock_hwnd_arg, ipc_commands));

    let latest_values = Arc::new(Mutex::new(boot.values.clone()));
    let mut state = UiAppState {
        dock_hwnd: dock_hwnd_arg,
        commands: cmd_tx,
        latest_values,
        settings: None,
        notice: None,
        settings_open: false,
        notice_open: false,
        font_id,
        app_version: boot.app_version.clone(),
        startup_enabled: boot.startup_enabled,
        startup_mode: boot.startup_mode,
    };

    match boot.mode {
        UiLaunchMode::Settings => {
            let Ok(window) = create_settings_window(
                state.dock_hwnd,
                &state.commands,
                &state.latest_values,
                &boot.values,
                &state.app_version,
                state.startup_enabled,
                state.startup_mode,
                &state.font_id,
                boot.system_dark,
                &boot.update_view,
            ) else {
                return ExitCode::FAILURE;
            };
            if show_settings_window(&window) {
                state.settings_open = true;
            }
            state.settings = Some(window);
        }
        UiLaunchMode::Notice(kind) => {
            let Ok(dialog) = create_notice_dialog(
                &state.commands,
                &state.font_id,
                boot.values.appearance.theme,
                boot.system_dark,
            ) else {
                return ExitCode::FAILURE;
            };
            apply_notice_update(
                &dialog,
                state.dock_hwnd,
                NoticeUpdate {
                    kind: Some(kind),
                    theme: Some(boot.values.appearance.theme),
                    system_dark: Some(boot.system_dark),
                    feedback: Some(String::new()),
                    show: true,
                },
            );
            state.notice_open = true;
            state.notice = Some(dialog);
        }
    }

    UI_STATE.with_borrow_mut(|slot| {
        *slot = Some(state);
    });

    if let Err(error) = slint::run_event_loop_until_quit() {
        eprintln!("settings UI event loop error: {error}");
        return ExitCode::FAILURE;
    }
    UI_STATE.with_borrow_mut(|slot| {
        *slot = None;
    });
    ExitCode::SUCCESS
}

fn run_selftest() -> ExitCode {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if SettingsWindow::new().is_err() || NoticeDialog::new().is_err() {
        return ExitCode::FAILURE;
    }
    println!("glide-settings selftest: OK");
    ExitCode::SUCCESS
}

fn parse_dock_hwnd(args: &[String]) -> Option<isize> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--dock-hwnd" {
            return iter.next().and_then(|value| value.parse::<isize>().ok());
        }
    }
    None
}

unsafe extern "system" fn ipc_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            if let Some(bytes) =
                unsafe { copydata_payload(lparam, COPYDATA_UI_MAGIC, MAX_COPYDATA_BYTES) }
                && let Ok(event) = serde_json::from_slice::<DockToUiEvent>(&bytes)
            {
                let _ = slint::invoke_from_event_loop(move || handle_dock_event(event));
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_WATCH_DOCK
                && let Some(&dock_raw) = IPC_OWNER_HWND.get()
                && dock_raw != 0
                && !is_window(HWND(dock_raw as *mut _))
            {
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_WATCH_DOCK);
                    PostQuitMessage(0);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_WATCH_DOCK);
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn run_ipc_listener(dock_hwnd: isize, commands: Sender<SettingsCommand>) {
    let Ok(instance) = register_class(UI_IPC_CLASS, Some(ipc_wndproc)) else {
        return;
    };
    let Ok(ipc_hwnd) = create_message_window(instance, UI_IPC_CLASS, "Glide-Dock Settings IPC")
    else {
        return;
    };
    if let Some(slot) = IPC_CLOSE_PENDING.get() {
        *slot.lock().unwrap() = Some(ipc_hwnd.0 as isize);
    }
    unsafe {
        let _ = SetTimer(Some(ipc_hwnd), TIMER_WATCH_DOCK, WATCH_DOCK_MS, None);
    }
    let _ = commands.send(SettingsCommand::RegisterUiWindow(ipc_hwnd.0 as isize));

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let _ = dock_hwnd;
}

fn handle_dock_event(event: DockToUiEvent) {
    UI_STATE.with_borrow_mut(|slot| {
        let Some(state) = slot.as_mut() else {
            return;
        };
        match event {
            DockToUiEvent::CloseUi => {
                if let Some(window) = state.settings.as_ref() {
                    let _ = window.hide();
                }
                if let Some(dialog) = state.notice.as_ref() {
                    let _ = dialog.hide();
                }
                let _ = slint::quit_event_loop();
            }
            DockToUiEvent::UpdateSettings(update) => {
                let update = *update;
                if let Some(font_id) = update.font_id.as_deref() {
                    state.font_id = ensure_font_registered(font_id);
                }
                if let Some((enabled, mode)) = update.startup {
                    state.startup_enabled = enabled;
                    state.startup_mode = mode;
                }
                if let Some(values) = update.values.as_ref() {
                    *state.latest_values.lock().unwrap() = values.clone();
                }
                if state.settings.is_none() && update.show {
                    let values = update
                        .values
                        .clone()
                        .unwrap_or_else(|| state.latest_values.lock().unwrap().clone());
                    let view = update.update_view.clone().unwrap_or_default();
                    let sys_dark = update.system_dark.unwrap_or_else(|| !prefers_light_theme());
                    if let Ok(window) = create_settings_window(
                        state.dock_hwnd,
                        &state.commands,
                        &state.latest_values,
                        &values,
                        &state.app_version,
                        state.startup_enabled,
                        state.startup_mode,
                        &state.font_id,
                        sys_dark,
                        &view,
                    ) {
                        state.settings = Some(window);
                    }
                }
                if let Some(window) = state.settings.as_ref()
                    && apply_settings_update(window, update)
                {
                    state.settings_open = true;
                }
            }
            DockToUiEvent::UpdateNotice(update) => {
                if state.notice.is_none() && update.show {
                    let theme = update.theme.unwrap_or(ThemeMode::System);
                    let sys_dark = update.system_dark.unwrap_or_else(|| !prefers_light_theme());
                    if let Ok(dialog) =
                        create_notice_dialog(&state.commands, &state.font_id, theme, sys_dark)
                    {
                        state.notice = Some(dialog);
                    }
                }
                if let Some(dialog) = state.notice.as_ref() {
                    let showed = update.show;
                    apply_notice_update(dialog, state.dock_hwnd, update);
                    if showed {
                        state.notice_open = true;
                    }
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn create_settings_window(
    dock_hwnd: isize,
    commands: &Sender<SettingsCommand>,
    latest_values: &Arc<Mutex<SettingsValues>>,
    initial: &SettingsValues,
    app_version: &str,
    startup_enabled: bool,
    startup_mode: StartupChoice,
    font_id: &str,
    system_dark: bool,
    update_view: &SettingsUpdateView,
) -> Result<SettingsWindow, slint::PlatformError> {
    let window = SettingsWindow::new()?;
    let monitor = metrics_for_window(HWND(dock_hwnd as *mut _));
    window.set_available_width((monitor.work.width() as f32 / monitor.dpi_scale - 32.0).max(1.0));
    window.set_available_height((monitor.work.height() as f32 / monitor.dpi_scale - 32.0).max(1.0));
    window.set_app_version(app_version.into());
    window.set_system_dark(system_dark);
    populate_values(&window, initial);
    window.set_startup_enabled(startup_enabled);
    window.set_startup_mode(startup_mode.as_id().into());
    window.set_font_id(font_id.into());
    window.set_status_message("设置已连接到 Glide-Dock 主程序。".into());
    window.set_diagnostics("选择“刷新诊断”以查看运行信息。".into());
    apply_update_view(&window, update_view);

    window.window().on_close_requested(|| {
        UI_STATE.with_borrow_mut(|slot| {
            if let Some(state) = slot.as_mut() {
                state.settings_open = false;
                state.maybe_quit_if_all_closed();
            }
        });
        slint::CloseRequestResponse::HideWindow
    });

    let weak = window.as_weak();
    let changes = commands.clone();
    let live = Arc::clone(latest_values);
    window.on_settings_changed(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let next = {
            let mut guard = live.lock().unwrap();
            let values = values_from_window(&window, &guard);
            *guard = values.clone();
            values
        };
        window.set_status_message("已保存设置，Dock 已同步更新。".into());
        let _ = changes.send(SettingsCommand::Apply(Box::new(next)));
    });

    let weak = window.as_weak();
    let actions = commands.clone();
    window.on_action(move |action| {
        if action == "window-minimize" {
            if let Some(window) = weak.upgrade() {
                window
                    .window()
                    .with_winit_window(|native| native.set_minimized(true));
            }
            return;
        }
        if action == "window-toggle-maximize" {
            if let Some(window) = weak.upgrade() {
                window.window().with_winit_window(|native| {
                    native.set_maximized(!native.is_maximized());
                });
            }
            return;
        }
        if action == "window-close" {
            if let Some(window) = weak.upgrade() {
                let _ = window.hide();
            }
            UI_STATE.with_borrow_mut(|slot| {
                if let Some(state) = slot.as_mut() {
                    state.settings_open = false;
                    state.maybe_quit_if_all_closed();
                }
            });
            return;
        }

        let command = match action.as_str() {
            "open-config" => SettingsCommand::OpenConfigDirectory,
            "open-logs" => SettingsCommand::OpenLogDirectory,
            "reset" => SettingsCommand::Reset,
            "reload" => SettingsCommand::Reload,
            "startup-enable" => SettingsCommand::SetStartup {
                enabled: true,
                mode: startup_choice_from_window(&weak),
            },
            "startup-disable" => SettingsCommand::SetStartup {
                enabled: false,
                mode: startup_choice_from_window(&weak),
            },
            "startup-update" => SettingsCommand::SetStartup {
                enabled: true,
                mode: startup_choice_from_window(&weak),
            },
            "startup-mode" => SettingsCommand::SetStartupMode(startup_choice_from_window(&weak)),
            "font-changed" => {
                let requested = weak
                    .upgrade()
                    .map(|window| window.get_font_id().to_string())
                    .unwrap_or_else(|| "system".into());
                let resolved = ensure_font_registered(&requested);
                if let Some(window) = weak.upgrade()
                    && window.get_font_id().as_str() != resolved
                {
                    window.set_font_id(resolved.clone().into());
                }
                SettingsCommand::SetFont(resolved)
            }
            "refresh-diagnostics" => SettingsCommand::RefreshDiagnostics,
            "copy-diagnostics" => SettingsCommand::CopyDiagnostics,
            "copy-motion" => SettingsCommand::CopyMotionConfig,
            "check-updates" => SettingsCommand::CheckForUpdates,
            "install-update" => SettingsCommand::InstallUpdate,
            "open-cnb-releases" => SettingsCommand::OpenCnbReleases,
            "open-github-releases" => SettingsCommand::OpenGithubReleases,
            _ => return,
        };
        let _ = actions.send(command);
    });
    Ok(window)
}

fn show_settings_window(window: &SettingsWindow) -> bool {
    let visible = window.window().is_visible();
    if !visible && let Err(error) = window.show() {
        eprintln!("could not show settings window: {error}");
        return false;
    }
    window.window().with_winit_window(|native| {
        if native.is_minimized() == Some(true) {
            native.set_minimized(false);
        }
        native.focus_window();
    });
    true
}

fn apply_settings_update(window: &SettingsWindow, update: SettingsUpdate) -> bool {
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
        window.set_startup_mode(mode.as_id().into());
    }
    if let Some(font_id) = update.font_id {
        window.set_font_id(font_id.into());
    }
    if let Some(update_view) = update.update_view {
        apply_update_view(window, &update_view);
    }
    if let Some(values) = update.values {
        populate_values(window, &values);
    }
    if update.show {
        show_settings_window(window)
    } else {
        false
    }
}

fn apply_update_view(window: &SettingsWindow, update: &SettingsUpdateView) {
    window.set_update_managed(update.managed);
    window.set_update_checking(update.checking);
    window.set_update_downloading(update.downloading);
    window.set_update_ready(update.ready);
    window.set_update_launching(update.launching);
    window.set_update_progress(update.progress as f32 / 100.0);
    window.set_update_message(update.status.clone().into());
    window.set_update_version(update.latest_version.clone().into());
    window.set_update_source(update.source.clone().into());
    window.set_update_cnb_status(update.cnb_status.clone().into());
    window.set_update_github_status(update.github_status.clone().into());
    window.set_update_last_check(update.last_check_text.clone().into());
    window.set_update_notes(update.release_notes.clone().into());
}

fn create_notice_dialog(
    commands: &Sender<SettingsCommand>,
    font_id: &str,
    theme: ThemeMode,
    system_dark: bool,
) -> Result<NoticeDialog, slint::PlatformError> {
    let dialog = NoticeDialog::new()?;
    dialog.set_font_id(font_id.into());
    dialog.set_theme(theme_id(theme).into());
    dialog.set_system_dark(system_dark);
    dialog.window().on_close_requested(|| {
        UI_STATE.with_borrow_mut(|slot| {
            if let Some(state) = slot.as_mut() {
                state.notice_open = false;
                state.maybe_quit_if_all_closed();
            }
        });
        slint::CloseRequestResponse::HideWindow
    });

    let weak = dialog.as_weak();
    let actions = commands.clone();
    dialog.on_action(move |action| match action.as_str() {
        "close" => {
            if let Some(dialog) = weak.upgrade() {
                let _ = dialog.hide();
            }
            UI_STATE.with_borrow_mut(|slot| {
                if let Some(state) = slot.as_mut() {
                    state.notice_open = false;
                    state.maybe_quit_if_all_closed();
                }
            });
        }
        "open-secpol" => {
            let _ = actions.send(SettingsCommand::OpenLocalSecurityPolicy);
        }
        "copy-command" => {
            let _ = actions.send(SettingsCommand::CopyNoticeCommand);
        }
        "quit-dock" => {
            let _ = actions.send(SettingsCommand::QuitDock);
            let _ = slint::quit_event_loop();
        }
        _ => {}
    });
    Ok(dialog)
}

fn apply_notice_update(dialog: &NoticeDialog, dock_hwnd: isize, update: NoticeUpdate) {
    if let Some(theme) = update.theme {
        dialog.set_theme(theme_id(theme).into());
    }
    if let Some(dark) = update.system_dark {
        dialog.set_system_dark(dark);
    }
    if let Some(feedback) = update.feedback {
        dialog.set_feedback_message(feedback.into());
    }
    if let Some(kind) = update.kind {
        dialog.set_kind(kind.id().into());
    }
    if update.show {
        let kind =
            NoticeKind::parse(dialog.get_kind().as_str()).unwrap_or(NoticeKind::ElevatedWarning);
        let monitor = metrics_for_window(HWND(dock_hwnd as *mut _));
        let available_width = (monitor.work.width() as f32 / monitor.dpi_scale - 32.0).max(1.0);
        let available_height = (monitor.work.height() as f32 / monitor.dpi_scale - 32.0).max(1.0);
        dialog.set_available_width(available_width);
        dialog.set_available_height(available_height);
        let (target_w, target_h) = match kind {
            NoticeKind::BuiltinAdminApproval => (620.0_f32, 540.0_f32),
            NoticeKind::ElevatedWarning => (520.0_f32, 292.0_f32),
        };
        let size = slint::LogicalSize::new(
            target_w.min(available_width),
            target_h.min(available_height),
        );
        dialog.window().set_size(size);
        dialog.window().set_position(slint::PhysicalPosition::new(
            monitor.work.left
                + ((monitor.work.width() as f32 - size.width * monitor.dpi_scale) / 2.0) as i32,
            monitor.work.top
                + ((monitor.work.height() as f32 - size.height * monitor.dpi_scale) / 2.0) as i32,
        ));
        if !dialog.window().is_visible()
            && let Err(error) = dialog.show()
        {
            eprintln!("could not show notice dialog: {error}");
            return;
        }
        dialog.window().with_winit_window(|native| {
            if native.is_minimized() == Some(true) {
                native.set_minimized(false);
            }
            native.focus_window();
        });
    }
}

fn ensure_font_registered(preferred: &str) -> String {
    if preferred != "noto-sans-sc" {
        return "system".into();
    }
    static FONT_REGISTERED: OnceLock<bool> = OnceLock::new();
    let available = *FONT_REGISTERED.get_or_init(|| {
        let executable_font = std::env::current_exe().ok().and_then(|path| {
            path.parent()
                .map(|parent| parent.join("assets/fonts/NotoSansSC-VF.ttf"))
        });
        let source_font =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/fonts/NotoSansSC-VF.ttf");
        let path = executable_font
            .filter(|path| path.is_file())
            .unwrap_or(source_font);
        let Ok(bytes) = std::fs::read(&path) else {
            return false;
        };
        let mut collection = slint::fontique_012::fontique::Collection::new(
            slint::fontique_012::fontique::CollectionOptions {
                shared: true,
                system_fonts: true,
            },
        );
        let fonts = collection.register_fonts(
            slint::fontique_012::fontique::Blob::new(Arc::new(bytes)),
            None,
        );
        if fonts.is_empty() {
            return false;
        }
        slint::fontique_012::set_shared_collection(collection).is_ok()
    });
    if available {
        "noto-sans-sc".into()
    } else {
        "system".into()
    }
}

fn startup_choice_from_window(weak: &slint::Weak<SettingsWindow>) -> StartupChoice {
    weak.upgrade()
        .map(|window| StartupChoice::from_id(window.get_startup_mode().as_str()))
        .unwrap_or(StartupChoice::Task)
}

fn populate_values(window: &SettingsWindow, values: &SettingsValues) {
    let appearance = &values.appearance;
    let behavior = &values.behavior;
    let motion = &values.motion;
    window.set_system_dark(!prefers_light_theme());
    window.set_theme(theme_id(appearance.theme).into());
    window.set_accent(accent_id(appearance.accent).into());
    window.set_layer(layer_id(appearance.layer).into());
    window.set_dock_font(appearance.dock_font.as_id().into());
    window.set_icon_size(appearance.icon_size);
    window.set_spacing(appearance.spacing);
    window.set_dock_padding(appearance.padding);
    window.set_corner_radius(appearance.corner_radius);
    window.set_dock_opacity(appearance.opacity * 100.0);
    window.set_frosted_glass(appearance.frosted_glass);
    window.set_gpu_acceleration(appearance.gpu_acceleration);
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
    values.appearance.dock_font = DockFont::from_id(window.get_dock_font().as_str());
    values.appearance.icon_size = window.get_icon_size();
    values.appearance.spacing = window.get_spacing();
    values.appearance.padding = window.get_dock_padding();
    values.appearance.corner_radius = window.get_corner_radius();
    values.appearance.opacity = window.get_dock_opacity() / 100.0;
    values.appearance.frosted_glass = window.get_frosted_glass();
    values.appearance.gpu_acceleration = window.get_gpu_acceleration();
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

#[cfg(test)]
mod update_view_tests {
    use super::*;

    #[test]
    fn about_page_and_notice_dialog_initialize_and_update() {
        let window = SettingsWindow::new().expect("settings component should initialize");
        window.set_app_version(env!("CARGO_PKG_VERSION").into());
        let mut snapshot = SettingsUpdateView {
            managed: true,
            latest_version: env!("CARGO_PKG_VERSION").into(),
            status: format!("当前已是最新版本 v{}。", env!("CARGO_PKG_VERSION")),
            ..Default::default()
        };
        apply_update_view(&window, &snapshot);
        assert!(window.get_update_managed());
        assert_eq!(window.get_app_version().as_str(), env!("CARGO_PKG_VERSION"));
        assert!(!window.get_update_checking());

        snapshot.checking = true;
        snapshot.status = "正在检查更新…".into();
        apply_update_view(&window, &snapshot);
        assert!(window.get_update_checking());
        assert_eq!(window.get_update_message().as_str(), "正在检查更新…");

        snapshot.checking = false;
        snapshot.downloading = true;
        snapshot.progress = 42;
        snapshot.latest_version = "0.5.0".into();
        snapshot.source = "CNB".into();
        apply_update_view(&window, &snapshot);
        assert!(window.get_update_downloading());
        assert_eq!(window.get_update_version().as_str(), "0.5.0");
        assert_eq!(window.get_update_source().as_str(), "CNB");
        assert!((window.get_update_progress() - 0.42).abs() < 0.001);

        snapshot.downloading = false;
        snapshot.ready = true;
        snapshot.progress = 100;
        apply_update_view(&window, &snapshot);
        assert!(!window.get_update_downloading());
        assert!(window.get_update_ready());

        snapshot.ready = false;
        snapshot.launching = true;
        apply_update_view(&window, &snapshot);
        assert!(window.get_update_launching());

        // Switch UI to About page and confirm all properties bind cleanly.
        window.set_page(5);
        assert_eq!(window.get_page(), 5);

        let initial = SettingsValues::default();
        populate_values(&window, &initial);
        assert_eq!(window.get_dock_font().as_str(), "system");
        window.set_dock_font("noto-sans-sc".into());
        let updated = values_from_window(&window, &initial);
        assert_eq!(updated.appearance.dock_font, DockFont::NotoSansSc);

        let dialog = NoticeDialog::new().expect("notice dialog should initialize");
        dialog.set_kind(NoticeKind::BuiltinAdminApproval.id().into());
        assert!(dialog.get_builtin_mode());
        assert_eq!(
            dialog.get_dialog_title().as_str(),
            "建议开启内置管理员审批模式"
        );

        dialog.set_kind(NoticeKind::ElevatedWarning.id().into());
        assert!(!dialog.get_builtin_mode());
        assert_eq!(dialog.get_dialog_title().as_str(), "检测到管理员权限");
    }
}
