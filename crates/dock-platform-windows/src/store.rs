//! Microsoft Store / MSIX app discovery and a native picker menu.
//!
//! Discovery walks the shell's virtual `AppsFolder`. Every Start-menu app is
//! exposed there; packaged apps carry an AppUserModelID (`Family!AppId`) that
//! can be activated with `IApplicationActivationManager`.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use dock_core::models::is_packaged_target;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, FOLDERID_AppsFolder, IEnumIDList, IShellFolder, IShellItem, KF_FLAG_DEFAULT,
    SHCONTF_FOLDERS, SHCONTF_NONFOLDERS, SHCreateItemWithParent, SHGetKnownFolderItem, SIGDN,
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING,
    PostMessageW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, TPM_WORKAREA, TrackPopupMenuEx,
    WM_APP,
};
use windows::core::PCWSTR;

use crate::com::ComGuard;
use crate::window::wide;

/// Posted to the dock window with a completed store scan.
pub const WM_APP_STORE_APPS: u32 = WM_APP + 5;

/// A packaged app exposed through its AppUserModelID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreApp {
    /// AppUserModelID used by `IApplicationActivationManager`.
    pub aumid: String,
    /// Localized display name from the shell Apps folder.
    pub display_name: String,
}

/// Background Store-app scanner.
pub struct StoreAppService {
    sender: Sender<()>,
    receiver: Receiver<Vec<StoreApp>>,
}

impl StoreAppService {
    /// Spawns the scanner worker. `hwnd` is only used to wake the UI thread.
    #[must_use]
    pub fn spawn(hwnd: HWND) -> Self {
        let (request_sender, request_receiver) = mpsc::channel::<()>();
        let (result_sender, result_receiver) = mpsc::channel::<Vec<StoreApp>>();
        let hwnd_value = hwnd.0 as isize;
        let spawn_result = thread::Builder::new()
            .name("glide-dock-store-apps".to_string())
            .spawn(move || {
                let _com = ComGuard::sta();
                while request_receiver.recv().is_ok() {
                    let apps = match scan_store_apps() {
                        Ok(apps) => apps,
                        Err(error) => {
                            tracing::warn!(%error, "store app scan failed");
                            Vec::new()
                        }
                    };
                    let _ = result_sender.send(apps);
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_value as *mut _)),
                            WM_APP_STORE_APPS,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            });
        if let Err(error) = spawn_result {
            tracing::error!(%error, "failed to spawn store app scanner");
        }
        Self {
            sender: request_sender,
            receiver: result_receiver,
        }
    }

    /// Queues a scan. Results arrive through `WM_APP_STORE_APPS`.
    pub fn request(&self) {
        let _ = self.sender.send(());
    }

    /// Drains completed scans.
    #[must_use]
    pub fn drain(&self) -> Vec<Vec<StoreApp>> {
        self.receiver.try_iter().collect()
    }
}

/// Enumerates packaged apps visible in the Start menu's Apps folder.
pub fn scan_store_apps() -> std::result::Result<Vec<StoreApp>, String> {
    let folder_item: IShellItem =
        unsafe { SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) }
            .map_err(|error| format!("AppsFolder: {error}"))?;
    let folder: IShellFolder = unsafe {
        folder_item
            .BindToHandler(None::<&IBindCtx>, &BHID_SFObject)
            .map_err(|error| format!("AppsFolder shell folder: {error}"))
    }?;

    let mut enumerator: Option<IEnumIDList> = None;
    unsafe {
        folder
            .EnumObjects(
                HWND::default(),
                (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32,
                &mut enumerator,
            )
            .ok()
            .map_err(|error| format!("EnumObjects: {error}"))?;
    }
    let Some(enumerator) = enumerator else {
        return Ok(Vec::new());
    };

    let mut apps = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let mut fetched = 0u32;
        let result =
            unsafe { enumerator.Next(std::slice::from_mut(&mut pidl), Some(&mut fetched)) };
        if result.is_err() || fetched == 0 || pidl.is_null() {
            break;
        }
        if let Ok(item) =
            unsafe { SHCreateItemWithParent::<&IShellFolder, IShellItem>(None, &folder, pidl) }
        {
            collect_item(&item, &mut apps, &mut seen);
        }
        unsafe {
            CoTaskMemFree(Some(pidl.cast::<c_void>().cast_const()));
        }
    }

    apps.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.aumid.cmp(&b.aumid))
    });
    Ok(apps)
}

/// Shows a native popup menu listing the scanned apps. Returns the selected
/// index, or `None` when the menu was dismissed.
#[must_use]
pub fn pick_store_app(hwnd: HWND, apps: &[StoreApp], at: POINT) -> Option<usize> {
    if apps.is_empty() {
        return None;
    }
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    let header = wide(&format!("选择要添加的商店应用（{}）", apps.len()));
    unsafe {
        let _ = AppendMenuW(
            menu,
            MF_STRING | MF_GRAYED | MF_DISABLED,
            0,
            PCWSTR::from_raw(header.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        for (index, app) in apps.iter().enumerate() {
            let label = app.display_name.replace('&', "&&");
            let label = wide(&label);
            let _ = AppendMenuW(menu, MF_STRING, index + 1, PCWSTR::from_raw(label.as_ptr()));
        }
    }
    let selected = unsafe {
        TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_WORKAREA).0,
            at.x,
            at.y,
            hwnd,
            None,
        )
    };
    unsafe {
        let _ = DestroyMenu(menu);
    }
    let command = selected.0 as usize;
    if command == 0 {
        None
    } else {
        command.checked_sub(1).filter(|index| *index < apps.len())
    }
}

fn collect_item(item: &IShellItem, apps: &mut Vec<StoreApp>, seen: &mut BTreeSet<String>) {
    let Some(parsing_name) = display_name(item, SIGDN_DESKTOPABSOLUTEPARSING) else {
        return;
    };
    let aumid = parsing_name
        .strip_prefix("shell:AppsFolder\\")
        .or_else(|| parsing_name.strip_prefix("shell:appsfolder\\"))
        .unwrap_or(parsing_name.as_str())
        .to_string();
    if !is_packaged_target(&aumid) || !seen.insert(aumid.to_ascii_lowercase()) {
        return;
    }
    let display_name = display_name(item, SIGDN_NORMALDISPLAY)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| aumid.clone());
    apps.push(StoreApp {
        aumid,
        display_name,
    });
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    let text = unsafe { item.GetDisplayName(kind) }.ok()?;
    let result = unsafe { text.to_string() }.ok();
    unsafe {
        CoTaskMemFree(Some(text.0.cast::<c_void>().cast_const()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_store_apps_smoke() {
        let _com = ComGuard::sta();
        let apps = scan_store_apps().expect("store app scan");
        eprintln!("store apps: {}", apps.len());
        for app in apps.iter().take(5) {
            eprintln!("  {} -> {}", app.display_name, app.aumid);
        }
    }
}
