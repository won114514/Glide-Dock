//! Setup, upgrade, and uninstall operations, independent of the Tauri window.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf, Prefix};

use dock_core::models::{DockConfig, DockItem, ItemKind, LaunchType};
use dock_core::theme::ThemeMode;
use dock_platform_windows::registry::Key;
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, GetDiskFreeSpaceExW};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_ProgramFiles,
    FOLDERID_Programs, FOLDERID_PublicDesktop, FOLDERID_RoamingAppData, FOLDERID_Windows,
    IShellLinkW, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, PostMessageW, WM_CLOSE};
use windows::core::{GUID, Interface, PCWSTR, PWSTR};

use self::embedded_payload::{EMBEDDED_PAYLOAD, EMBEDDED_UNINSTALLER};
use zip::ZipArchive;

const PRODUCT: &str = "Glide-Dock";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Glide-Dock";
const LEGACY_UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C6F1D7A-4B23-4D8E-9C1A-6E2F0A7B3C51}_is1";
pub const UNINSTALL_EXE: &str = "Glide-Dock 卸载程序.exe";
const MINIMUM_FREE_SPACE: u64 = 256 * 1024 * 1024;

mod embedded_payload {
    include!(concat!(env!("OUT_DIR"), "/embedded_payload.rs"));
}

fn payload_bytes() -> &'static [u8] {
    #[cfg(all(test, not(glide_uninstaller)))]
    if flow_tests::sandbox_active() {
        return flow_tests::payload();
    }
    EMBEDDED_PAYLOAD
}

fn uninstaller_bytes() -> &'static [u8] {
    #[cfg(all(test, not(glide_uninstaller)))]
    if flow_tests::sandbox_active() {
        return b"MZ-uninstaller-fixture";
    }
    EMBEDDED_UNINSTALLER
}

/// File/registry scope for an installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallScope {
    /// Install for the account that launched Setup.
    CurrentUser,
    /// Install files and uninstall registration for the whole computer.
    AllUsers,
}

impl InstallScope {
    pub fn id(self) -> &'static str {
        match self {
            Self::CurrentUser => "current-user",
            Self::AllUsers => "all-users",
        }
    }
}

/// Choices made in the installation wizard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallOptions {
    pub scope: InstallScope,
    pub install_directory: PathBuf,
    pub create_desktop_shortcut: bool,
    pub enable_startup: bool,
    pub startup_mode: dock_platform_windows::startup::StartupMode,
    pub launch_after_install: bool,
    pub theme: ThemeMode,
    pub selected_apps: Vec<RecommendedApp>,
    pub update: bool,
}

/// An optional, already installed application that can be added to the Dock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommendedApp {
    pub name: String,
    pub path: PathBuf,
    pub source: String,
}

/// Registry and file-system metadata for a discovered installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    pub scope: InstallScope,
    #[serde(default)]
    pub version: Option<String>,
    pub install_directory: PathBuf,
}

/// A user-visible installation progress update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProgress {
    pub percent: u8,
    pub message: String,
}

/// Environment preflight result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreflightCheck {
    pub name: &'static str,
    pub message: String,
    pub passed: bool,
}

/// The embedded archive's size and contents summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadInfo {
    pub files: usize,
    pub uncompressed_bytes: u64,
}

/// Suggested installation location for the selected scope.
pub fn default_install_directory(scope: InstallScope) -> io::Result<PathBuf> {
    let base = match scope {
        InstallScope::CurrentUser => known_folder(&FOLDERID_LocalAppData)?.join("Programs"),
        InstallScope::AllUsers => known_folder(&FOLDERID_ProgramFiles)?,
    };
    Ok(base.join(PRODUCT))
}

/// Reads the embedded payload index without extracting any files.
pub fn installer_ready() -> bool {
    !super::UNINSTALL_ONLY && !uninstaller_bytes().is_empty() && inspect_payload().is_ok()
}

pub fn update_eligibility(record: &InstallRecord) -> super::version::UpdateEligibility {
    super::version::check(record.version.as_deref(), env!("CARGO_PKG_VERSION"))
}

pub fn ensure_update_allowed(options: &InstallOptions) -> io::Result<()> {
    if super::UNINSTALL_ONLY {
        return Err(io::Error::other("卸载程序不支持安装或更新。"));
    }
    let current = find_install();
    if !options.update {
        if current.is_some() {
            return Err(io::Error::other("安装记录已变更，请重新打开新版安装程序。"));
        }
        return Ok(());
    }
    let record = current.ok_or_else(|| io::Error::other("安装记录已变更，请重新打开安装程序。"))?;
    if record.scope != options.scope
        || !equal_path(&record.install_directory, &options.install_directory)
    {
        return Err(io::Error::other("更新位置已改变，请重新打开安装程序。"));
    }
    let eligibility = update_eligibility(&record);
    if !eligibility.allowed {
        return Err(io::Error::other(eligibility.message));
    }
    Ok(())
}

pub fn inspect_payload() -> zip::result::ZipResult<PayloadInfo> {
    let mut archive = ZipArchive::new(Cursor::new(payload_bytes()))?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut has_dock = false;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let path = payload_path(&entry)?;
        if path.components().count() == 1
            && path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("glide-dock.exe"))
        {
            has_dock = true;
        }
        files += 1;
        bytes = bytes
            .checked_add(entry.size())
            .ok_or(zip::result::ZipError::InvalidArchive(
                "payload size overflow",
            ))?;
    }
    if !has_dock {
        return Err(zip::result::ZipError::InvalidArchive(
            "Glide-Dock executable is missing",
        ));
    }
    Ok(PayloadInfo {
        files,
        uncompressed_bytes: bytes,
    })
}

fn platform_supported() -> bool {
    std::env::consts::ARCH == "x86_64" && dock_platform_windows::windows_build() >= 17_763
}

/// Checks basic platform and payload requirements for Setup.
pub fn check_environment() -> Vec<PreflightCheck> {
    let os_ok = platform_supported();
    let mut checks = vec![PreflightCheck {
        name: "系统兼容性",
        message: if os_ok {
            "Windows 10 1809 或更高版本，64 位系统。".into()
        } else {
            "需要 Windows 10 1809 或更高版本的 64 位系统。".into()
        },
        passed: os_ok,
    }];
    if super::UNINSTALL_ONLY {
        return checks;
    }
    match inspect_payload() {
        Ok(payload) => checks.push(PreflightCheck {
            name: "安装文件",
            message: format!(
                "程序文件可读取，共 {} 个文件（{:.1} MiB）。",
                payload.files,
                payload.uncompressed_bytes as f64 / 1_048_576.0
            ),
            passed: true,
        }),
        Err(error) => checks.push(PreflightCheck {
            name: "安装文件",
            message: if EMBEDDED_PAYLOAD.is_empty() {
                "此开发版本未包含程序文件。请使用打包脚本生成完整安装包。".into()
            } else {
                format!("安装包无法读取：{error}")
            },
            passed: false,
        }),
    }
    if uninstaller_bytes().is_empty() {
        checks.push(PreflightCheck {
            name: "卸载程序",
            message: "此构建未包含卸载程序，请使用正式打包脚本生成安装包。".into(),
            passed: false,
        });
    }
    checks
}

/// Rejects root/system locations and non-empty directories for a new install.
pub fn validate_install_directory(directory: &Path, update: bool) -> io::Result<PathBuf> {
    let full = local_path(directory)?;
    for folder in [
        FOLDERID_Windows,
        FOLDERID_RoamingAppData,
        FOLDERID_LocalAppData,
    ] {
        let protected = known_folder(&folder)?;
        let protected = if folder == FOLDERID_Windows {
            protected
        } else {
            protected.join(PRODUCT)
        };
        if is_at_or_below(&full, &protected) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "请选择没有程序或个人数据的独立文件夹。",
            ));
        }
    }
    existing_ancestor(&full)?;
    if full.exists() && !update && fs::read_dir(&full)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "新安装只能使用空文件夹。",
        ));
    }
    Ok(full)
}

fn local_path(directory: &Path) -> io::Result<PathBuf> {
    let mut parts = directory.components();
    if let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) =
        (parts.next(), parts.next())
        && let Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) = prefix.kind()
    {
        let mut result = PathBuf::from(format!("{}:\\", char::from(drive)));
        for part in parts {
            if let Component::Normal(name) = part
                && safe_component(name)
            {
                result.push(name);
            } else {
                return Err(invalid_directory());
            }
        }
        if result.file_name().is_some() {
            return Ok(result);
        }
    }
    Err(invalid_directory())
}

fn payload_path(entry: &zip::read::ZipFile<'_>) -> zip::result::ZipResult<PathBuf> {
    let path = entry
        .enclosed_name()
        .ok_or(zip::result::ZipError::InvalidArchive("unsafe payload path"))?;
    if path.components().any(|part| match part {
        Component::Normal(name) => !safe_component(name),
        Component::CurDir => false,
        _ => true,
    }) {
        return Err(zip::result::ZipError::InvalidArchive(
            "unsafe payload path component",
        ));
    }
    Ok(path.to_path_buf())
}

fn invalid_directory() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "请选择本机磁盘上的文件夹，不能使用磁盘根目录或相对路径。",
    )
}

fn safe_component(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !name.ends_with([' ', '.'])
        && !name
            .chars()
            .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        && !matches!(
            base.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        && !["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}

fn equal_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
}

fn is_at_or_below(path: &Path, root: &Path) -> bool {
    path.ancestors()
        .any(|candidate| equal_path(candidate, root))
}

/// Walks all ancestors, including existing junctions above a non-existent target.
fn existing_ancestor(path: &Path) -> io::Result<&Path> {
    let mut nearest = None;
    for candidate in path.ancestors() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata)
                if metadata.is_dir()
                    && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 =>
            {
                nearest.get_or_insert(candidate);
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "安装位置不能是文件、符号链接或重定向目录。",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    nearest.ok_or_else(invalid_directory)
}

/// Verifies the selected volume has room for the extracted application and rollback copy.
pub fn validate_capacity(directory: &Path) -> io::Result<String> {
    let payload = inspect_payload().map_err(io::Error::other)?;
    let required = payload
        .uncompressed_bytes
        .saturating_add(128 * 1024 * 1024)
        .max(MINIMUM_FREE_SPACE);
    let probe = existing_ancestor(directory)?;
    let wide = wide_path(probe);
    let mut available = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }?;
    if available < required {
        return Err(io::Error::other(format!(
            "磁盘空间不足：可用 {:.1} MiB，建议至少 {:.1} MiB。",
            available as f64 / 1_048_576.0,
            required as f64 / 1_048_576.0
        )));
    }
    Ok(format!(
        "可用 {:.1} GiB，安装需要至少 {} MiB。",
        available as f64 / 1_073_741_824.0,
        required.div_ceil(1_048_576)
    ))
}

/// Finds the registered installation, preferring a machine-wide registration.
pub fn find_install() -> Option<InstallRecord> {
    registered_install(InstallScope::AllUsers)
        .or_else(|| registered_install(InstallScope::CurrentUser))
}

pub fn registered_install(scope: InstallScope) -> Option<InstallRecord> {
    read_install(scope, UNINSTALL_KEY).or_else(|| read_install(scope, LEGACY_UNINSTALL_KEY))
}

pub fn verify_registered(record: &InstallRecord) -> io::Result<PathBuf> {
    let directory = validate_install_directory(&record.install_directory, true)?;
    let matched = registered_install(record.scope)
        .is_some_and(|found| equal_path(&directory, &found.install_directory));
    if !matched {
        return Err(io::Error::other("安装记录已变更。请关闭向导后重试。"));
    }
    Ok(directory)
}

pub fn verify_uninstall_target(record: &InstallRecord) -> io::Result<PathBuf> {
    let directory = validate_install_directory(&record.install_directory, true)?;
    if let Some(found) = registered_install(record.scope) {
        if equal_path(&directory, &found.install_directory) {
            return Ok(directory);
        }
    } else if !directory.try_exists()? {
        // A repeat request after successful cleanup is already complete. Never
        // accept an unregistered directory that could now contain unrelated files.
        return Ok(directory);
    }
    Err(io::Error::other("安装记录已变更。请关闭向导后重试。"))
}

fn registry_root(scope: InstallScope) -> windows::Win32::System::Registry::HKEY {
    #[cfg(all(test, not(glide_uninstaller)))]
    if let Some(root) = flow_tests::registry_override(scope) {
        return root;
    }
    match scope {
        InstallScope::CurrentUser => HKEY_CURRENT_USER,
        InstallScope::AllUsers => HKEY_LOCAL_MACHINE,
    }
}

pub fn resolve_install_target(options: &InstallOptions) -> io::Result<PathBuf> {
    if options.update {
        return verify_registered(&InstallRecord {
            scope: options.scope,
            version: None,
            install_directory: options.install_directory.clone(),
        });
    }
    if find_install().is_some() {
        return Err(io::Error::other(
            "已找到现有安装。请重新打开向导更新该版本。",
        ));
    }
    validate_install_directory(&options.install_directory, false)
}

fn operation_lock() -> io::Result<dock_platform_windows::SingleInstance> {
    use dock_platform_windows::SingleInstanceOutcome;
    match dock_platform_windows::SingleInstance::acquire_named(r"Local\Glide-Dock.Setup.Operation")
    {
        SingleInstanceOutcome::Acquired(guard) => Ok(guard),
        SingleInstanceOutcome::AlreadyRunning => {
            Err(io::Error::other("另一个 Glide-Dock 安装操作正在进行。"))
        }
        SingleInstanceOutcome::Failed(error) => Err(io::Error::other(error)),
    }
}

/// Transactionally extracts and registers the application.
pub fn install(
    options: &InstallOptions,
    mut report: impl FnMut(InstallProgress),
) -> io::Result<InstallRecord> {
    let _operation = operation_lock()?;
    if !platform_supported() {
        return Err(io::Error::other(
            "此版本需要 Windows 10 1809 或更新的 64 位 Windows。",
        ));
    }
    if options.scope == InstallScope::AllUsers && !dock_platform_windows::is_elevated() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "全用户安装需要管理员权限。",
        ));
    }
    ensure_update_allowed(options)?;
    if uninstaller_bytes().is_empty() {
        return Err(io::Error::other("安装包未包含卸载程序。"));
    }
    let directory = resolve_install_target(options)?;
    if super::elevation::is_inside(&std::env::current_exe()?, &directory) {
        return Err(io::Error::other(
            "请从“下载”文件夹或临时目录运行新版安装包。",
        ));
    }
    validate_capacity(&directory)?;
    let parent = directory
        .parent()
        .ok_or_else(|| io::Error::other("invalid install path"))?;
    fs::create_dir_all(parent)?;
    let token = format!("{}-{}", std::process::id(), unique_nanos());
    let stage = parent.join(format!(".Glide-Dock-stage-{token}"));
    let backup = parent.join(format!(".Glide-Dock-backup-{token}"));
    let root = registry_root(options.scope);
    let registration_before = snapshot_registration(root)?;
    let mut old_moved = false;
    let mut new_moved = false;
    let result = (|| {
        report(InstallProgress {
            percent: 2,
            message: "正在验证安装文件…".into(),
        });
        fs::create_dir(&stage)?;
        extract_payload(&stage, &mut report)?;
        fs::write(stage.join(UNINSTALL_EXE), uninstaller_bytes())?;
        report(InstallProgress {
            percent: 83,
            message: "正在关闭 Glide-Dock…".into(),
        });
        request_dock_shutdown(&directory)?;
        ensure_update_allowed(options)?;
        resolve_install_target(options)?;
        if directory.exists() {
            report(InstallProgress {
                percent: 88,
                message: "正在保留旧版本以便回滚…".into(),
            });
            fs::rename(&directory, &backup)?;
            old_moved = true;
        }
        fs::rename(&stage, &directory)?;
        new_moved = true;
        report(InstallProgress {
            percent: 94,
            message: "正在创建快捷方式…".into(),
        });
        create_shortcuts(&directory, options.scope, options.create_desktop_shortcut)?;
        write_registration(&directory, options.scope)?;
        Ok(())
    })();

    if let Err(error) = result {
        restore_registration(root, registration_before.as_ref());
        if new_moved {
            let _ = remove_install_tree(&directory, parent);
        }
        if old_moved && backup.exists() {
            let _ = fs::rename(&backup, &directory);
        }
        let _ = remove_install_tree(&stage, parent);
        return Err(error);
    }
    if old_moved && let Err(error) = remove_install_tree(&backup, parent) {
        tracing::warn!(path = %backup.display(), %error, "installed successfully but could not remove the rollback copy");
    }
    if options.update && registration_matches(options.scope, LEGACY_UNINSTALL_KEY, &directory)? {
        let _ = Key::delete(root, LEGACY_UNINSTALL_KEY);
    }
    report(InstallProgress {
        percent: 100,
        message: "程序文件与快捷方式已准备完成。".into(),
    });
    Ok(InstallRecord {
        scope: options.scope,
        version: Some(env!("CARGO_PKG_VERSION").into()),
        install_directory: directory,
    })
}

/// Removes a registered install tree and its machine/user shortcuts. It never touches user settings.
pub fn uninstall(
    record: &InstallRecord,
    mut report: impl FnMut(InstallProgress),
) -> io::Result<Vec<String>> {
    let _operation = operation_lock()?;
    if record.scope == InstallScope::AllUsers && !dock_platform_windows::is_elevated() {
        return Err(io::Error::other("全用户卸载需要管理员权限。"));
    }
    report(InstallProgress {
        percent: 8,
        message: "正在验证卸载信息…".into(),
    });
    let directory = verify_uninstall_target(record)?;
    let root = registry_root(record.scope);
    let mut matching_keys = Vec::new();
    for key in [UNINSTALL_KEY, LEGACY_UNINSTALL_KEY] {
        if registration_matches(record.scope, key, &directory)? {
            matching_keys.push(key);
        }
    }
    report(InstallProgress {
        percent: 24,
        message: "正在关闭 Glide-Dock…".into(),
    });
    request_dock_shutdown(&directory)?;
    let parent = directory.parent().ok_or_else(invalid_directory)?;
    report(InstallProgress {
        percent: 52,
        message: "正在移除程序文件…".into(),
    });
    let mut warnings = remove_uninstall_tree(&directory, parent)?;
    report(InstallProgress {
        percent: 78,
        message: "正在移除快捷方式…".into(),
    });
    if let Err(error) = remove_shortcuts(record.scope) {
        warnings.push(format!("程序已卸载，但部分快捷方式未能清理：{error}"));
    }
    report(InstallProgress {
        percent: 92,
        message: "正在清理卸载记录…".into(),
    });
    for key in matching_keys {
        if let Err(error) = Key::delete(root, key) {
            warnings.push(format!("程序已卸载，但卸载记录未能清理：{error}"));
        }
    }
    Ok(warnings)
}

/// Creates a fresh config only when the current user's config does not exist.
pub fn create_first_run_config(
    theme: dock_core::theme::ThemeMode,
    apps: &[RecommendedApp],
) -> io::Result<()> {
    let path = dock_storage::config_dir().join("config.json");
    if path.exists() {
        return Ok(());
    }
    let mut config = DockConfig::default();
    config.appearance.theme = theme;
    for app in apps {
        if app.path.is_file() {
            let mut item = DockItem::application(
                config.items.len() as u32 + 1,
                app.name.clone(),
                app.path.to_string_lossy().into_owned(),
            );
            item.launch_type = LaunchType::Auto;
            item.normalize();
            config.items.push(item);
        }
    }
    config.next_item_id = config.items.len() as u32 + 1;
    dock_storage::ConfigStore::new(path)
        .save(&config)
        .map_err(io::Error::other)
}

/// Scans Start Menu and desktop shortcuts for launchable applications.
pub fn scan_installed_apps() -> Vec<RecommendedApp> {
    std::thread::Builder::new()
        .name("glide-setup-app-scan".into())
        .spawn(scan_installed_apps_sta)
        .and_then(|worker| {
            worker
                .join()
                .map_err(|_| io::Error::other("应用扫描线程意外退出。"))
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not scan installed applications");
            Vec::new()
        })
}

fn scan_installed_apps_sta() -> Vec<RecommendedApp> {
    let _com = dock_platform_windows::ComGuard::sta();
    let mut apps = Vec::new();
    let mut names = HashSet::new();
    for (folder, source) in [
        (&FOLDERID_Programs, "当前账户"),
        (&FOLDERID_Desktop, "当前账户桌面"),
        (&FOLDERID_CommonPrograms, "这台电脑"),
        (&FOLDERID_PublicDesktop, "公共桌面"),
    ] {
        if let Ok(directory) = known_folder(folder) {
            collect_application_shortcuts(&directory, source, 0, &mut names, &mut apps);
        }
    }

    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    for (name, path) in [
        ("文件资源管理器", windows.join("explorer.exe")),
        (
            "Microsoft Edge",
            [
                program_files.join(r"Microsoft\Edge\Application\msedge.exe"),
                program_files_x86.join(r"Microsoft\Edge\Application\msedge.exe"),
            ]
            .into_iter()
            .find(|path| path.is_file())
            .unwrap_or_default(),
        ),
        ("计算器", windows.join(r"System32\calc.exe")),
    ] {
        if path.is_file() && names.insert(name.to_lowercase()) {
            apps.push(RecommendedApp {
                name: name.into(),
                path,
                source: "Windows".into(),
            });
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

fn collect_application_shortcuts(
    directory: &Path,
    source: &str,
    depth: usize,
    names: &mut HashSet<String>,
    apps: &mut Vec<RecommendedApp>,
) {
    if depth > 6 || apps.len() >= 300 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if apps.len() >= 300 {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_application_shortcuts(&path, source, depth + 1, names, apps);
            continue;
        }
        if !file_type.is_file()
            || !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            continue;
        }
        let resolved = dock_platform_windows::resolve(&path.to_string_lossy());
        if resolved.kind != ItemKind::Application
            || (resolved.launch_type != LaunchType::PackagedApp
                && !Path::new(&resolved.target_path).is_file())
        {
            continue;
        }
        let name = resolved.display_name.trim();
        if name.is_empty() || !names.insert(name.to_lowercase()) {
            continue;
        }
        apps.push(RecommendedApp {
            name: name.to_owned(),
            path,
            source: source.into(),
        });
    }
}

fn extract_payload(destination: &Path, report: &mut impl FnMut(InstallProgress)) -> io::Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(payload_bytes())).map_err(io::Error::other)?;
    let total = archive.file_names().count().max(1);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(io::Error::other)?;
        let relative = payload_path(&entry).map_err(io::Error::other)?;
        let destination_path = destination.join(relative);
        if !destination_path.starts_with(destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "安装包包含无效的文件路径。",
            ));
        }
        if entry.is_dir() {
            fs::create_dir_all(destination_path)?;
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "安装包不能包含符号链接。",
            ));
        }
        let parent = destination_path
            .parent()
            .ok_or_else(|| io::Error::other("invalid payload path"))?;
        fs::create_dir_all(parent)?;
        let mut file = File::create(&destination_path)?;
        let declared = entry.size();
        let mut copied = 0u64;
        let mut buffer = [0; 64 * 1024];
        loop {
            let bytes = entry.read(&mut buffer)?;
            if bytes == 0 {
                break;
            }
            copied = copied
                .checked_add(bytes as u64)
                .ok_or_else(|| io::Error::other("extracted file size overflow"))?;
            if copied > declared {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "安装文件超过了压缩包声明的大小。",
                ));
            }
            file.write_all(&buffer[..bytes])?;
        }
        if copied != declared {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "安装文件大小与压缩包声明不一致。",
            ));
        }
        report(InstallProgress {
            percent: 5 + (index * 75 / total) as u8,
            message: format!("正在释放程序文件（{}/{total}）…", index + 1),
        });
    }
    Ok(())
}

type RegistrySnapshot = Vec<(&'static str, Option<dock_platform_windows::registry::Value>)>;

fn snapshot_registration(
    root: windows::Win32::System::Registry::HKEY,
) -> io::Result<Option<RegistrySnapshot>> {
    let Some(key) = Key::open(root, UNINSTALL_KEY, KEY_QUERY_VALUE)? else {
        return Ok(None);
    };
    let mut values = Vec::new();
    for name in [
        "DisplayName",
        "DisplayVersion",
        "Publisher",
        "InstallLocation",
        "DisplayIcon",
        "UninstallString",
        "QuietUninstallString",
        "OwnerSid",
        "InstallScope",
        "NoModify",
        "NoRepair",
    ] {
        values.push((name, key.read(name)?));
    }
    Ok(Some(values))
}

fn restore_registration(
    root: windows::Win32::System::Registry::HKEY,
    before: Option<&RegistrySnapshot>,
) {
    let Ok(key) = Key::create(root, UNINSTALL_KEY) else {
        return;
    };
    if let Some(before) = before {
        for (name, value) in before {
            if let Some(value) = value {
                let _ = key.write(name, value);
            } else {
                let _ = key.remove(name);
            }
        }
    } else {
        drop(key);
        let _ = Key::delete(root, UNINSTALL_KEY);
    }
}

fn write_registration(directory: &Path, scope: InstallScope) -> io::Result<()> {
    let root = registry_root(scope);
    let key = Key::create(root, UNINSTALL_KEY)?;
    let exe = directory.join("glide-dock.exe");
    let setup = directory.join(UNINSTALL_EXE);
    key.set_text("DisplayName", PRODUCT)?;
    key.set_text("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    key.set_text("Publisher", "Glide-Dock contributors")?;
    key.set_text("InstallLocation", &directory.to_string_lossy())?;
    key.set_text("DisplayIcon", &exe.to_string_lossy())?;
    key.set_text(
        "UninstallString",
        &format!("\"{}\" --uninstall", setup.display()),
    )?;
    key.set_text(
        "QuietUninstallString",
        &format!("\"{}\" --uninstall", setup.display()),
    )?;
    key.set_text("InstallScope", scope.id())?;
    key.set_dword("NoModify", 1)?;
    key.set_dword("NoRepair", 1)
}

fn read_install(scope: InstallScope, key_path: &str) -> Option<InstallRecord> {
    let key = Key::open(registry_root(scope), key_path, KEY_QUERY_VALUE).ok()??;
    let path = key.text("InstallLocation").ok()??;
    let directory = PathBuf::from(path);
    // A failed cleanup may already have removed the main executable. The registry
    // remains authoritative until uninstall finishes, so these installs can resume.
    Some(InstallRecord {
        scope,
        version: key.text("DisplayVersion").ok().flatten(),
        install_directory: directory,
    })
}

fn registration_matches(scope: InstallScope, key_path: &str, directory: &Path) -> io::Result<bool> {
    let Some(key) = Key::open(registry_root(scope), key_path, KEY_QUERY_VALUE)? else {
        return Ok(false);
    };
    let Some(path) = key.text("InstallLocation")? else {
        return Ok(false);
    };
    Ok(equal_path(Path::new(&path), directory))
}

fn create_shortcuts(directory: &Path, scope: InstallScope, desktop: bool) -> io::Result<()> {
    let programs_id = if scope == InstallScope::AllUsers {
        &FOLDERID_CommonPrograms
    } else {
        &FOLDERID_Programs
    };
    let menu = known_folder(programs_id)?.join(PRODUCT);
    fs::create_dir_all(&menu)?;
    let dock = directory.join("glide-dock.exe");
    let icon = directory.join("assets/brand/03-Windows-ICO/Glide-Dock-FullColor.ico");
    create_shortcut(&dock, &icon, &menu.join("Glide-Dock.lnk"))?;
    if desktop {
        let folder = if scope == InstallScope::AllUsers {
            &FOLDERID_PublicDesktop
        } else {
            &FOLDERID_Desktop
        };
        create_shortcut(&dock, &icon, &known_folder(folder)?.join("Glide-Dock.lnk"))?;
    }
    Ok(())
}

fn remove_shortcuts(scope: InstallScope) -> io::Result<()> {
    let mut errors = Vec::new();
    let folders = match scope {
        InstallScope::CurrentUser => [&FOLDERID_Programs, &FOLDERID_Desktop],
        InstallScope::AllUsers => [&FOLDERID_CommonPrograms, &FOLDERID_PublicDesktop],
    };
    for (index, folder) in folders.into_iter().enumerate() {
        let base = match known_folder(folder) {
            Ok(path) => path,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        let group = if index == 0 { base.join(PRODUCT) } else { base };
        for name in ["Glide-Dock.lnk", "Glide-Dock 设置.lnk"] {
            let path = group.join(name);
            if let Err(error) = fs::remove_file(&path)
                && error.kind() != io::ErrorKind::NotFound
            {
                errors.push(format!("{}：{error}", path.display()));
            }
        }
        if index == 0 && fs::read_dir(&group).is_ok_and(|mut entries| entries.next().is_none()) {
            let _ = fs::remove_dir(group);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("；")))
    }
}

fn create_shortcut(executable: &Path, icon: &Path, destination: &Path) -> io::Result<()> {
    let _com = dock_platform_windows::ComGuard::sta();
    let clsid = GUID::from_u128(0x0002_1401_0000_0000_c000_0000_0000_0046);
    let shell: IShellLinkW = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }
        .map_err(io::Error::other)?;
    let executable = wide_os(executable.as_os_str());
    let icon = wide_os(icon.as_os_str());
    let working = wide_os(executable_path(executable.as_slice()).as_os_str());
    unsafe {
        shell
            .SetPath(PCWSTR(executable.as_ptr()))
            .map_err(io::Error::other)?;
        shell
            .SetWorkingDirectory(PCWSTR(working.as_ptr()))
            .map_err(io::Error::other)?;
        shell
            .SetDescription(PCWSTR(wide("Glide-Dock").as_ptr()))
            .map_err(io::Error::other)?;
        shell
            .SetIconLocation(PCWSTR(icon.as_ptr()), 0)
            .map_err(io::Error::other)?;
        let persist: IPersistFile = shell.cast().map_err(io::Error::other)?;
        persist
            .Save(PCWSTR(wide_os(destination.as_os_str()).as_ptr()), true)
            .map_err(io::Error::other)?;
    }
    Ok(())
}

fn executable_path(path: &[u16]) -> PathBuf {
    let path = String::from_utf16_lossy(path);
    Path::new(path.trim_end_matches('\0'))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn known_folder(id: &GUID) -> io::Result<PathBuf> {
    #[cfg(all(test, not(glide_uninstaller)))]
    if let Some(path) = flow_tests::folder_override(id) {
        return Ok(path);
    }
    let path =
        unsafe { SHGetKnownFolderPath(id, windows::Win32::UI::Shell::KNOWN_FOLDER_FLAG(0), None) }
            .map_err(io::Error::other)?;
    let owned = PathBuf::from(unsafe { path.to_string() }.map_err(io::Error::other)?);
    unsafe {
        CoTaskMemFree(Some(path.0.cast()));
    }
    Ok(owned)
}

fn request_dock_shutdown(install_directory: &Path) -> io::Result<()> {
    for title in [
        "Glide-Dock",
        "Glide-Dock 设置",
        "建议开启内置管理员审批模式",
        "检测到管理员权限",
    ] {
        if let Some(hwnd) = dock_platform_windows::find_window_by_title(title) {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            if pid != 0
                && let Ok(process) =
                    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            {
                let mut buffer = vec![0u16; 32_768];
                let mut length = buffer.len() as u32;
                let ok = unsafe {
                    QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_WIN32,
                        PWSTR(buffer.as_mut_ptr()),
                        &mut length,
                    )
                };
                let _ = unsafe { CloseHandle(process) };
                if ok.is_ok() {
                    let executable =
                        PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
                    if executable
                        .parent()
                        .is_some_and(|parent| is_at_or_below(parent, install_directory))
                    {
                        request_shutdown_window(hwnd)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn request_shutdown_window(hwnd: HWND) -> io::Result<()> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_CLOSE,
            windows::Win32::Foundation::WPARAM(0),
            LPARAM(0),
        )
    }
    .map_err(io::Error::other)?;
    if pid == 0 {
        return Ok(());
    }
    let process = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(io::Error::other)?;
    let _guard = ProcessHandle(process);
    match unsafe { WaitForSingleObject(process, 10_000) } {
        WAIT_OBJECT_0 => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "请先关闭 Glide-Dock 后重试安装。",
        )),
    }
}

fn remove_install_tree(directory: &Path, parent: &Path) -> io::Result<()> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => (),
    }
    let requested = local_path(directory)?;
    let expected_parent = local_path(parent)?;
    if !equal_path(
        requested.parent().ok_or_else(invalid_directory)?,
        &expected_parent,
    ) || !equal_path(existing_ancestor(&expected_parent)?, &expected_parent)
        || requested
            .file_name()
            .is_none_or(|name| !safe_component(name))
    {
        return Err(invalid_directory());
    }
    let metadata = fs::symlink_metadata(&requested)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "拒绝删除重解析目录或非目录路径。",
        ));
    }
    fs::remove_dir_all(requested)
}

fn remaining_install_files(directory: &Path) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Ok(true);
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        // Do not follow junctions or symlinks while classifying a failed removal.
        if !kind.is_dir()
            || fs::symlink_metadata(entry.path())?.file_attributes()
                & FILE_ATTRIBUTE_REPARSE_POINT.0
                != 0
            || remaining_install_files(&entry.path())?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn remove_uninstall_tree(directory: &Path, parent: &Path) -> io::Result<Vec<String>> {
    for attempt in 0..10 {
        match remove_install_tree(directory, parent) {
            Ok(()) => return Ok(Vec::new()),
            Err(error) => {
                if matches!(remaining_install_files(directory), Ok(false)) {
                    return Ok(vec![format!(
                        "程序文件已移除，但空目录暂时无法删除：{}。关闭占用该目录的窗口后可手动删除。{error}",
                        directory.display()
                    )]);
                }
                if attempt == 9 || !matches!(error.raw_os_error(), Some(5 | 32 | 33 | 145)) {
                    return Err(io::Error::other(format!(
                        "仍有程序文件未能移除，卸载尚未完成：{}。已保留卸载记录，关闭占用程序后可直接重试。{error}",
                        directory.display()
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
    unreachable!()
}

fn unique_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or(0)
}

pub fn user_data_directories() -> io::Result<[PathBuf; 2]> {
    Ok([
        known_folder(&FOLDERID_RoamingAppData)?.join(PRODUCT),
        known_folder(&FOLDERID_LocalAppData)?.join(PRODUCT),
    ])
}

pub fn remove_user_data() -> io::Result<()> {
    for directory in user_data_directories()? {
        if !directory.exists() {
            continue;
        }
        let path = local_path(&directory)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(io::Error::other("个人数据目录已被重定向，拒绝自动删除。"));
        }
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn wide_path(path: &Path) -> Vec<u16> {
    wide_os(path.as_os_str())
}

fn wide_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_install_refuses_nonempty_directory() {
        let root =
            std::env::temp_dir().join(format!("glide-installer-validation-{}", unique_nanos()));
        fs::create_dir_all(&root).expect("create test directory");
        fs::write(root.join("keep.txt"), "user data").expect("create sentinel");
        let result = validate_install_directory(&root, false);
        assert_eq!(
            result.expect_err("must refuse non-empty directory").kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            fs::read_to_string(root.join("keep.txt")).expect("sentinel remains"),
            "user data"
        );
        fs::remove_dir_all(root).expect("clean test directory");
    }

    #[test]
    fn paths_outside_a_directory_are_rejected() {
        assert!(!is_at_or_below(
            Path::new(r"D:\Apps\Glide-Dock-other"),
            Path::new(r"D:\Apps\Glide-Dock")
        ));
        assert!(is_at_or_below(
            Path::new(r"D:\Apps\Glide-Dock\assets"),
            Path::new(r"D:\Apps\Glide-Dock")
        ));
    }
}

#[cfg(all(test, not(glide_uninstaller)))]
#[path = "flow_tests.rs"]
mod flow_tests;
