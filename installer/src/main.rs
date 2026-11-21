#![windows_subsystem = "windows"]

mod engine;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use dock_core::theme::ThemeMode;
use dock_platform_windows::startup::{self, StartupMode};
use engine::{InstallOptions, InstallRecord, InstallScope, PreflightCheck, RecommendedApp};
use serde::{Deserialize, Serialize};
use tauri::{State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};

const WORKER_FLAG: &str = "--glide-worker";
const UI_COPY_FLAG: &str = "--setup-copy";

struct Session {
    mode: &'static str,
    existing: Option<InstallRecord>,
    operation: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct SetupInfo {
    version: &'static str,
    mode: &'static str,
    payload_ready: bool,
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
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
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
    if !args.iter().any(|arg| arg == UI_COPY_FLAG)
        && existing
            .as_ref()
            .is_some_and(|record| executable_is_inside(&record.install_directory))
    {
        match run_from_temporary_copy(&args) {
            Ok(()) => return,
            Err(error) => {
                dock_platform_windows::show_message(
                    HWND::default(),
                    "Glide-Dock 安装程序",
                    &error.to_string(),
                    true,
                );
                return;
            }
        }
    }

    let requested = if args.iter().any(|arg| arg == "--uninstall") {
        "uninstall"
    } else if args.iter().any(|arg| arg == "--update") && existing.is_some() {
        "update"
    } else if existing.is_some() {
        "manage"
    } else {
        "install"
    };
    let mode = match (requested, existing.as_ref()) {
        ("uninstall", None) => {
            dock_platform_windows::show_message(
                HWND::default(),
                "Glide-Dock",
                "没有找到已注册的安装。",
                false,
            );
            return;
        }
        ("uninstall", Some(_)) => "uninstall",
        ("update", Some(_)) => "update",
        _ => requested,
    };
    let session = Session {
        mode,
        existing,
        operation: Mutex::new(None),
    };

    let result = tauri::Builder::default()
        .manage(session)
        .invoke_handler(tauri::generate_handler![
            setup_info,
            default_destination,
            check_destination,
            browse_destination,
            start_install,
            start_uninstall,
            operation_status,
            finish_operation,
        ])
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
    let message = format!("安装向导未能启动。\n\n{detail}");
    eprintln!("{message}");
    dock_platform_windows::show_message(HWND::default(), "Glide-Dock 安装程序", &message, true);
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
    WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
        .title("Glide-Dock 安装程序")
        .inner_size(width, height)
        .min_inner_size(width.min(560.0), height.min(400.0))
        .center()
        .decorations(false)
        .shadow(true)
        .resizable(true)
        .build()?;
    Ok(())
}

fn executable_is_inside(directory: &Path) -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .is_some_and(|parent| same_path(&parent, directory))
}

fn run_from_temporary_copy(args: &[String]) -> io::Result<()> {
    let (copy, _) = create_worker_copy("ui")?;
    let mut command = Command::new(&copy);
    command
        .args(args)
        .arg(UI_COPY_FLAG)
        .creation_flags(0x0800_0000);
    if let Err(error) = command.spawn() {
        let _ = fs::remove_file(copy);
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
        payload_ready: engine::inspect_payload().is_ok(),
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
    if !matches!(session.mode, "install" | "update" | "manage") {
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
fn browse_destination(window: WebviewWindow) -> Result<Option<String>, String> {
    let owner = window
        .hwnd()
        .map_err(|error| format!("无法关联文件夹选择窗口：{error}"))?
        .0 as usize;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("glide-setup-folder-picker".into())
        .spawn(move || {
            let result = choose_folder(HWND(owner as *mut _));
            let _ = sender.send(result);
        })
        .map_err(|error| format!("无法打开文件夹选择器：{error}"))?;
    let selected = receiver
        .recv()
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("无法选择安装位置：{error}"))?;
    let Some(path) = selected else {
        return Ok(None);
    };
    Ok(Some(path))
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
    let path = unsafe { display.to_string() }?;
    unsafe {
        CoTaskMemFree(Some(display.0.cast()));
    }
    Ok(Some(path))
}

#[tauri::command]
fn start_install(
    session: State<'_, Session>,
    update: bool,
    mut options: InstallOptions,
) -> Result<String, String> {
    if session.mode == "uninstall" {
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
        if session.mode == "manage" || session.existing.is_some() {
            return Err("此电脑已有 Glide-Dock 安装，请选择更新或卸载。".into());
        }
        options.update = false;
    }
    let target = engine::resolve_install_target(&options).map_err(|error| error.to_string())?;
    engine::validate_capacity(&target).map_err(|error| error.to_string())?;
    let allowed_apps = engine::scan_installed_apps();
    options.selected_apps.retain(|selected| {
        allowed_apps.iter().any(|allowed| {
            allowed.name == selected.name && same_path(&allowed.path, &selected.path)
        })
    });
    let encoded = encode(&options).map_err(|error| error.to_string())?;
    start_worker(
        &session,
        "install",
        &encoded,
        options.scope == InstallScope::AllUsers,
    )
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
    engine::verify_registered(record).map_err(|error| error.to_string())?;
    let request = UninstallRequest {
        record: record.clone(),
        delete_personal_data,
    };
    let encoded = encode(&request).map_err(|error| error.to_string())?;
    start_worker(
        &session,
        "uninstall",
        &encoded,
        record.scope == InstallScope::AllUsers,
    )
}

fn start_worker(
    session: &Session,
    operation: &str,
    payload: &str,
    elevated: bool,
) -> Result<String, String> {
    let mut active = session
        .operation
        .lock()
        .map_err(|_| "安装状态不可用。".to_string())?;
    if active.is_some() {
        return Err("另一个 Glide-Dock 安装操作正在进行。".into());
    }
    let (copy, token) =
        create_worker_copy("worker").map_err(|error| format!("无法准备安装工作进程：{error}"))?;
    if let Err(error) = write_status(
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
        },
    ) {
        let _ = fs::remove_file(copy);
        return Err(format!("无法准备安装状态：{error}"));
    }
    let arguments = format!("{WORKER_FLAG} {operation} {token} {payload}");
    let spawn_error = if elevated {
        dock_platform_windows::process::spawn_elevated_detached(&copy, &arguments)
            .err()
            .map(|error| {
                let canceled = dock_platform_windows::process::elevation_canceled(&error);
                (error.to_string(), canceled)
            })
    } else {
        Command::new(&copy)
            .args([WORKER_FLAG, operation, &token, payload])
            .creation_flags(0x0800_0000)
            .spawn()
            .err()
            .map(|error| (error.to_string(), false))
    };
    if let Some((error, canceled)) = spawn_error {
        *active = None;
        let _ = fs::remove_file(status_path(&token));
        let _ = fs::remove_file(copy);
        let message = if canceled {
            "你已取消管理员确认，没有更改系统设置。".into()
        } else {
            format!("无法启动安装工作进程：{error}")
        };
        return Err(message);
    }
    *active = Some(token.clone());
    Ok(token)
}

#[derive(Serialize, Deserialize)]
struct UninstallRequest {
    record: InstallRecord,
    delete_personal_data: bool,
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
    let mut current_percent = 1;
    let outcome = match args[1].as_str() {
        "install" => decode::<InstallOptions>(&args[3]).and_then(|options| {
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            engine::install(&options, &executable, |progress| {
                let percent = 4 + (u16::from(progress.percent) * 72 / 100) as u8;
                report_operation_progress(token, &mut current_percent, percent, progress.message);
            })
            .map_err(|error| error.to_string())?;
            finish_install(&options, token, &mut current_percent)
        }),
        "uninstall" => decode::<UninstallRequest>(&args[3]).and_then(|request| {
            let config = dock_storage::config_dir();
            report_operation_progress(token, &mut current_percent, 5, "正在检查登录启动项…");
            startup::remove_for_install(&request.record.install_directory, &config)
                .map_err(|error| format!("无法移除 Glide-Dock 的登录启动项：{error}"))?;
            engine::uninstall(&request.record, |progress| {
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
                engine::remove_user_data()
                    .map_err(|error| format!("程序已卸载，但个人数据清理未完成：{error}"))?;
            }
            Ok(())
        }),
        _ => Err("未知的安装工作类型。".into()),
    };
    match outcome {
        Ok(()) => {
            let message = if args[1] == "uninstall" {
                "Glide-Dock 已卸载。"
            } else {
                "Glide-Dock 已安装。"
            };
            let _ = write_status(
                token,
                OperationStatus {
                    percent: 100,
                    message: message.into(),
                    phase: "complete".into(),
                },
            );
            0
        }
        Err(error) => {
            let _ = write_status(
                token,
                OperationStatus {
                    percent: current_percent.min(99),
                    message: error,
                    phase: "failed".into(),
                },
            );
            1
        }
    }
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
        startup::enable(
            options.startup_mode,
            &executable,
            &dock_storage::config_dir(),
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

fn create_worker_copy(kind: &str) -> io::Result<(PathBuf, String)> {
    let token = format!("{}-{}", std::process::id(), unique_nanos());
    let directory = std::env::temp_dir();
    let target = directory.join(format!("Glide-Dock-Setup-{kind}-{token}.exe"));
    let mut source = File::open(std::env::current_exe()?)?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)?;
    if let Err(error) = io::copy(&mut source, &mut destination).and_then(|_| destination.flush()) {
        drop(destination);
        let _ = fs::remove_file(&target);
        return Err(error);
    }
    Ok((target, token))
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
