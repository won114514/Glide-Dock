//! Shell integration: shortcut resolution, launching and Explorer reveal.

use std::path::Path;

use dock_core::models::{
    DockItem, ItemKind, LaunchType, file_extension, file_stem, is_packaged_target, is_url,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{TOKEN_DUPLICATE, TOKEN_QUERY};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetCurrentProcess, OpenProcessToken,
    PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_LWIN, keybd_event};
use windows::Win32::UI::Shell::{
    AO_NONE, IApplicationActivationManager, IShellLinkW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
    SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, SW_SHOWNORMAL};
use windows::core::{GUID, Interface, PCWSTR};

use crate::window::wide;

/// `CLSID_ShellLink` — not exported by the `windows` crate.
const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
/// `CLSID_ApplicationActivationManager`.
const CLSID_APPLICATION_ACTIVATION_MANAGER: GUID =
    GUID::from_u128(0x45BA127D_10A8_46EA_8AB7_56EA9078943C);

/// A path resolved into dock entry fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEntry {
    /// Entry kind.
    pub kind: ItemKind,
    /// How the entry should be started.
    pub launch_type: LaunchType,
    /// Display label.
    pub display_name: String,
    /// Resolved target path. Falls back to the dropped path when resolution
    /// fails so the entry is never lost.
    pub target_path: String,
    /// Original `.lnk` path.
    pub shortcut_path: Option<String>,
    /// Shortcut arguments.
    pub arguments: Option<String>,
    /// Shortcut working directory.
    pub working_directory: Option<String>,
}

impl ResolvedEntry {
    /// Converts the entry to a persistable dock item.
    #[must_use]
    pub fn into_item(self, id: u32) -> DockItem {
        DockItem {
            id,
            kind: self.kind,
            launch_type: self.launch_type,
            display_name: self.display_name,
            target_path: self.target_path,
            shortcut_path: self.shortcut_path,
            arguments: self.arguments,
            working_directory: self.working_directory,
        }
    }

    /// Creates a packaged-app entry from a scan result.
    #[must_use]
    pub fn packaged_app(name: impl Into<String>, aumid: impl Into<String>) -> Self {
        Self {
            kind: ItemKind::Application,
            launch_type: LaunchType::PackagedApp,
            display_name: name.into(),
            target_path: aumid.into(),
            shortcut_path: None,
            arguments: None,
            working_directory: None,
        }
    }
}

/// Resolves a dropped path into an entry. Never returns `None` for a non-empty
/// path: unresolvable shortcuts fall back to the shortcut itself.
#[must_use]
pub fn resolve(path: &str) -> ResolvedEntry {
    let path = path.trim().trim_matches('"').to_string();
    if path.is_empty() {
        return ResolvedEntry {
            kind: ItemKind::File,
            launch_type: LaunchType::File,
            display_name: "Unknown".to_string(),
            target_path: String::new(),
            shortcut_path: None,
            arguments: None,
            working_directory: None,
        };
    }

    let extension = extension(&path);
    let resolved = if extension == "lnk" {
        resolve_shortcut(&path)
    } else {
        None
    };

    if let Some(shortcut) = resolved {
        let raw_target = if shortcut.target.is_empty() {
            path.clone()
        } else {
            expand_environment(&shortcut.target)
        };
        let target = strip_apps_folder(&raw_target);
        let kind = if is_packaged_target(&target) {
            ItemKind::Application
        } else {
            classify(&target)
        };
        let launch_type = infer_launch_type(&target, kind, Some(&path));
        return ResolvedEntry {
            kind,
            launch_type,
            display_name: display_name(&path, true),
            target_path: target,
            shortcut_path: Some(path),
            arguments: shortcut.arguments,
            working_directory: shortcut.working_directory,
        };
    }

    let target = strip_apps_folder(&path);
    let kind = if is_packaged_target(&target) {
        ItemKind::Application
    } else {
        classify(&target)
    };
    let launch_type = infer_launch_type(&target, kind, None);
    ResolvedEntry {
        kind,
        launch_type,
        display_name: display_name(&path, false),
        target_path: target.clone(),
        shortcut_path: if extension == "lnk" { Some(path) } else { None },
        arguments: None,
        working_directory: None,
    }
}

struct ShortcutTarget {
    target: String,
    arguments: Option<String>,
    working_directory: Option<String>,
}

fn resolve_shortcut(path: &str) -> Option<ShortcutTarget> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(
            &CLSID_SHELL_LINK,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        let wide_path = wide(path);
        persist
            .Load(PCWSTR::from_raw(wide_path.as_ptr()), STGM_READ)
            .ok()?;

        let mut buffer = [0u16; 1024];
        let target = if link.GetPath(&mut buffer, std::ptr::null_mut(), 0).is_ok() {
            from_wide(&buffer)
        } else {
            String::new()
        };
        let mut buffer = [0u16; 1024];
        let arguments = if link.GetArguments(&mut buffer).is_ok() {
            let text = from_wide(&buffer);
            (!text.is_empty()).then_some(text)
        } else {
            None
        };
        let mut buffer = [0u16; 1024];
        let working_directory = if link.GetWorkingDirectory(&mut buffer).is_ok() {
            let text = from_wide(&buffer);
            (!text.is_empty()).then_some(text)
        } else {
            None
        };

        Some(ShortcutTarget {
            target,
            arguments,
            working_directory,
        })
    }
}

/// Launches an item through the appropriate native API. Returns a human
/// readable error.
pub fn launch(item: &DockItem) -> std::result::Result<(), String> {
    match item.infer_launch_type() {
        LaunchType::Exe => launch_exe(item),
        LaunchType::Shortcut => launch_shortcut(item),
        LaunchType::File | LaunchType::Folder | LaunchType::Url => launch_shell(item),
        LaunchType::PackagedApp => launch_packaged(item),
        LaunchType::Auto => launch_shell(item),
    }
}

fn launch_exe(item: &DockItem) -> std::result::Result<(), String> {
    let target = expand_environment(item.target_path.trim());
    if target.is_empty() || !is_executable(&target) {
        return launch_fallback(item);
    }
    let directory = launch_directory(item, false);
    match launch_executable(item, &target, directory.as_deref()) {
        Ok(()) => Ok(()),
        Err(error) => {
            tracing::warn!(%error, path = %target, "direct launch failed; falling back to shell");
            launch_fallback(item).map_err(|fallback| format!("{error}; {fallback}"))
        }
    }
}

fn launch_shortcut(item: &DockItem) -> std::result::Result<(), String> {
    if let Some(shortcut) = item.shortcut().filter(|path| Path::new(path).is_file()) {
        let operation = wide("open");
        let file = wide(shortcut);
        return shell_execute(&operation, &file, &[], None);
    }
    launch_shell(item)
}

fn launch_shell(item: &DockItem) -> std::result::Result<(), String> {
    let file_text = item
        .shortcut()
        .filter(|path| Path::new(path).is_file())
        .unwrap_or_else(|| item.target_path.trim());
    if file_text.is_empty() {
        return Err("目标路径为空".to_string());
    }
    let operation = wide("open");
    let file = wide(file_text);
    let arguments = wide(item.arguments.as_deref().unwrap_or(""));
    let directory = launch_directory(item, item.shortcut().is_some());
    shell_execute(&operation, &file, &arguments, directory.as_deref())
}

fn launch_fallback(item: &DockItem) -> std::result::Result<(), String> {
    if let Some(shortcut) = item.shortcut().filter(|path| Path::new(path).is_file()) {
        let operation = wide("open");
        let file = wide(shortcut);
        return shell_execute(&operation, &file, &[], None);
    }
    launch_shell(item)
}

fn launch_packaged(item: &DockItem) -> std::result::Result<(), String> {
    let aumid = strip_apps_folder(item.target_path.trim());
    if aumid.is_empty() {
        return Err("商店应用缺少 AppUserModelID".to_string());
    }
    let manager: IApplicationActivationManager = match unsafe {
        CoCreateInstance(
            &CLSID_APPLICATION_ACTIVATION_MANAGER,
            None::<&windows::core::IUnknown>,
            CLSCTX_ALL,
        )
    } {
        Ok(manager) => manager,
        Err(error) => return launch_packaged_fallback(item, &aumid, &error.to_string()),
    };
    let aumid_wide = wide(&aumid);
    let arguments = wide(item.arguments.as_deref().unwrap_or(""));
    let activated = unsafe {
        manager.ActivateApplication(
            PCWSTR::from_raw(aumid_wide.as_ptr()),
            PCWSTR::from_raw(arguments.as_ptr()),
            AO_NONE,
        )
    };
    match activated {
        Ok(_pid) => Ok(()),
        Err(error) => {
            if let Some(shortcut) = item.shortcut().filter(|path| Path::new(path).is_file()) {
                tracing::warn!(%error, "packaged activation failed; falling back to shortcut");
                let operation = wide("open");
                let file = wide(shortcut);
                shell_execute(&operation, &file, &[], None)
            } else {
                launch_packaged_fallback(item, &aumid, &error.to_string())
            }
        }
    }
}

/// Low-integrity processes (for example a debug build under a restricted
/// workspace) cannot activate `ApplicationActivationManager`. The shell's
/// virtual Apps folder still starts the same packaged app for them.
fn launch_packaged_fallback(
    item: &DockItem,
    aumid: &str,
    activation_error: &str,
) -> std::result::Result<(), String> {
    if let Some(shortcut) = item.shortcut().filter(|path| Path::new(path).is_file()) {
        let operation = wide("open");
        let file = wide(shortcut);
        return shell_execute(&operation, &file, &[], None);
    }
    let operation = wide("open");
    let path = wide(&format!("shell:AppsFolder\\{aumid}"));
    shell_execute(&operation, &path, &[], None).map_err(|fallback| {
        format!("无法激活商店应用：{activation_error}；回退启动也失败：{fallback}")
    })
}

fn shell_execute(
    operation: &[u16],
    file: &[u16],
    arguments: &[u16],
    directory: Option<&[u16]>,
) -> std::result::Result<(), String> {
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR::from_raw(operation.as_ptr()),
        lpFile: PCWSTR::from_raw(file.as_ptr()),
        lpParameters: if arguments.len() > 1 {
            PCWSTR::from_raw(arguments.as_ptr())
        } else {
            PCWSTR::null()
        },
        lpDirectory: directory.map_or(PCWSTR::null(), |path| PCWSTR::from_raw(path.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };

    unsafe { ShellExecuteExW(&mut info) }.map_err(|error| format!("无法启动：{error}"))
}

fn is_executable(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn launch_executable(
    item: &DockItem,
    target: &str,
    directory: Option<&[u16]>,
) -> std::result::Result<(), String> {
    let application = wide(target);
    let mut command_line = format!("\"{target}\"");
    if let Some(arguments) = item.arguments.as_deref() {
        let arguments = arguments.trim();
        if !arguments.is_empty() {
            command_line.push(' ');
            command_line.push_str(arguments);
        }
    }
    let mut command_line: Vec<u16> = command_line.encode_utf16().chain([0]).collect();

    let mut token = HANDLE::default();
    let mut environment = std::ptr::null_mut();
    let has_environment = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut token,
        )
        .is_ok()
            && CreateEnvironmentBlock(&mut environment, Some(token), false).is_ok()
    };

    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: SW_SHOWNORMAL.0 as u16,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    let result = unsafe {
        CreateProcessW(
            PCWSTR::from_raw(application.as_ptr()),
            Some(windows::core::PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT,
            has_environment.then_some(environment.cast()),
            directory.map_or(PCWSTR::null(), |path| PCWSTR::from_raw(path.as_ptr())),
            &startup,
            &mut process,
        )
    };

    if has_environment {
        unsafe {
            let _ = DestroyEnvironmentBlock(environment.cast_const());
        }
    }
    if !token.is_invalid() {
        unsafe {
            let _ = CloseHandle(token);
        }
    }

    match result {
        Ok(()) => {
            unsafe {
                let _ = CloseHandle(process.hThread);
                let _ = CloseHandle(process.hProcess);
            }
            Ok(())
        }
        Err(error) => Err(format!("无法启动：{error}")),
    }
}

/// Chooses a sane working directory so the child does not inherit the dock's
/// own install directory (which can contain unrelated DLLs) or an empty path.
fn launch_directory(item: &DockItem, use_shortcut: bool) -> Option<Vec<u16>> {
    if let Some(directory) = item.working_directory.as_deref() {
        let directory = directory.trim();
        if !directory.is_empty() && Path::new(directory).is_dir() {
            return Some(wide(directory));
        }
        if !directory.is_empty() {
            tracing::debug!(%directory, "shortcut working directory is missing; using target folder");
        }
    }

    if use_shortcut {
        return None;
    }

    let target = item.target_path.trim();
    let target_path = Path::new(target);
    if target_path.is_file() {
        if let Some(parent) = target_path.parent().filter(|parent| parent.is_dir()) {
            return Some(wide(&parent.to_string_lossy()));
        }
    }
    None
}

/// Opens an item's location in Explorer. Files are selected, folders opened.
pub fn reveal(item: &DockItem) -> std::result::Result<(), String> {
    if item.infer_launch_type() == LaunchType::PackagedApp {
        // Packaged apps have no stable file-system folder to select; open the
        // virtual Apps folder instead.
        return open_path("shell:AppsFolder");
    }
    let path = item.target_path.trim();
    if path.is_empty() {
        return Err("目标路径为空".to_string());
    }
    let path_ref = Path::new(path);
    if path_ref.is_dir() {
        return open_path(path);
    }
    let parent = path_ref.parent().unwrap_or_else(|| Path::new(""));
    if !parent.exists() {
        return Err(format!("路径不存在: {}", parent.display()));
    }
    let parameters = format!("/select,\"{}\"", path.replace('"', ""));
    let operation = wide("open");
    let explorer = wide("explorer.exe");
    let parameters = wide(&parameters);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(explorer.as_ptr()),
            PCWSTR::from_raw(parameters.as_ptr()),
            PCWSTR::default(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as isize;
    if code <= 32 {
        Err(format!("打开文件位置失败，错误码 {code}"))
    } else {
        Ok(())
    }
}

/// Opens the Windows Start menu with the native Win key chord.
///
/// This is intentionally not routed through `cmd.exe` / PowerShell and does not
/// require the dock window to be active.
pub fn open_start_menu() -> std::result::Result<(), String> {
    unsafe {
        keybd_event(VK_LWIN.0 as u8, 0, Default::default(), 0);
        keybd_event(VK_LWIN.0 as u8, 0, KEYEVENTF_KEYUP, 0);
    }
    Ok(())
}

/// Opens a folder in Explorer.
pub fn open_path(path: &str) -> std::result::Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径为空".to_string());
    }
    let operation = wide("open");
    let target = wide(path);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(target.as_ptr()),
            PCWSTR::default(),
            PCWSTR::default(),
            SW_SHOW,
        )
    };
    let code = result.0 as isize;
    if code <= 32 {
        Err(format!("打开路径失败，错误码 {code}"))
    } else {
        Ok(())
    }
}

fn classify(path: &str) -> ItemKind {
    if Path::new(path).is_dir() {
        return ItemKind::Folder;
    }
    let extension = extension(path);
    if matches!(
        extension.as_str(),
        "exe" | "com" | "bat" | "cmd" | "msi" | "appref-ms" | "ahk"
    ) {
        ItemKind::Application
    } else {
        ItemKind::File
    }
}

fn display_name(path: &str, is_shortcut: bool) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let name = trimmed
        .rsplit(['\\', '/'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(trimmed);
    if name.is_empty() {
        return path.to_string();
    }
    if is_shortcut || !Path::new(path).is_dir() {
        let stem = file_stem(name);
        if !stem.is_empty() {
            return stem.to_string();
        }
    }
    name.to_string()
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Resolves a target into the native launch strategy.
#[must_use]
fn infer_launch_type(target: &str, kind: ItemKind, shortcut: Option<&str>) -> LaunchType {
    let target = target.trim();
    if is_packaged_target(target) {
        return LaunchType::PackagedApp;
    }
    if is_url(target) {
        return LaunchType::Url;
    }
    let extension = file_extension(target);
    if extension == "exe" || extension == "com" {
        return LaunchType::Exe;
    }
    if extension == "lnk" {
        return LaunchType::Shortcut;
    }
    if kind == ItemKind::Folder {
        return LaunchType::Folder;
    }
    if shortcut.is_some() && target.is_empty() {
        return LaunchType::Shortcut;
    }
    if kind == ItemKind::Application && extension.is_empty() {
        return LaunchType::Shortcut;
    }
    LaunchType::File
}

/// Stores packaged entries as bare AUMIDs so activation and de-duplication do
/// not need to understand the `shell:AppsFolder\` prefix.
#[must_use]
fn strip_apps_folder(target: &str) -> String {
    let trimmed = target.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("shell:appsfolder\\") {
        return trimmed
            .get("shell:AppsFolder\\".len()..)
            .unwrap_or(trimmed)
            .to_string();
    }
    trimmed.to_string()
}

fn from_wide(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

/// Expands `%VAR%` sequences using the process environment. Unknown variables
/// are left untouched.
#[must_use]
pub fn expand_environment(input: &str) -> String {
    if !input.contains('%') {
        return input.to_string();
    }
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                if name.is_empty() {
                    output.push('%');
                } else if let Ok(value) = std::env::var(name) {
                    output.push_str(&value);
                } else {
                    output.push('%');
                    output.push_str(name);
                    output.push('%');
                }
                rest = &after[end + 1..];
            }
            None => {
                output.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComGuard;

    #[test]
    fn environment_expansion_keeps_unknown_variables() {
        let value = expand_environment("%GLIDE_DOCK_DOES_NOT_EXIST%\\file.exe");
        assert_eq!(value, "%GLIDE_DOCK_DOES_NOT_EXIST%\\file.exe");
    }

    #[test]
    fn environment_expansion_uses_known_variables() {
        // `windir` is present on every Windows installation.
        if let Ok(windir) = std::env::var("windir") {
            let value = expand_environment("%windir%\\notepad.exe");
            assert_eq!(value, format!("{windir}\\notepad.exe"));
        }
    }

    #[test]
    fn resolve_plain_exe_keeps_application_kind() {
        let entry = resolve("C:\\Windows\\notepad.exe");
        assert_eq!(entry.kind, ItemKind::Application);
        assert_eq!(entry.launch_type, LaunchType::Exe);
        assert!(entry.shortcut_path.is_none());
        assert_eq!(entry.target_path, "C:\\Windows\\notepad.exe");
        assert_eq!(entry.display_name, "notepad");
    }

    #[test]
    fn resolve_missing_path_keeps_entry() {
        let entry = resolve("C:\\does\\not\\exist\\thing.exe");
        assert_eq!(entry.kind, ItemKind::Application);
        assert_eq!(entry.launch_type, LaunchType::Exe);
        assert_eq!(entry.target_path, "C:\\does\\not\\exist\\thing.exe");
        assert_eq!(entry.display_name, "thing");
    }

    #[test]
    fn resolve_folder_keeps_name() {
        let entry = resolve("C:\\Windows");
        assert_eq!(entry.kind, ItemKind::Folder);
        assert_eq!(entry.launch_type, LaunchType::Folder);
        assert_eq!(entry.display_name, "Windows");
    }

    #[test]
    fn resolved_lnk_uses_direct_exe_launch() {
        let _com = ComGuard::sta();
        let directory = Path::new("target/temp");
        let _ = std::fs::create_dir_all(directory);
        let path = directory.join(format!("glide-dock-launch-type-{}.lnk", std::process::id()));
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER) }
                .expect("shell link");
        let target = wide("C:\\Windows\\System32\\notepad.exe");
        let arguments = wide("--glide-dock-test");
        let directory = wide("C:\\Windows\\System32");
        let path_wide = wide(path.to_string_lossy().as_ref());
        unsafe {
            link.SetPath(PCWSTR::from_raw(target.as_ptr()))
                .expect("target");
            link.SetArguments(PCWSTR::from_raw(arguments.as_ptr()))
                .expect("arguments");
            link.SetWorkingDirectory(PCWSTR::from_raw(directory.as_ptr()))
                .expect("directory");
            let file: IPersistFile = link.cast().expect("IPersistFile");
            file.Save(PCWSTR::from_raw(path_wide.as_ptr()), true)
                .expect("save shortcut");
        }
        let entry = resolve(path.to_string_lossy().as_ref());
        let _ = std::fs::remove_file(&path);
        assert_eq!(entry.launch_type, LaunchType::Exe);
        assert_eq!(entry.kind, ItemKind::Application);
        assert!(entry.shortcut_path.is_some());
        assert_eq!(entry.arguments.as_deref(), Some("--glide-dock-test"));
    }

    #[test]
    fn packaged_aumid_is_recognized() {
        let entry = resolve("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        assert_eq!(entry.kind, ItemKind::Application);
        assert_eq!(entry.launch_type, LaunchType::PackagedApp);
        assert_eq!(
            entry.target_path,
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        );
    }

    #[test]
    fn activation_manager_can_be_created() {
        let _com = ComGuard::sta();
        if crate::system::is_low_integrity() {
            // AAM denies low-integrity callers; `launch_packaged` falls back to
            // shell activation in that case.
            return;
        }
        let manager: IApplicationActivationManager = unsafe {
            CoCreateInstance(
                &CLSID_APPLICATION_ACTIVATION_MANAGER,
                None::<&windows::core::IUnknown>,
                CLSCTX_ALL,
            )
        }
        .expect("ApplicationActivationManager");
        drop(manager);
    }
}
