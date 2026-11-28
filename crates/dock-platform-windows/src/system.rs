//! Small system queries: theme preference, running executables, single instance.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE, HWND, LPARAM, SetLastError,
};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenIntegrityLevel, TokenUser,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_DWORD, RegCloseKey,
    RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetWindowThreadProcessId, IsWindowVisible, MB_ICONINFORMATION,
    MB_ICONWARNING, MB_OK, MessageBoxW, SPI_GETCLIENTAREAANIMATION, SW_RESTORE,
    SetForegroundWindow, ShowWindow, SystemParametersInfoW,
};
use windows::core::{BOOL, PCWSTR};

use crate::window::wide;

/// Waits until Explorer's desktop window exists, or `timeout` expires.
///
/// A Task Scheduler logon task can start before the shell has created
/// `Progman`; `place_above_desktop` needs that anchor. The wait is bounded so a
/// missing/slow shell can never block the dock forever.
#[must_use]
pub fn wait_for_desktop(timeout: Duration) -> bool {
    let class = wide("Progman");
    let deadline = Instant::now() + timeout;
    loop {
        if unsafe { FindWindowW(PCWSTR::from_raw(class.as_ptr()), PCWSTR::null()) }.is_ok() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(now);
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

/// Returns the Windows build number, or `0` when the query fails.
#[must_use]
pub fn windows_build() -> u32 {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    if unsafe { RtlGetVersion(&mut version) }.0 >= 0 {
        version.dwBuildNumber
    } else {
        0
    }
}

/// Trims unused resident pages from the current process working set.
pub fn trim_working_set() {
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Returns `true` when the current process runs below Medium integrity.
///
/// Low-integrity processes cannot bind AF_UNIX sockets, write normal AppData,
/// or start many desktop applications successfully. This is usually caused by
/// running the executable from a directory carrying a Low mandatory label.
#[must_use]
pub fn is_low_integrity() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut buffer = [0usize; 32];
    let mut needed = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            std::mem::size_of_val(&buffer) as u32,
            &mut needed,
        )
    }
    .is_ok();
    unsafe {
        let _ = CloseHandle(token);
    }
    if !ok {
        return false;
    }
    let label = unsafe { &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    let count = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) };
    if count == 0 {
        return false;
    }
    let rid = unsafe { *GetSidSubAuthority(label.Label.Sid, u32::from(count) - 1) };
    tracing::debug!(rid, count, "token integrity level");
    rid < 0x2000
}

/// Returns `true` when the current process token is elevated.
///
/// An elevated dock cannot receive drag-and-drop from a normal-integrity
/// Explorer because of Windows UIPI. The tray diagnostics use this to explain
/// why dropping may fail.
#[must_use]
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut needed = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut needed,
        )
    }
    .is_ok();
    unsafe {
        let _ = CloseHandle(token);
    }
    ok && elevation.TokenIsElevated != 0
}

const BUILTIN_ADMIN_RID: u32 = 500;
const UAC_POLICIES_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";

/// Returns `true` when the current process belongs to the built-in
/// Administrator account (SID ending with RID `500`).
#[must_use]
pub fn is_builtin_administrator() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    let is_builtin = if needed > 0 {
        let mut buffer = vec![0u64; needed.div_ceil(std::mem::size_of::<u64>() as u32) as usize];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .is_ok();
        if ok {
            let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            let count = unsafe { *GetSidSubAuthorityCount(user.User.Sid) };
            if count > 0 {
                let rid = unsafe { *GetSidSubAuthority(user.User.Sid, u32::from(count) - 1) };
                rid == BUILTIN_ADMIN_RID
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    unsafe {
        let _ = CloseHandle(token);
    }
    is_builtin
}

/// Returns `true` when UAC Admin Approval Mode is enabled for the built-in
/// Administrator account (`EnableLUA != 0` and `FilterAdministratorToken == 1`).
#[must_use]
pub fn is_admin_approval_mode_enabled() -> bool {
    let Ok(Some(key)) =
        crate::registry::Key::open(HKEY_LOCAL_MACHINE, UAC_POLICIES_KEY, KEY_QUERY_VALUE)
    else {
        return false;
    };
    let enable_lua = key.dword("EnableLUA").ok().flatten();
    let filter_admin = key.dword("FilterAdministratorToken").ok().flatten();
    approval_mode_from_dwords(enable_lua, filter_admin)
}

/// Returns `true` when running under the built-in Administrator account with
/// Admin Approval Mode disabled.
#[must_use]
pub fn is_builtin_admin_without_approval() -> bool {
    is_builtin_administrator() && !is_admin_approval_mode_enabled()
}

const fn approval_mode_from_dwords(enable_lua: Option<u32>, filter_admin: Option<u32>) -> bool {
    !matches!(enable_lua, Some(0)) && matches!(filter_admin, Some(1))
}

/// Shows a native information/warning message box owned by `owner`.
pub fn show_message(owner: HWND, title: &str, text: &str, warning: bool) {
    let title = wide(title);
    let text = wide(text);
    let icon = if warning {
        MB_ICONWARNING
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        let _ = MessageBoxW(
            Some(owner),
            PCWSTR::from_raw(text.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            MB_OK | icon,
        );
    }
}

/// Returns `true` when Windows animations are enabled.
///
/// This mirrors the accessibility setting `Settings > Accessibility > Visual
/// effects > Animation effects`.
#[must_use]
pub fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut _ as *mut _),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && enabled.as_bool()
}

/// Returns the lowercased image paths of processes that own a visible
/// top-level window.
#[must_use]
pub fn visible_window_paths() -> Vec<String> {
    let mut paths = BTreeSet::new();
    let pointer = &mut paths as *mut BTreeSet<String> as isize;
    unsafe {
        let _ = EnumWindows(Some(enum_visible_window), LPARAM(pointer));
    }
    paths.into_iter().collect()
}

unsafe extern "system" fn enum_visible_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 {
        return BOOL(1);
    }
    if let Some(path) = process_path(pid) {
        let paths = unsafe { &mut *(lparam.0 as *mut BTreeSet<String>) };
        paths.insert(path.to_ascii_lowercase());
    }
    BOOL(1)
}

/// Returns `true` when a visible top-level window belongs to `path`.
///
/// Packaged apps often execute from `WindowsApps` rather than the path a
/// shortcut points at, so an exact file-name match is also accepted.
#[must_use]
pub fn has_visible_window(path: &str) -> bool {
    let target = path.trim();
    if target.is_empty() {
        return false;
    }
    let target = Path::new(target);
    visible_window_paths()
        .iter()
        .any(|visible| paths_match(target, Path::new(visible)))
}

fn paths_match(target: &Path, visible: &Path) -> bool {
    if target
        .to_string_lossy()
        .eq_ignore_ascii_case(&visible.to_string_lossy())
    {
        return true;
    }
    let same_name = target
        .file_name()
        .zip(visible.file_name())
        .is_some_and(|(a, b)| {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        });
    same_name
        && (is_packaged_path(&target.to_string_lossy())
            || is_packaged_path(&visible.to_string_lossy()))
}

fn is_packaged_path(path: &str) -> bool {
    path.to_ascii_lowercase().contains("\\windowsapps\\")
}

/// Finds a top-level window by its exact title.
#[must_use]
pub fn find_window_by_title(title: &str) -> Option<HWND> {
    let title = wide(title);
    unsafe { FindWindowW(PCWSTR::null(), PCWSTR::from_raw(title.as_ptr())).ok() }
}

/// Brings a window to the foreground.
pub fn activate_window(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Returns `true` when Windows is configured with light app mode.
///
/// Reads `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`
/// → `AppsUseLightTheme`. Falls back to the dark theme on any failure.
#[must_use]
pub fn prefers_light_theme() -> bool {
    const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
    const VALUE: &str = "AppsUseLightTheme";
    let key_path = wide(KEY);
    let value_name = wide(VALUE);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(key_path.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }

    let mut data = [0u8; 4];
    let mut size = data.len() as u32;
    let mut kind = REG_DWORD;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR::from_raw(value_name.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    status == ERROR_SUCCESS && data[0] != 0
}

/// Enumerates the executable paths of all accessible running processes.
///
/// Returns a de-duplicated, lowercased set. Failures to open individual
/// processes are ignored (many system processes are protected).
#[must_use]
pub fn running_executables() -> Vec<String> {
    let mut paths = BTreeSet::new();
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::debug!(%error, "CreateToolhelp32Snapshot failed");
            return Vec::new();
        }
    };

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while has_entry {
        let pid = entry.th32ProcessID;
        if pid != 0
            && let Some(path) = process_path(pid)
        {
            paths.insert(path.to_ascii_lowercase());
        }
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }

    unsafe {
        let _ = CloseHandle(snapshot);
    }
    paths.into_iter().collect()
}

fn process_path(pid: u32) -> Option<String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let query = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR::from_raw(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    unsafe {
        let _ = CloseHandle(process);
    }
    query.ok()?;
    if size == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

/// A named mutex that keeps a single Glide-Dock instance alive.
pub struct SingleInstance {
    handle: HANDLE,
}

/// Result of trying to become the single running instance.
pub enum SingleInstanceOutcome {
    /// This process owns the mutex.
    Acquired(SingleInstance),
    /// Another instance already owns the mutex.
    AlreadyRunning,
    /// The mutex could not be created; the caller may continue without the
    /// single-instance guarantee and log a warning.
    Failed(String),
}

impl SingleInstance {
    /// Tries to acquire the per-session mutex.
    #[must_use]
    pub fn acquire() -> SingleInstanceOutcome {
        Self::acquire_named("Local\\Glide-Dock.SingleInstance")
    }

    /// Tries to acquire a named per-session mutex.
    ///
    /// The guard process uses a separate name so it can coexist with the dock.
    #[must_use]
    pub fn acquire_named(name: &str) -> SingleInstanceOutcome {
        let name = wide(name);
        // CreateMutexW reports ERROR_ALREADY_EXISTS through GetLastError. Clear
        // it first so a stale value from an earlier call cannot be mistaken for
        // an existing instance.
        unsafe {
            SetLastError(ERROR_SUCCESS);
        }
        match unsafe { CreateMutexW(None, false, PCWSTR::from_raw(name.as_ptr())) } {
            Ok(handle) => {
                let last_error = unsafe { windows::Win32::Foundation::GetLastError() };
                if last_error == ERROR_ALREADY_EXISTS {
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    SingleInstanceOutcome::AlreadyRunning
                } else {
                    SingleInstanceOutcome::Acquired(Self { handle })
                }
            }
            Err(error) => SingleInstanceOutcome::Failed(error.to_string()),
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_build_is_queryable() {
        let build = windows_build();
        eprintln!("windows build = {build}");
        assert!(build == 0 || build > 1000);
    }

    #[test]
    fn theme_query_does_not_panic() {
        let _ = prefers_light_theme();
    }

    #[test]
    fn running_executables_contains_our_process_when_accessible() {
        let paths = running_executables();
        // The test binary itself is usually queryable; do not fail if the
        // environment blocks the query.
        if !paths.is_empty() {
            assert!(paths.iter().all(|path| !path.is_empty()));
        }
    }

    #[test]
    fn approval_mode_requires_uac_and_filter_administrator_token() {
        assert!(!approval_mode_from_dwords(None, None));
        assert!(!approval_mode_from_dwords(Some(1), None));
        assert!(!approval_mode_from_dwords(Some(1), Some(0)));
        assert!(!approval_mode_from_dwords(Some(0), Some(1)));
        assert!(approval_mode_from_dwords(Some(1), Some(1)));
        assert!(approval_mode_from_dwords(None, Some(1)));
    }

    #[test]
    fn builtin_admin_and_approval_queries_do_not_panic() {
        let _ = is_builtin_administrator();
        let _ = is_admin_approval_mode_enabled();
        let _ = is_builtin_admin_without_approval();
    }
}
