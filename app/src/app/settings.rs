use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use dock_core::layout::Metrics;
use dock_core::models::DockConfig;
use dock_platform_windows::{activate_window, find_window_by_title};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

use super::{DockApp, absolute_config_path, palette_for, resolve_theme};

const SETTINGS_MAGIC: usize = 0x474C_4944;
const SETTINGS_PROTOCOL_VERSION: u32 = 1;
const SETTINGS_HEADER_SIZE: usize = 8;
const MAX_SETTINGS_PAYLOAD: usize = 256 * 1024;

#[derive(Default)]
pub(super) struct SettingsHost {
    child: Option<Child>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SettingsLaunchError {
    #[error("settings executable was not found")]
    NotBuilt,
    #[error("failed to launch settings: {0}")]
    Spawn(#[from] std::io::Error),
}

impl SettingsHost {
    pub(super) fn open(
        &mut self,
        owner: HWND,
        config_path: &Path,
    ) -> Result<(), SettingsLaunchError> {
        let mut running = false;
        let mut stale = false;
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => stale = true,
                Ok(None) => running = true,
                Err(error) => {
                    tracing::warn!(%error, "failed to query settings process");
                    stale = true;
                }
            }
        }
        if stale {
            self.child = None;
        }
        if running {
            if let Some(hwnd) = find_window_by_title("Glide-Dock 设置") {
                activate_window(hwnd);
            }
            return Ok(());
        }

        let Some(executable) = settings_executable() else {
            return Err(SettingsLaunchError::NotBuilt);
        };
        let mut command = std::process::Command::new(&executable);
        command
            .arg("--owner")
            .arg((owner.0 as isize).to_string())
            .arg("--config")
            .arg(absolute_config_path(config_path));
        if let Ok(path) = std::env::current_exe() {
            command.arg("--dock-exe").arg(path);
        }
        if let Some(directory) = executable.parent() {
            command.current_dir(directory);
        }
        command.creation_flags(0x0800_0000);
        let child = command.spawn()?;
        tracing::info!(path = %executable.display(), "settings opened");
        self.child = Some(child);
        Ok(())
    }

    pub(super) fn owns_process(&self, process_id: u32) -> bool {
        process_id != 0 && self.child.as_ref().map(Child::id) == Some(process_id)
    }

    pub(super) fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Some(hwnd) = find_window_by_title("Glide-Dock 设置") {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            let deadline = Instant::now() + Duration::from_millis(350);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    _ => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl DockApp {
    pub(super) fn on_settings_update(&mut self, wparam: WPARAM, lparam: LPARAM) -> bool {
        if lparam.0 == 0 || wparam.0 == 0 || self.read_only {
            return false;
        }

        let sender = HWND(wparam.0 as *mut c_void);
        let mut sender_pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(sender, Some(&mut sender_pid));
        }
        if !self.settings.owns_process(sender_pid) {
            tracing::warn!(
                sender_pid,
                "rejected WM_COPYDATA from an unexpected process"
            );
            return false;
        }

        let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        if data.dwData != SETTINGS_MAGIC
            || data.cbData as usize > SETTINGS_HEADER_SIZE + MAX_SETTINGS_PAYLOAD
            || data.cbData as usize <= SETTINGS_HEADER_SIZE
            || data.lpData.is_null()
        {
            return false;
        }
        let packet =
            unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize) };
        let Some(bytes) = decode_settings_packet(packet) else {
            tracing::warn!("settings sent an unsupported or malformed protocol packet");
            return false;
        };
        let Ok(mut incoming) = serde_json::from_slice::<DockConfig>(bytes) else {
            tracing::warn!("settings sent invalid configuration");
            return false;
        };

        incoming.normalize();
        let previous_metrics = self.metrics;
        let previous_offset = self.config.monitor.offset;
        let previous_glass = self.config.appearance.frosted_glass;
        let next_motion = if incoming.version >= dock_core::CONFIG_VERSION {
            incoming.motion
        } else {
            self.config.motion.clone()
        };
        self.config.appearance = incoming.appearance;
        self.config.behavior = incoming.behavior;
        self.config.motion = next_motion;
        self.config.monitor = incoming.monitor;
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
        let offset_changed = (self.config.monitor.offset - previous_offset).abs() > f32::EPSILON;
        let glass_changed = self.config.appearance.frosted_glass != previous_glass;
        if glass_changed {
            self.refresh_glass_state();
        }
        if next_metrics != previous_metrics || offset_changed {
            self.relayout_window(self.window_visible);
        }
        self.animate();
        self.render();

        // The dock remains the single writer of config.json and retains its own item list.
        self.mark_dirty();
        tracing::debug!(
            protocol_version = SETTINGS_PROTOCOL_VERSION,
            "settings configuration applied"
        );
        true
    }
}

fn decode_settings_packet(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < SETTINGS_HEADER_SIZE {
        return None;
    }

    let version = u32::from_le_bytes(packet[0..4].try_into().ok()?);
    let payload_len = u32::from_le_bytes(packet[4..8].try_into().ok()?) as usize;
    if version != SETTINGS_PROTOCOL_VERSION
        || payload_len == 0
        || payload_len > MAX_SETTINGS_PAYLOAD
        || packet.len() != SETTINGS_HEADER_SIZE + payload_len
    {
        return None;
    }
    Some(&packet[SETTINGS_HEADER_SIZE..])
}

fn settings_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GLIDE_DOCK_SETTINGS_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("GlideDock.Settings.exe"));
            candidates.push(directory.join("settings").join("GlideDock.Settings.exe"));
        }
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    candidates.push(
        repository
            .join("settings-app")
            .join("publish")
            .join("GlideDock.Settings.exe"),
    );
    candidates.push(
        repository
            .join("settings-app")
            .join("bin")
            .join("x64")
            .join("Release")
            .join("net8.0-windows10.0.19041.0")
            .join("win-x64")
            .join("GlideDock.Settings.exe"),
    );

    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::{SETTINGS_HEADER_SIZE, SETTINGS_PROTOCOL_VERSION, decode_settings_packet};

    fn packet(version: u32, payload: &[u8]) -> Vec<u8> {
        let mut packet = Vec::with_capacity(SETTINGS_HEADER_SIZE + payload.len());
        packet.extend_from_slice(&version.to_le_bytes());
        packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    #[test]
    fn decodes_current_protocol_packet() {
        let json = br#"{"version":3}"#;
        assert_eq!(
            decode_settings_packet(&packet(SETTINGS_PROTOCOL_VERSION, json)),
            Some(json.as_slice())
        );
    }

    #[test]
    fn rejects_unknown_protocol_version() {
        assert!(decode_settings_packet(&packet(SETTINGS_PROTOCOL_VERSION + 1, b"{}")).is_none());
    }

    #[test]
    fn rejects_truncated_and_length_mismatched_packets() {
        assert!(decode_settings_packet(&[1, 0, 0, 0]).is_none());
        let mut malformed = packet(SETTINGS_PROTOCOL_VERSION, b"{}");
        malformed[4] = 3;
        assert!(decode_settings_packet(&malformed).is_none());
    }
}
