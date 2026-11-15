//! Hidden watchdog mode for the dock.
//!
//! The dock starts one guard process and passes its PID. The guard waits for
//! that process and restarts it after abnormal exits. A normal exit (code 0,
//! including the tray Exit command) stops the guard.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use dock_platform_windows::{SingleInstance, SingleInstanceOutcome};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

const GUARD_MUTEX: &str = "Local\\Glide-Dock.Guard";
const GUARD_ENV: &str = "GLIDE_DOCK_GUARDED";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_RESTARTS: u32 = 5;
const RESTART_DELAY: Duration = Duration::from_secs(1);

/// Starts the hidden guard as a detached child of the current dock.
pub fn spawn(config: &Path, autostart: bool) -> std::io::Result<()> {
    if std::env::var_os(GUARD_ENV).is_some() {
        return Ok(());
    }

    let executable = std::env::current_exe()?;
    let config = absolute_path(config)?;
    let mut command = Command::new(executable);
    command
        .arg("--guard")
        .arg("--guard-pid")
        .arg(std::process::id().to_string())
        .arg("--config")
        .arg(config);
    if autostart {
        command.arg("--autostart");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let child = command.spawn()?;
    tracing::info!(guard_pid = child.id(), "guard process started");
    Ok(())
}

/// Runs the watchdog loop. Returns after the dock exits normally or after the
/// restart budget is exhausted.
pub fn run(
    dock_pid: Option<u32>,
    config: Option<&Path>,
    autostart: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let _instance = match SingleInstance::acquire_named(GUARD_MUTEX) {
        SingleInstanceOutcome::Acquired(instance) => Some(instance),
        SingleInstanceOutcome::AlreadyRunning => {
            tracing::info!("guard already running");
            return Ok(());
        }
        SingleInstanceOutcome::Failed(error) => {
            tracing::warn!(%error, "guard mutex unavailable; continuing");
            None
        }
    };

    let mut pid = dock_pid.ok_or("guard requires --guard-pid")?;
    let config = config.map(absolute_path).transpose()?;
    let mut restarts = 0u32;
    loop {
        let code = wait_for_process(pid)?;
        if code == 0 {
            tracing::info!(pid, "dock exited normally; guard stopping");
            return Ok(());
        }
        restarts += 1;
        tracing::warn!(pid, code, restarts, "dock exited unexpectedly");
        if restarts > MAX_RESTARTS {
            tracing::error!(
                restarts,
                "guard restart budget exhausted; leaving dock stopped"
            );
            return Ok(());
        }
        std::thread::sleep(RESTART_DELAY);
        let child = spawn_dock(config.as_deref(), autostart)?;
        pid = child.id();
        tracing::info!(pid, "guard restarted dock");
        // Dropping the Child handle does not terminate the process on Windows.
        drop(child);
    }
}

fn spawn_dock(config: Option<&Path>, autostart: bool) -> std::io::Result<std::process::Child> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    if autostart {
        command.arg("--autostart");
    }
    command
        .env(GUARD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    command.spawn()
}

fn wait_for_process(pid: u32) -> Result<u32, Box<dyn std::error::Error>> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(|error| format!("OpenProcess({pid}) failed: {error}"))?;
    let _handle = ProcessHandle(handle);
    let wait = unsafe { WaitForSingleObject(handle, INFINITE) };
    if wait != windows::Win32::Foundation::WAIT_OBJECT_0 {
        return Err(format!("WaitForSingleObject({pid}) returned {wait:?}").into());
    }
    let mut code = 0u32;
    unsafe {
        GetExitCodeProcess(handle, &mut code)
            .map_err(|error| format!("GetExitCodeProcess({pid}) failed: {error}"))?;
    }
    Ok(code)
}

fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
