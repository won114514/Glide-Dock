#![windows_subsystem = "windows"]

mod elevation;
mod engine;
mod version;

use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use dock_core::theme::ThemeMode;
use dock_platform_windows::startup::{self, StartupMode};
use elevation::{create_uninstall_copy, remove_worker_copy};
use engine::{InstallOptions, InstallRecord, InstallScope, PreflightCheck, RecommendedApp};
use serde::{Deserialize, Serialize};
use tauri::{State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use webview2_com::AcceleratorKeyPressedEventHandler;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_F5, VK_MENU, VK_R};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};

const WORKER_FLAG: &str = "--glide-worker";
const UI_COPY_FLAG: &str = "--uninstall-copy";
const UNINSTALL_ONLY: bool = cfg!(glide_uninstaller);
const PRODUCT_TITLE: &str = if UNINSTALL_ONLY {
    "Glide-Dock 卸载程序"
} else {
    "Glide-Dock 安装程序"
};

struct Session {
    mode: &'static str,
    auto_update: bool,
    existing: Option<InstallRecord>,
    operation: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct SetupInfo {
    version: &'static str,
    mode: &'static str,
    auto_update: bool,
    payload_ready: bool,
    update_eligibility: Option<version::UpdateEligibility>,
    install_directory: String,
    existing: Option<InstallRecord>,
    checks: Vec<PreflightCheck>,
    apps: Vec<RecommendedApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct OperationStatus {
    percent: u8,
    message: String,
    phase: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|arg| arg == elevation::STARTUP_WORKER_FLAG)
    {
        std::process::exit(elevation::run_startup_worker(&args));
    }
    if args.first().is_some_and(|arg| arg == WORKER_FLAG) {
        std::process::exit(run_worker(&args));
    }

    if dock_platform_windows::is_low_integrity() {
        show_startup_error(
            "安装程序当前运行在 Windows 低完整性环境中，无法访问安装所需的数据。\n\n请将安装程序复制到桌面或“下载”文件夹后，再双击运行。",
        );
        std::process::exit(1);
    }

    let existing = engine::find_install();
    let mode = match initial_mode(UNINSTALL_ONLY, &args, existing.is_some()) {
        Ok(mode) => mode,
        Err(message) => {
            dock_platform_windows::show_message(HWND::default(), PRODUCT_TITLE, message, false);
            return;
        }
    };
    if mode == "uninstall"
        && let Some(record) = &existing
    {
        let prepare = std::env::current_exe().and_then(|source| {
            let directory = elevation::uninstall_working_directory(
                &source,
                &record.install_directory,
                &std::env::temp_dir(),
            )?;
            std::env::set_current_dir(directory)
        });
        if let Err(error) = prepare {
            show_startup_error(&format!("无法切换卸载工作目录：{error}"));
            return;
        }
    }
    if mode == "uninstall"
        && existing
            .as_ref()
            .is_some_and(|record| executable_is_inside(&record.install_directory))
    {
        match run_from_temporary_copy(&args) {
            Ok(()) => return,
            Err(error) => {
                show_startup_error(&error.to_string());
                return;
            }
        }
    }
    if mode != "uninstall"
        && existing
            .as_ref()
            .is_some_and(|record| executable_is_inside(&record.install_directory))
    {
        show_startup_error(
            "请将新版安装包移到“下载”文件夹或临时目录后运行，不能从正在更新的安装目录内部执行更新。",
        );
        return;
    }
    let session = Session {
        mode,
        auto_update: args.iter().any(|arg| arg == "--auto-update"),
        existing,
        operation: Mutex::new(None),
    };

    let builder = tauri::Builder::default().manage(session);
    let builder = if UNINSTALL_ONLY {
        builder.invoke_handler(tauri::generate_handler![
            setup_info,
            start_uninstall,
            operation_status,
            finish_operation,
        ])
    } else {
        builder.invoke_handler(tauri::generate_handler![
            setup_info,
            default_destination,
            check_destination,
            browse_destination,
            start_install,
            start_uninstall,
            operation_status,
            finish_operation,
        ])
    };
    let result = builder
        .setup(|app| {
            // Tauri panics if a setup hook returns Err, so report window failures here.
            if let Err(error) = create_setup_window(app) {
                show_startup_error(&error.to_string());
                app.handle().exit(1);
            }
            Ok(())
        })
        .run(tauri::generate_context!());

    if let Err(error) = result {
        show_startup_error(&error.to_string());
        std::process::exit(1);
    }
}

fn show_startup_error(detail: &str) {
    let message = format!("{PRODUCT_TITLE}未能启动。\n\n{detail}");
    eprintln!("{message}");
    dock_platform_windows::show_message(HWND::default(), PRODUCT_TITLE, &message, true);
}

fn create_setup_window(app: &tauri::App) -> tauri::Result<()> {
    let (width, height) = app.primary_monitor()?.map_or((840.0, 620.0), |monitor| {
        let work = monitor.work_area();
        let scale = monitor.scale_factor();
        (
            (f64::from(work.size.width) / scale - 32.0).clamp(1.0, 840.0),
            (f64::from(work.size.height) / scale - 32.0).clamp(1.0, 620.0),
        )
    });
    let window = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
        .title(PRODUCT_TITLE)
        .inner_size(width, height)
        .min_inner_size(width, height)
        .max_inner_size(width, height)
        .center()
        .decorations(false)
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .visible(false)
        .build()?;
    window.with_webview(|webview| {
        // WebView2 can handle refresh before a DOM keydown reaches the wizard.
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let mut key = 0;
                unsafe {
                    args.VirtualKey(&mut key)?;
                    if key == u32::from(VK_F5.0)
                        || (key == u32::from(VK_R.0)
                            && GetKeyState(i32::from(VK_CONTROL.0)) < 0
                            && GetKeyState(i32::from(VK_MENU.0)) >= 0)
                    {
                        args.SetHandled(true)?;
                    }
                }
            }
            Ok(())
        }));
        // The controller owns the handler for the lifetime of the window.
        let mut token = 0;
        if let Err(error) = unsafe {
            webview
                .controller()
                .add_AcceleratorKeyPressed(&handler, &mut token)
        } {
            tracing::warn!(%error, "could not register WebView2 refresh shortcut handler");
        }
    })?;
    let handle = window.hwnd()?;
    // Keep the frame square on Windows 11; older Windows versions are already square.
    unsafe {
        let _ = DwmSetWindowAttribute(
            HWND(handle.0),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&DWMWCP_DONOTROUND).cast(),
            std::mem::size_of_val(&DWMWCP_DONOTROUND) as u32,
        );
    }
    window.show()?;
    Ok(())
}

fn initial_mode(
    uninstall_only: bool,
    args: &[String],
    installed: bool,
) -> Result<&'static str, &'static str> {
    let auto_update = args.iter().any(|arg| arg == "--auto-update");
    if auto_update && (uninstall_only || !installed || args.iter().any(|arg| arg == "--uninstall"))
    {
        return Err("自动更新参数只适用于已安装 Glide-Dock 的安装程序。请手动运行安装程序。");
    }
    if uninstall_only && args.iter().any(|arg| arg == "--update") {
        return Err("这是 Glide-Dock 卸载程序，更新请运行下载的新版安装包。");
    }
    if uninstall_only || args.iter().any(|arg| arg == "--uninstall") {
        return if installed {
            Ok("uninstall")
        } else {
            Err("没有找到已注册的安装。")
        };
    }
    Ok(if installed { "update" } else { "install" })
}

fn executable_is_inside(directory: &Path) -> bool {
    std::env::current_exe().is_ok_and(|source| elevation::is_inside(&source, directory))
}

fn run_from_temporary_copy(args: &[String]) -> io::Result<()> {
    let copy = create_uninstall_copy(&std::env::current_exe()?, &std::env::temp_dir())?;
    let mut command = Command::new(&copy);
    command
        .args(args)
        .current_dir(
            copy.parent()
                .ok_or_else(|| io::Error::other("卸载副本路径无效。"))?,
        )
        .arg("--uninstall")
        .arg(UI_COPY_FLAG)
        .creation_flags(0x0800_0000);
    if let Err(error) = command.spawn() {
        remove_worker_copy(&copy);
        return Err(io::Error::other(format!("无法重新打开安装向导：{error}")));
    }
    Ok(())
}

#[tauri::command]
fn setup_info(session: State<'_, Session>) -> Result<SetupInfo, String> {
    let existing = session.existing.clone();
    let scope = existing
        .as_ref()
        .map_or(InstallScope::CurrentUser, |record| record.scope);
    let path = existing
        .as_ref()
        .map_or_else(
            || engine::default_install_directory(scope),
            |record| Ok(record.install_directory.clone()),
        )
        .map_err(|error| format!("无法读取默认安装位置：{error}"))?;
    Ok(SetupInfo {
        version: env!("CARGO_PKG_VERSION"),
        mode: session.mode,
        auto_update: session.auto_update,
        payload_ready: engine::installer_ready(),
        update_eligibility: if session.mode == "update" {
            existing.as_ref().map(engine::update_eligibility)
        } else {
            None
        },
        install_directory: path.to_string_lossy().into_owned(),
        existing,
        checks: engine::check_environment(),
        apps: if session.mode == "install" {
            engine::scan_installed_apps()
        } else {
            Vec::new()
        },
    })
}

#[tauri::command]
fn default_destination(scope: InstallScope) -> Result<String, String> {
    engine::default_install_directory(scope)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| format!("无法读取默认安装位置：{error}"))
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct DestinationCheck {
    valid: bool,
    space: String,
    message: String,
}

#[tauri::command]
fn check_destination(
    install_directory: String,
    scope: InstallScope,
    update: bool,
    session: State<'_, Session>,
) -> DestinationCheck {
    let fail = |error: String| DestinationCheck {
        valid: false,
        space: String::new(),
        message: error,
    };
    if !matches!(session.mode, "install" | "update") {
        return fail("卸载模式不接受新的安装位置。".into());
    }
    let options = InstallOptions {
        scope,
        install_directory: PathBuf::from(install_directory),
        create_desktop_shortcut: false,
        enable_startup: false,
        startup_mode: StartupMode::Task,
        launch_after_install: false,
        theme: ThemeMode::System,
        selected_apps: Vec::new(),
        update: update || session.mode == "update",
    };
    let path = match engine::resolve_install_target(&options) {
        Ok(path) => path,
        Err(error) => return fail(error.to_string()),
    };
    match engine::validate_capacity(&path) {
        Ok(space) => DestinationCheck {
            valid: true,
            space,
            message: "此位置可以安装。".into(),
        },
        Err(error) => fail(error.to_string()),
    }
}

#[tauri::command]
async fn browse_destination(window: WebviewWindow) -> Result<Option<String>, String> {
    let owner = window
        .hwnd()
        .map_err(|error| format!("无法关联文件夹选择窗口：{error}"))?
        .0 as usize;
    // The shell dialog sends messages to its owner. Keep the owner's event loop
    // running while a dedicated STA thread owns the dialog and its COM objects.
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    std::thread::Builder::new()
        .name("glide-setup-folder-picker".into())
        .spawn(move || {
            let result = choose_folder(HWND(owner as *mut _))
                .map_err(|error| format!("无法选择安装位置：{error}"));
            let _ = sender.blocking_send(result);
        })
        .map_err(|error| format!("无法打开文件夹选择器：{error}"))?;
    receiver
        .recv()
        .await
        .ok_or_else(|| "文件夹选择器意外退出，请重试。".to_string())?
}

fn choose_folder(owner: HWND) -> windows::core::Result<Option<String>> {
    let _com = dock_platform_windows::ComGuard::sta();
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        dialog.SetTitle(windows::core::w!("选择 Glide-Dock 安装文件夹"))?;
    }
    unsafe {
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
    }
    if let Err(error) = unsafe { dialog.Show(Some(owner)) } {
        if error.code() == windows::Win32::Foundation::ERROR_CANCELLED.to_hresult() {
            return Ok(None);
        }
        return Err(error);
    }
    let item = unsafe { dialog.GetResult()? };
    let display = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = unsafe { display.to_string() };
    unsafe {
        CoTaskMemFree(Some(display.0.cast()));
    }
    Ok(Some(path?))
}

#[tauri::command]
fn start_install(
    session: State<'_, Session>,
    update: bool,
    mut options: InstallOptions,
) -> Result<String, String> {
    if UNINSTALL_ONLY || session.mode == "uninstall" {
        return Err("当前向导只允许卸载。".into());
    }
    let existing = session
        .existing
        .as_ref()
        .ok_or_else(|| "安装记录已变更。请重新打开向导。".to_string());
    if update || session.mode == "update" {
        let record = existing?;
        if options.scope != record.scope
            || !same_path(&options.install_directory, &record.install_directory)
        {
            return Err("更新位置已改变。请重新打开向导。".into());
        }
        options.update = true;
    } else {
        if session.existing.is_some() {
            return Err("此电脑已有 Glide-Dock 安装，请运行新版安装包更新。".into());
        }
        options.update = false;
    }
    engine::ensure_update_allowed(&options).map_err(|error| error.to_string())?;
    let target = engine::resolve_install_target(&options).map_err(|error| error.to_string())?;
    engine::validate_capacity(&target).map_err(|error| error.to_string())?;
    let allowed_apps = engine::scan_installed_apps();
    options.selected_apps.retain(|selected| {
        allowed_apps.iter().any(|allowed| {
            allowed.name == selected.name && same_path(&allowed.path, &selected.path)
        })
    });
    start_operation(&session, OperationRequest::Install(options))
}

#[tauri::command]
fn start_uninstall(
    session: State<'_, Session>,
    delete_personal_data: bool,
) -> Result<String, String> {
    if session.mode == "install" {
        return Err("没有找到可以卸载的 Glide-Dock。".into());
    }
    let record = session
        .existing
        .as_ref()
        .ok_or_else(|| "安装记录已变更。请重新打开向导。".to_string())?;
    engine::verify_uninstall_target(record).map_err(|error| error.to_string())?;
    let request = UninstallRequest {
        record: record.clone(),
        delete_personal_data,
    };
    start_operation(&session, OperationRequest::Uninstall(request))
}

fn start_operation(session: &Session, request: OperationRequest) -> Result<String, String> {
    let source = std::env::current_exe().map_err(|error| error.to_string())?;
    let elevated = request.needs_elevation(dock_platform_windows::is_elevated());
    start_operation_with(
        session,
        request,
        elevated,
        &source,
        &|executable, args| {
            dock_platform_windows::process::spawn_elevated_detached(executable, &args.join(" "))
                .map_err(|error| {
                    let canceled = dock_platform_windows::process::elevation_canceled(&error);
                    (error.to_string(), canceled)
                })
        },
        &|request, token| {
            std::thread::Builder::new()
                .name("glide-operation".into())
                .spawn(move || {
                    run_operation(request, &token);
                })
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    )
}

fn start_operation_with(
    session: &Session,
    request: OperationRequest,
    elevated: bool,
    source: &Path,
    spawn_elevated: &impl Fn(&Path, &[&str]) -> Result<(), (String, bool)>,
    execute_local: &impl Fn(OperationRequest, String) -> Result<(), String>,
) -> Result<String, String> {
    if UNINSTALL_ONLY && matches!(request, OperationRequest::Install(_)) {
        return Err("当前向导只允许卸载。".into());
    }
    let mut active = session
        .operation
        .lock()
        .map_err(|_| "安装状态不可用。".to_string())?;
    if active.is_some() {
        return Err("另一个 Glide-Dock 安装操作正在进行。".into());
    }
    let executable = elevation::operation_executable(
        source,
        session
            .existing
            .as_ref()
            .map(|record| record.install_directory.as_path()),
    )
    .map_err(|error| error.to_string())?;
    let token = format!("{}-{}", std::process::id(), unique_nanos());
    write_status(
        &token,
        OperationStatus {
            percent: 1,
            message: if elevated {
                "等待管理员确认…"
            } else {
                "正在准备…"
            }
            .into(),
            phase: "running".into(),
            warnings: Vec::new(),
        },
    )
    .map_err(|error| format!("无法准备安装状态：{error}"))?;
    let result = if elevated {
        request
            .worker_payload()
            .map_err(|error| (error, false))
            .and_then(|(operation, payload)| {
                spawn_elevated(&executable, &[WORKER_FLAG, operation, &token, &payload])
            })
    } else {
        execute_local(request, token.clone()).map_err(|error| (error, false))
    };
    if let Err((error, canceled)) = result {
        let _ = fs::remove_file(status_path(&token));
        return Err(if canceled {
            "你已取消管理员确认，没有更改系统设置。".into()
        } else {
            format!("无法开始操作：{error}")
        });
    }
    *active = Some(token.clone());
    Ok(token)
}

#[derive(Serialize, Deserialize)]
struct UninstallRequest {
    record: InstallRecord,
    delete_personal_data: bool,
}

enum OperationRequest {
    Install(InstallOptions),
    Uninstall(UninstallRequest),
}

impl OperationRequest {
    fn needs_elevation(&self, already_elevated: bool) -> bool {
        self.scope() == InstallScope::AllUsers && !already_elevated
    }
    fn scope(&self) -> InstallScope {
        match self {
            Self::Install(options) => options.scope,
            Self::Uninstall(request) => request.record.scope,
        }
    }
    fn worker_payload(&self) -> Result<(&'static str, String), String> {
        match self {
            Self::Install(options) => encode(options).map(|payload| ("install", payload)),
            Self::Uninstall(request) => encode(request).map(|payload| ("uninstall", payload)),
        }
        .map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn operation_status(token: String, session: State<'_, Session>) -> Result<OperationStatus, String> {
    let active = session
        .operation
        .lock()
        .map_err(|_| "安装状态不可用。".to_string())?;
    if active.as_deref() != Some(&token) {
        return Err("安装操作已过期。".into());
    }
    let text = fs::read_to_string(status_path(&token))
        .map_err(|error| format!("读取安装进度失败：{error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("安装进度格式无效：{error}"))
}

#[tauri::command]
fn finish_operation(token: String, session: State<'_, Session>) -> Result<(), String> {
    let mut active = session
        .operation
        .lock()
        .map_err(|_| "安装状态不可用。".to_string())?;
    if active.as_deref() != Some(&token) {
        return Err("安装操作已过期。".into());
    }
    let status = fs::read_to_string(status_path(&token)).map_err(|error| error.to_string())?;
    let status: OperationStatus =
        serde_json::from_str(&status).map_err(|error| error.to_string())?;
    if status.phase == "running" {
        return Err("安装操作仍在进行。".into());
    }
    fs::remove_file(status_path(&token)).map_err(|error| error.to_string())?;
    *active = None;
    Ok(())
}

fn run_worker(args: &[String]) -> i32 {
    if args.len() != 4 {
        return 2;
    }
    let token = &args[2];
    if !valid_token(token) {
        return 2;
    }
    let request = match args[1].as_str() {
        "install" if !UNINSTALL_ONLY => {
            decode::<InstallOptions>(&args[3]).map(OperationRequest::Install)
        }
        "uninstall" => decode::<UninstallRequest>(&args[3]).map(OperationRequest::Uninstall),
        _ => Err("未知的安装工作类型。".into()),
    };
    match request {
        Ok(request) => run_operation(request, token),
        Err(error) => fail_operation(token, 1, error),
    }
}

fn run_operation(request: OperationRequest, token: &str) -> i32 {
    let uninstalling = matches!(request, OperationRequest::Uninstall(_));
    let mut current_percent = 1;
    let outcome = match request {
        OperationRequest::Install(options) if !UNINSTALL_ONLY => (|| {
            engine::install(&options, |progress| {
                let percent = 4 + (u16::from(progress.percent) * 72 / 100) as u8;
                report_operation_progress(token, &mut current_percent, percent, progress.message);
            })
            .map_err(|error| error.to_string())?;
            finish_install(&options, token, &mut current_percent)?;
            Ok(Vec::new())
        })(),
        OperationRequest::Uninstall(request) => (|| {
            let config = dock_storage::config_dir();
            report_operation_progress(token, &mut current_percent, 5, "正在检查登录启动项…");
            startup::remove_for_install_with_elevation(
                &request.record.install_directory,
                &config,
                &elevation::elevate_startup_task,
            )
            .map_err(|error| format!("无法移除 Glide-Dock 的登录启动项：{error}"))?;
            let mut warnings = engine::uninstall(&request.record, |progress| {
                report_operation_progress(
                    token,
                    &mut current_percent,
                    progress.percent,
                    progress.message,
                );
            })
            .map_err(|error| error.to_string())?;
            if request.delete_personal_data {
                report_operation_progress(
                    token,
                    &mut current_percent,
                    96,
                    "正在删除个人设置和日志…",
                );
                if let Err(error) = engine::remove_user_data() {
                    warnings.push(format!("程序已卸载，但个人数据清理未完成：{error}"));
                }
            }
            Ok(warnings)
        })(),
        _ => Err("未知的安装工作类型。".into()),
    };
    match outcome {
        Ok(warnings) => {
            let mut message = if uninstalling {
                "Glide-Dock 已卸载。"
            } else {
                "Glide-Dock 已安装。"
            }
            .to_string();
            if !warnings.is_empty() {
                message.push_str(&format!("\n{}", warnings.join("\n")));
            }
            let _ = write_status(
                token,
                OperationStatus {
                    percent: 100,
                    message,
                    phase: "complete".into(),
                    warnings,
                },
            );
            0
        }
        Err(error) => fail_operation(token, current_percent.min(99), error),
    }
}

fn fail_operation(token: &str, percent: u8, message: String) -> i32 {
    let _ = write_status(
        token,
        OperationStatus {
            percent,
            message,
            phase: "failed".into(),
            warnings: Vec::new(),
        },
    );
    1
}

fn report_operation_progress(
    token: &str,
    current: &mut u8,
    percent: u8,
    message: impl Into<String>,
) {
    *current = (*current).max(percent).min(99);
    let _ = write_status(
        token,
        OperationStatus {
            percent: *current,
            message: message.into(),
            phase: "running".into(),
            warnings: Vec::new(),
        },
    );
}

fn finish_install(
    options: &InstallOptions,
    token: &str,
    current_percent: &mut u8,
) -> Result<(), String> {
    if !options.update {
        report_operation_progress(token, current_percent, 80, "正在保存首次启动设置…");
        engine::create_first_run_config(options.theme, &options.selected_apps)
            .map_err(|error| format!("程序已安装，但首次设置未能保存：{error}"))?;
    }
    let executable = options.install_directory.join("glide-dock.exe");
    if options.enable_startup && !options.update {
        report_operation_progress(token, current_percent, 87, "正在设置登录时自动启动…");
        startup::enable_with_elevation(
            options.startup_mode,
            &executable,
            &dock_storage::config_dir(),
            &elevation::elevate_startup_task,
        )
        .map_err(|error| format!("程序已安装，但登录启动项未能创建：{error}"))?;
    }
    if options.launch_after_install {
        report_operation_progress(token, current_percent, 94, "正在启动 Glide-Dock…");
        Command::new(executable)
            .current_dir(&options.install_directory)
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|error| format!("程序已安装，但 Dock 未能启动：{error}"))?;
    }
    report_operation_progress(token, current_percent, 98, "正在完成安装检查…");
    Ok(())
}

fn encode<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(serde_json::to_vec(value)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn decode<T: serde::de::DeserializeOwned>(hex: &str) -> Result<T, String> {
    if !hex.len().is_multiple_of(2) || hex.len() > 4 * 1024 * 1024 {
        return Err("工作进程参数长度无效。".into());
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let (pairs, _) = hex.as_bytes().as_chunks::<2>();
    for pair in pairs {
        let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
        bytes.push(u8::from_str_radix(pair, 16).map_err(|error| error.to_string())?);
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn unique_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
}

fn status_path(token: &str) -> PathBuf {
    std::env::temp_dir().join(format!("Glide-Dock-Setup-{token}.json"))
}

fn write_status(token: &str, status: OperationStatus) -> io::Result<()> {
    if !valid_token(token) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid operation id",
        ));
    }
    let target = status_path(token);
    let temporary = target.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&status).map_err(io::Error::other)?,
    )?;
    match fs::rename(&temporary, &target) {
        Ok(()) => Ok(()),
        Err(error) if target.exists() => {
            fs::remove_file(&target)?;
            fs::rename(temporary, target).map_err(|_| error)
        }
        Err(error) => {
            let _ = fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod routing_tests {
    use super::*;

    fn request(scope: InstallScope) -> OperationRequest {
        let directory = std::env::temp_dir().join("Glide-Dock-operation-test-target");
        if UNINSTALL_ONLY {
            OperationRequest::Uninstall(UninstallRequest {
                record: InstallRecord {
                    scope,
                    version: Some("0.4.2".into()),
                    install_directory: directory,
                },
                delete_personal_data: false,
            })
        } else {
            OperationRequest::Install(InstallOptions {
                scope,
                install_directory: directory,
                create_desktop_shortcut: false,
                enable_startup: false,
                startup_mode: StartupMode::Run,
                launch_after_install: false,
                theme: ThemeMode::System,
                selected_apps: Vec::new(),
                update: false,
            })
        }
    }

    #[test]
    fn canceled_operation_uac_leaves_original_file_and_allows_retry() {
        let session = Session {
            mode: "install",
            auto_update: false,
            existing: None,
            operation: Mutex::new(None),
        };
        let source = std::env::current_exe().unwrap();
        let before = fs::metadata(&source).unwrap().len();
        let captured = std::cell::RefCell::new(String::new());
        let error = start_operation_with(
            &session,
            request(InstallScope::AllUsers),
            true,
            &source,
            &|executable, args| {
                assert_eq!(executable, source);
                *captured.borrow_mut() = args[2].to_string();
                Err(("canceled".into(), true))
            },
            &|_, _| panic!("UAC cancellation must not execute locally"),
        )
        .unwrap_err();
        assert!(error.contains("取消管理员确认"));
        assert!(session.operation.lock().unwrap().is_none());
        assert!(!status_path(&captured.borrow()).exists());
        assert_eq!(fs::metadata(&source).unwrap().len(), before);
        let token = start_operation_with(
            &session,
            request(InstallScope::AllUsers),
            true,
            &source,
            &|executable, _| {
                assert_eq!(executable, source);
                Ok(())
            },
            &|_, _| panic!("An elevated worker was requested"),
        )
        .unwrap();
        assert_eq!(
            session.operation.lock().unwrap().as_deref(),
            Some(token.as_str())
        );
        fs::remove_file(status_path(&token)).unwrap();
    }

    #[test]
    fn local_and_already_elevated_operations_do_not_launch_a_process() {
        for (scope, already_elevated) in [
            (InstallScope::CurrentUser, false),
            (InstallScope::AllUsers, true),
        ] {
            let session = Session {
                mode: "install",
                auto_update: false,
                existing: None,
                operation: Mutex::new(None),
            };
            let request = request(scope);
            let elevated = request.needs_elevation(already_elevated);
            assert!(!elevated);
            let captured = std::cell::RefCell::new(None);
            let token = start_operation_with(
                &session,
                request,
                elevated,
                &std::env::current_exe().unwrap(),
                &|_, _| panic!("An operation with sufficient privileges must stay in-process"),
                &|request, token| {
                    assert_eq!(request.scope(), scope);
                    *captured.borrow_mut() = Some(token);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(captured.borrow().as_deref(), Some(token.as_str()));
            assert_eq!(
                session.operation.lock().unwrap().as_deref(),
                Some(token.as_str())
            );
            fs::remove_file(status_path(&token)).unwrap();
        }
    }

    #[test]
    fn failed_background_dispatch_leaves_no_active_operation() {
        let session = Session {
            mode: "install",
            auto_update: false,
            existing: None,
            operation: Mutex::new(None),
        };
        let captured = std::cell::RefCell::new(String::new());
        let error = start_operation_with(
            &session,
            request(InstallScope::CurrentUser),
            false,
            &std::env::current_exe().unwrap(),
            &|_, _| panic!("A local operation must not launch a worker"),
            &|_, token| {
                *captured.borrow_mut() = token;
                Err("thread unavailable".into())
            },
        )
        .unwrap_err();
        assert!(error.contains("thread unavailable"));
        assert!(session.operation.lock().unwrap().is_none());
        assert!(!status_path(&captured.borrow()).exists());
    }

    #[test]
    fn downloaded_installer_opens_install_or_update_directly() {
        assert_eq!(initial_mode(false, &[], false).unwrap(), "install");
        assert_eq!(initial_mode(false, &[], true).unwrap(), "update");
        assert_eq!(
            initial_mode(false, &["--update".into()], true).unwrap(),
            "update"
        );
        assert_eq!(
            initial_mode(false, &["--auto-update".into()], true).unwrap(),
            "update"
        );
    }

    #[test]
    fn auto_update_requires_a_registered_install_and_cannot_uninstall() {
        let auto_update = ["--auto-update".into()];
        assert!(initial_mode(false, &auto_update, false).is_err());
        assert!(initial_mode(true, &auto_update, true).is_err());
        assert!(
            initial_mode(false, &["--auto-update".into(), "--uninstall".into()], true).is_err()
        );
        assert_eq!(initial_mode(false, &auto_update, true).unwrap(), "update");
    }

    #[test]
    fn uninstaller_never_selects_install_or_update() {
        assert_eq!(initial_mode(true, &[], true).unwrap(), "uninstall");
        assert!(initial_mode(true, &[], false).is_err());
        assert!(initial_mode(true, &["--update".into()], true).is_err());
        assert!(initial_mode(true, &["--auto-update".into()], true).is_err());
        assert_eq!(
            initial_mode(false, &["--uninstall".into()], true).unwrap(),
            "uninstall"
        );
    }

    #[cfg(glide_uninstaller)]
    #[test]
    fn uninstaller_rejects_install_worker_even_with_direct_arguments() {
        let token = format!("{}-{}", std::process::id(), unique_nanos());
        let directory = std::env::temp_dir().join(format!("Glide-Dock-rejected-install-{token}"));
        assert!(!directory.exists());
        for update in [false, true] {
            let options = InstallOptions {
                scope: InstallScope::CurrentUser,
                install_directory: directory.clone(),
                create_desktop_shortcut: false,
                enable_startup: false,
                startup_mode: StartupMode::Run,
                launch_after_install: false,
                theme: ThemeMode::System,
                selected_apps: Vec::new(),
                update,
            };
            let args = [
                WORKER_FLAG.into(),
                "install".into(),
                token.clone(),
                encode(&options).unwrap(),
            ];
            assert_eq!(run_worker(&args), 1);
            let status: OperationStatus =
                serde_json::from_slice(&fs::read(status_path(&token)).unwrap()).unwrap();
            assert_eq!(status.phase, "failed");
            assert_eq!(status.message, "未知的安装工作类型。");
            fs::remove_file(status_path(&token)).unwrap();
            assert!(!directory.exists());
        }
        assert!(!engine::installer_ready());
    }
}
