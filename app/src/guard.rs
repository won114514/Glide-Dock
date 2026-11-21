//! Hidden watchdog mode for the dock.
//!
//! The dock starts one guard process and passes its PID. The guard waits for
//! that process and restarts it after abnormal exits. A normal exit (code 0,
//! including the tray Exit command) stops the guard.

use std::collections::VecDeque;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dock_platform_windows::{SingleInstance, SingleInstanceOutcome};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

const GUARD_MUTEX: &str = "Local\\Glide-Dock.Guard";
const GUARD_ENV: &str = "GLIDE_DOCK_GUARDED";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_RESTARTS_PER_WINDOW: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const STABLE_RUN_RESET: Duration = Duration::from_secs(10 * 60);
const RESTART_DELAYS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(15),
];

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
    let mut policy = RestartPolicy::new(Instant::now());
    loop {
        let code = wait_for_process(pid)?;
        let decision = policy.decide(code, Instant::now());
        let delay = match decision {
            RestartDecision::StopNormally => {
                tracing::info!(pid, "dock exited normally; guard stopping");
                return Ok(());
            }
            RestartDecision::BudgetExhausted => {
                tracing::error!(
                    pid,
                    restarts = policy.restart_count_in_window(Instant::now()),
                    "guard restart budget exhausted; leaving dock stopped"
                );
                return Ok(());
            }
            RestartDecision::RestartAfter(delay) => delay,
        };
        tracing::warn!(
            pid,
            code,
            restart = policy.consecutive_restarts,
            delay_seconds = delay.as_secs(),
            "dock exited unexpectedly"
        );
        std::thread::sleep(delay);
        let child = spawn_dock(config.as_deref(), autostart)?;
        pid = child.id();
        policy.record_restart(Instant::now());
        tracing::info!(pid, "guard restarted dock");
        // Dropping the Child handle does not terminate the process on Windows.
        drop(child);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartDecision {
    StopNormally,
    RestartAfter(Duration),
    BudgetExhausted,
}

struct RestartPolicy {
    restart_times: VecDeque<Instant>,
    consecutive_restarts: usize,
    process_started_at: Instant,
}

impl RestartPolicy {
    fn new(process_started_at: Instant) -> Self {
        Self {
            restart_times: VecDeque::new(),
            consecutive_restarts: 0,
            process_started_at,
        }
    }

    fn decide(&mut self, exit_code: u32, now: Instant) -> RestartDecision {
        if exit_code == 0 {
            return RestartDecision::StopNormally;
        }
        if now.saturating_duration_since(self.process_started_at) >= STABLE_RUN_RESET {
            self.restart_times.clear();
            self.consecutive_restarts = 0;
        }
        self.prune_old_restarts(now);
        if self.restart_times.len() >= MAX_RESTARTS_PER_WINDOW {
            return RestartDecision::BudgetExhausted;
        }
        let delay_index = self.consecutive_restarts.min(RESTART_DELAYS.len() - 1);
        self.consecutive_restarts += 1;
        RestartDecision::RestartAfter(RESTART_DELAYS[delay_index])
    }

    fn record_restart(&mut self, started_at: Instant) {
        self.restart_times.push_back(started_at);
        self.process_started_at = started_at;
    }

    fn restart_count_in_window(&mut self, now: Instant) -> usize {
        self.prune_old_restarts(now);
        self.restart_times.len()
    }

    fn prune_old_restarts(&mut self, now: Instant) {
        while self
            .restart_times
            .front()
            .is_some_and(|started| now.saturating_duration_since(*started) >= RESTART_WINDOW)
        {
            self.restart_times.pop_front();
        }
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

#[cfg(test)]
mod tests {
    use super::{
        MAX_RESTARTS_PER_WINDOW, RESTART_DELAYS, RESTART_WINDOW, RestartDecision, RestartPolicy,
        STABLE_RUN_RESET,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn normal_exit_never_restarts() {
        let started = Instant::now();
        let mut policy = RestartPolicy::new(started);
        assert_eq!(
            policy.decide(0, started + Duration::from_secs(1)),
            RestartDecision::StopNormally
        );
    }

    #[test]
    fn unexpected_exits_use_capped_exponential_backoff() {
        let mut now = Instant::now();
        let mut policy = RestartPolicy::new(now);
        for delay in RESTART_DELAYS {
            assert_eq!(
                policy.decide(1, now + Duration::from_millis(500)),
                RestartDecision::RestartAfter(delay)
            );
            now += Duration::from_secs(2);
            policy.record_restart(now);
        }
        assert_eq!(policy.consecutive_restarts, RESTART_DELAYS.len());
    }

    #[test]
    fn restart_limit_is_five_inside_a_rolling_minute() {
        let mut now = Instant::now();
        let mut policy = RestartPolicy::new(now);
        for _ in 0..MAX_RESTARTS_PER_WINDOW {
            assert!(matches!(
                policy.decide(1, now + Duration::from_millis(100)),
                RestartDecision::RestartAfter(_)
            ));
            now += Duration::from_secs(5);
            policy.record_restart(now);
        }
        assert_eq!(
            policy.decide(1, now + Duration::from_millis(100)),
            RestartDecision::BudgetExhausted
        );
    }

    #[test]
    fn restart_budget_expires_after_sixty_seconds() {
        let started = Instant::now();
        let mut policy = RestartPolicy::new(started);
        for index in 0..MAX_RESTARTS_PER_WINDOW {
            let at = started + Duration::from_secs(index as u64);
            assert!(matches!(
                policy.decide(1, at),
                RestartDecision::RestartAfter(_)
            ));
            policy.record_restart(at);
        }
        let now = started + RESTART_WINDOW + Duration::from_secs(1);
        assert!(matches!(
            policy.decide(1, now),
            RestartDecision::RestartAfter(_)
        ));
    }

    #[test]
    fn ten_stable_minutes_reset_the_budget_and_backoff() {
        let started = Instant::now();
        let mut policy = RestartPolicy::new(started);
        assert!(matches!(
            policy.decide(1, started),
            RestartDecision::RestartAfter(_)
        ));
        policy.record_restart(started);
        let stable_exit = started + STABLE_RUN_RESET;
        assert_eq!(
            policy.decide(1, stable_exit),
            RestartDecision::RestartAfter(RESTART_DELAYS[0])
        );
        assert_eq!(policy.restart_count_in_window(stable_exit), 0);
    }
}
