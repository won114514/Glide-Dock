//! Native notification-area icon.
//!
//! The tray icon is owned by the dock process. The visual right-click menu is
//! rendered by the app as a custom layered popup; this module only registers
//! the icon, forwards clicks, and re-adds it after Explorer restarts.

use std::mem::size_of;

use dock_core::models::DockLayer;
use dock_core::theme::ThemeMode;
use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT};
use windows::Win32::UI::Shell::{
    ExtractIconExW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, HICON, HMENU,
    IDI_APPLICATION, LoadIconW, MENU_ITEM_FLAGS, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    PostMessageW, RegisterWindowMessageW, SetForegroundWindow, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TPM_TOPALIGN, TPM_WORKAREA, TrackPopupMenuEx, WM_APP, WM_LBUTTONDBLCLK,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
};
use windows::core::PCWSTR;

use crate::error::{PlatformError, Result};
use crate::window::wide;

/// Message posted to the dock window by the notification-area callback.
pub const WM_APP_TRAY: u32 = WM_APP + 6;

const TRAY_ID: u32 = 1;
const CMD_SETTINGS: usize = 1;
const CMD_THEME_SYSTEM: usize = 2;
const CMD_THEME_LIGHT: usize = 3;
const CMD_THEME_DARK: usize = 4;
const CMD_LAYER_DESKTOP: usize = 5;
const CMD_LAYER_DYNAMIC: usize = 6;
const CMD_LAYER_TOPMOST: usize = 7;
const CMD_TIPS: usize = 8;
const CMD_EXIT: usize = 9;

/// Actions exposed by the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Open the settings center.
    Settings,
    /// Follow the Windows app theme.
    ThemeSystem,
    /// Force the light theme.
    ThemeLight,
    /// Force the dark theme.
    ThemeDark,
    /// Keep the dock above the desktop.
    LayerDesktop,
    /// Raise the dock while interacting.
    LayerDynamic,
    /// Keep the dock above normal windows.
    LayerTopmost,
    /// Show usage tips.
    Tips,
    /// Exit the dock.
    Exit,
}

/// Events produced by the tray callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Open settings (left click / double click).
    OpenSettings,
    /// Show the context menu (right click).
    ShowMenu,
    /// Explorer restarted; re-add the icon.
    ReAdd,
}

/// Owns one `Shell_NotifyIcon` registration.
pub struct TrayIcon {
    data: NOTIFYICONDATAW,
    icon: HICON,
    owns_icon: bool,
    taskbar_created: u32,
    installed: bool,
}

impl TrayIcon {
    /// Creates the tray owner and makes a best-effort initial registration.
    #[must_use]
    pub fn install(hwnd: HWND) -> Self {
        let (icon, owns_icon) = icon_for_current_exe();
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        copy_utf16(&mut data.szTip, "Glide-Dock");
        let taskbar_created = register_taskbar_created();
        let mut tray = Self {
            data,
            icon,
            owns_icon,
            taskbar_created,
            installed: false,
        };
        if let Err(error) = tray.ensure_installed() {
            tracing::warn!(%error, "initial notification-area registration failed");
        }
        tray
    }

    /// Whether Shell currently acknowledges this icon registration.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.installed
    }

    /// Registers the icon if an earlier attempt failed.
    pub fn ensure_installed(&mut self) -> Result<()> {
        if self.installed {
            return Ok(());
        }
        self.add()
    }

    /// Removes the icon from the notification area.
    pub fn remove(&mut self) {
        if !self.installed {
            return;
        }
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
        self.installed = false;
    }

    /// Re-adds the icon after Explorer restarts.
    pub fn reinstall(&mut self) -> Result<()> {
        self.installed = false;
        self.add()
    }

    /// Message registered by Explorer; zero when unavailable.
    #[must_use]
    pub fn taskbar_created_message(&self) -> u32 {
        self.taskbar_created
    }

    /// Interprets a tray callback or Explorer-restart message.
    #[must_use]
    pub fn handle_message(&self, msg: u32, lparam: LPARAM) -> Option<TrayEvent> {
        tray_event_for_message(self.taskbar_created, msg, lparam)
    }

    /// Shows the native context menu and returns the selected command.
    #[must_use]
    pub fn show_menu(&self, hwnd: HWND, theme: ThemeMode, layer: DockLayer) -> Option<TrayCommand> {
        let menu = unsafe { CreatePopupMenu() }.ok()?;
        let appearance = unsafe { CreatePopupMenu() }.ok()?;
        let layer_menu = unsafe { CreatePopupMenu() }.ok()?;

        append_item(
            appearance,
            checked(theme == ThemeMode::System),
            CMD_THEME_SYSTEM,
            "跟随系统",
        );
        append_item(
            appearance,
            checked(theme == ThemeMode::Light),
            CMD_THEME_LIGHT,
            "浅色",
        );
        append_item(
            appearance,
            checked(theme == ThemeMode::Dark),
            CMD_THEME_DARK,
            "深色",
        );

        append_item(
            layer_menu,
            checked(layer == DockLayer::Desktop),
            CMD_LAYER_DESKTOP,
            "桌面同级",
        );
        append_item(
            layer_menu,
            checked(layer == DockLayer::Dynamic),
            CMD_LAYER_DYNAMIC,
            "动态层级",
        );
        append_item(
            layer_menu,
            checked(layer == DockLayer::Topmost),
            CMD_LAYER_TOPMOST,
            "始终置顶",
        );

        append_item(menu, MF_STRING, CMD_SETTINGS, "⚙  打开设置");
        append_item(menu, MF_SEPARATOR, 0, "");
        append_popup(menu, appearance, "◐  外观");
        append_popup(menu, layer_menu, "▦  Dock 层级");
        append_item(menu, MF_SEPARATOR, 0, "");
        append_item(menu, MF_STRING, CMD_TIPS, "💡  提示和技巧");
        append_item(menu, MF_STRING, CMD_EXIT, "⏻  退出");

        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return None;
        }
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
        let selected = unsafe {
            TrackPopupMenuEx(
                menu,
                (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_WORKAREA).0,
                point.x,
                point.y,
                hwnd,
                None,
            )
        };
        unsafe {
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(
                Some(hwnd),
                WM_NULL,
                windows::Win32::Foundation::WPARAM(0),
                LPARAM(0),
            );
        }
        match selected.0 as usize {
            CMD_SETTINGS => Some(TrayCommand::Settings),
            CMD_THEME_SYSTEM => Some(TrayCommand::ThemeSystem),
            CMD_THEME_LIGHT => Some(TrayCommand::ThemeLight),
            CMD_THEME_DARK => Some(TrayCommand::ThemeDark),
            CMD_LAYER_DESKTOP => Some(TrayCommand::LayerDesktop),
            CMD_LAYER_DYNAMIC => Some(TrayCommand::LayerDynamic),
            CMD_LAYER_TOPMOST => Some(TrayCommand::LayerTopmost),
            CMD_TIPS => Some(TrayCommand::Tips),
            CMD_EXIT => Some(TrayCommand::Exit),
            _ => None,
        }
    }

    /// Shows a balloon notification from the tray icon.
    pub fn notify(&mut self, title: &str, text: &str, warning: bool) {
        if !self.installed {
            return;
        }
        let mut data = self.data;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        copy_utf16(&mut data.szInfoTitle, title);
        copy_utf16(&mut data.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn add(&mut self) -> Result<()> {
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) }.as_bool() {
            let code = unsafe { GetLastError() }.0;
            tracing::warn!(code, "Shell_NotifyIconW(NIM_ADD) failed");
            return Err(PlatformError::Shell(format!(
                "Shell_NotifyIconW(NIM_ADD) failed (Win32 error {code})"
            )));
        }
        self.installed = true;
        tracing::info!("notification-area icon registered");
        Ok(())
    }
}

fn tray_event_for_message(taskbar_created: u32, msg: u32, lparam: LPARAM) -> Option<TrayEvent> {
    if taskbar_created != 0 && msg == taskbar_created {
        return Some(TrayEvent::ReAdd);
    }
    if msg != WM_APP_TRAY {
        return None;
    }
    match lparam.0 as u32 {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => Some(TrayEvent::OpenSettings),
        WM_RBUTTONUP => Some(TrayEvent::ShowMenu),
        _ => None,
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
        if self.owns_icon && !self.icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

fn checked(selected: bool) -> MENU_ITEM_FLAGS {
    if selected {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    }
}

fn append_item(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let label = wide(text);
    unsafe {
        let _ = AppendMenuW(menu, flags, id, PCWSTR::from_raw(label.as_ptr()));
    }
}

fn append_popup(menu: HMENU, submenu: HMENU, text: &str) {
    let label = wide(text);
    unsafe {
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            submenu.0 as usize,
            PCWSTR::from_raw(label.as_ptr()),
        );
    }
}

fn icon_for_current_exe() -> (HICON, bool) {
    let mut icon = HICON::default();
    if let Ok(path) = std::env::current_exe() {
        let path = wide(&path.to_string_lossy());
        let count =
            unsafe { ExtractIconExW(PCWSTR::from_raw(path.as_ptr()), 0, None, Some(&mut icon), 1) };
        if count > 0 && !icon.is_invalid() {
            return (icon, true);
        }
    }
    let fallback = unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default();
    (fallback, false)
}

fn register_taskbar_created() -> u32 {
    let name = wide("TaskbarCreated");
    unsafe { RegisterWindowMessageW(PCWSTR::from_raw(name.as_ptr())) }
}

fn copy_utf16(buffer: &mut [u16], text: &str) {
    if buffer.is_empty() {
        return;
    }
    let encoded: Vec<u16> = text.encode_utf16().take(buffer.len() - 1).collect();
    buffer.fill(0);
    buffer[..encoded.len()].copy_from_slice(&encoded);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_taskbar_created_even_when_no_icon_was_installed() {
        assert_eq!(
            tray_event_for_message(0xB123, 0xB123, LPARAM(0)),
            Some(TrayEvent::ReAdd)
        );
        assert_eq!(tray_event_for_message(0, 0xB123, LPARAM(0)), None);
    }

    #[test]
    fn maps_tray_clicks_and_ignores_other_messages() {
        assert_eq!(
            tray_event_for_message(0xB123, WM_APP_TRAY, LPARAM(WM_LBUTTONUP as isize)),
            Some(TrayEvent::OpenSettings)
        );
        assert_eq!(
            tray_event_for_message(0xB123, WM_APP_TRAY, LPARAM(WM_RBUTTONUP as isize)),
            Some(TrayEvent::ShowMenu)
        );
        assert_eq!(tray_event_for_message(0xB123, WM_NULL, LPARAM(0)), None);
    }
}
