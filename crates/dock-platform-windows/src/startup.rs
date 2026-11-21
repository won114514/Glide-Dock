//! Shared logon startup registration used by Glide-Dock and its setup tool.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

use crate::registry::Key;

const TASK_NAME: &str = "Glide-Dock";
const RUN_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Glide-Dock";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The preferred mechanism for launching Glide-Dock at sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupMode {
    /// A no-delay, current-user Task Scheduler logon task.
    Task,
    /// The current-user Run registry value, which requires no elevation.
    Run,
}

impl StartupMode {
    /// Parses the persisted `task` or `run` identifier.
    pub fn parse(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("run") {
            Self::Run
        } else {
            Self::Task
        }
    }
}

/// The configured startup mechanism and current registration state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupState {
    /// Whether either recognized registration is enabled.
    pub enabled: bool,
    /// Which registration will be preferred the next time startup is enabled.
    pub preferred: StartupMode,
    /// Resolved executable, when either registration points to one.
    pub executable: Option<PathBuf>,
}

/// Queries current-user registrations without changing them.
pub fn query(config_directory: &Path) -> io::Result<StartupState> {
    let preferred = read_preference(config_directory);
    let task = task_query()?;
    let run = Key::open(
        HKEY_CURRENT_USER,
        RUN_PATH,
        windows::Win32::System::Registry::KEY_QUERY_VALUE,
    )?
    .map(|key| key.text(RUN_VALUE))
    .transpose()?
    .flatten();
    let executable = task
        .as_deref()
        .and_then(extract_executable)
        .or_else(|| run.as_deref().and_then(extract_executable));
    Ok(StartupState {
        enabled: task.is_some() || run.is_some(),
        preferred,
        executable,
    })
}

/// Enables startup in the selected mode. A denied task registration prompts
/// through UAC, then stores the requested mode; canceled consent falls back to Run.
pub fn enable(
    mode: StartupMode,
    executable: &Path,
    config_directory: &Path,
) -> io::Result<Option<String>> {
    let executable = executable.canonicalize()?;
    match mode {
        StartupMode::Task => match create_task(&executable, false) {
            Ok(()) => {
                let warning = disable_run()
                    .err()
                    .map(|error| format!("登录任务已启用，但旧版启动项未能移除：{error}"));
                write_preference(config_directory, StartupMode::Task)?;
                Ok(warning)
            }
            Err(error) => match create_task(&executable, true) {
                Ok(()) => {
                    let warning = disable_run()
                        .err()
                        .map(|error| format!("登录任务已启用，但旧版启动项未能移除：{error}"));
                    write_preference(config_directory, StartupMode::Task)?;
                    Ok(warning)
                }
                Err(elevated) if elevated.raw_os_error() == Some(1223) => {
                    enable_run(&executable)?;
                    write_preference(config_directory, StartupMode::Run)?;
                    Ok(Some(
                        "未授予计划任务权限，已改用无需管理员权限的兼容启动方式。".into(),
                    ))
                }
                Err(elevated) => {
                    let _ = error;
                    enable_run(&executable)?;
                    write_preference(config_directory, StartupMode::Run)?;
                    Ok(Some(format!(
                        "计划任务无法创建，已改用兼容启动方式：{elevated}"
                    )))
                }
            },
        },
        StartupMode::Run => {
            enable_run(&executable)?;
            let warning = delete_task(false)
                .or_else(|_| delete_task(true))
                .err()
                .map(|error| format!("兼容启动已启用，但计划任务未能移除：{error}"));
            write_preference(config_directory, StartupMode::Run)?;
            Ok(warning)
        }
    }
}

/// Removes this user's recognized Task Scheduler and Run registrations.
pub fn disable() -> io::Result<()> {
    match delete_task(false) {
        Ok(()) => (),
        Err(_) => delete_task(true)?,
    }
    disable_run()
}

/// Stores a mode preference without changing the current registration.
pub fn set_preferred_mode(mode: StartupMode, config_directory: &Path) -> io::Result<()> {
    write_preference(config_directory, mode)
}

/// Removes a startup registration only when it points into `install_directory`.
pub fn remove_for_install(install_directory: &Path, config_directory: &Path) -> io::Result<()> {
    let state = query(config_directory)?;
    if state
        .executable
        .is_some_and(|path| path.starts_with(install_directory))
    {
        disable()?;
    }
    Ok(())
}

fn task_command(executable: &Path) -> String {
    format!(
        "{} --autostart",
        windows_quote(&executable.to_string_lossy())
    )
}

fn create_task(executable: &Path, elevated: bool) -> io::Result<()> {
    let task = task_command(executable);
    let args = [
        "/Create", "/SC", "ONLOGON", "/TN", TASK_NAME, "/TR", &task, "/RL", "LIMITED", "/F",
    ];
    if elevated {
        let system = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let system = system.join("System32").join("schtasks.exe");
        let encoded = args
            .iter()
            .map(|arg| windows_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let code = crate::process::run_elevated(&system, &encoded).map_err(|error| {
            if crate::process::elevation_canceled(&error) {
                io::Error::from_raw_os_error(1223)
            } else {
                io::Error::other(error.to_string())
            }
        })?;
        if code == 0 {
            return Ok(());
        }
        return Err(io::Error::other(format!(
            "Task Scheduler returned exit code {code}"
        )));
    }
    run_schtasks(&args).and_then(|output| ensure_success(output, "could not create the logon task"))
}

fn delete_task(elevated: bool) -> io::Result<()> {
    if task_query()?.is_none() {
        return Ok(());
    }
    let args = ["/Delete", "/TN", TASK_NAME, "/F"];
    if elevated {
        let system = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let system = system.join("System32").join("schtasks.exe");
        let encoded = args
            .iter()
            .map(|arg| windows_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let code = crate::process::run_elevated(&system, &encoded).map_err(|error| {
            if crate::process::elevation_canceled(&error) {
                io::Error::from_raw_os_error(1223)
            } else {
                io::Error::other(error.to_string())
            }
        })?;
        if code == 0 {
            return Ok(());
        }
        return Err(io::Error::other(format!(
            "Task Scheduler returned exit code {code}"
        )));
    }
    run_schtasks(&args).and_then(|output| ensure_success(output, "could not remove the logon task"))
}

fn task_query() -> io::Result<Option<String>> {
    let output = run_schtasks(&["/Query", "/TN", TASK_NAME, "/XML"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let xml = String::from_utf8_lossy(&output.stdout).into_owned();
    if xml.contains("<Task") {
        Ok(tag_text(&xml, "Command").map(|command| {
            let arguments = tag_text(&xml, "Arguments").unwrap_or_default();
            if arguments.is_empty() {
                windows_quote(&command)
            } else {
                format!("{} {arguments}", windows_quote(&command))
            }
        }))
    } else {
        Ok(None)
    }
}

fn run_schtasks(args: &[&str]) -> io::Result<Output> {
    let system = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    Command::new(system.join("System32").join("schtasks.exe"))
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

fn ensure_success(output: Output, action: &str) -> io::Result<()> {
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn enable_run(executable: &Path) -> io::Result<()> {
    let key = Key::create(HKEY_CURRENT_USER, RUN_PATH)?;
    key.set_text(
        RUN_VALUE,
        &format!(
            "{} --autostart",
            windows_quote(&executable.to_string_lossy())
        ),
    )
}

fn disable_run() -> io::Result<()> {
    if let Some(key) = Key::open(
        HKEY_CURRENT_USER,
        RUN_PATH,
        windows::Win32::System::Registry::KEY_SET_VALUE,
    )? {
        key.remove(RUN_VALUE)?;
    }
    Ok(())
}

fn read_preference(directory: &Path) -> StartupMode {
    fs::read_to_string(directory.join("startup-mode.txt"))
        .map(|value| StartupMode::parse(&value))
        .unwrap_or(StartupMode::Task)
}

fn write_preference(directory: &Path, mode: StartupMode) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let value = if mode == StartupMode::Task {
        "task\n"
    } else {
        "run\n"
    };
    let target = directory.join("startup-mode.txt");
    let temporary = directory.join("startup-mode.txt.tmp");
    fs::write(&temporary, value)?;
    match fs::rename(&temporary, &target) {
        Ok(()) => Ok(()),
        Err(error) if target.exists() => {
            fs::remove_file(&target)?;
            fs::rename(&temporary, &target).map_err(|_| error)
        }
        Err(error) => {
            let _ = fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(
        xml[start..end]
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">"),
    )
}

fn extract_executable(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let path = if let Some(rest) = command.strip_prefix('"') {
        rest.split_once('"')?.0
    } else {
        command.split_whitespace().next()?
    };
    Some(PathBuf::from(path))
}

fn windows_quote(argument: &str) -> String {
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut slashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                quoted.push(character);
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', slashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_task_paths_and_trailing_slashes_for_windows() {
        assert_eq!(
            windows_quote(r"C:\Program Files\Glide Dock\glide-dock.exe"),
            r#""C:\Program Files\Glide Dock\glide-dock.exe""#
        );
        assert_eq!(
            windows_quote(r#"C:\path\with\"quote.exe"#),
            r#""C:\path\with\\\"quote.exe""#
        );
    }

    #[test]
    fn parses_schtasks_xml_command() {
        let xml = "<Task><Command>C:\\Apps &amp; Tools\\glide-dock.exe</Command><Arguments>--autostart</Arguments></Task>";
        let command = tag_text(xml, "Command").expect("command");
        assert_eq!(
            extract_executable(&format!("{} --autostart", windows_quote(&command))),
            Some(PathBuf::from(r"C:\Apps & Tools\glide-dock.exe"))
        );
    }
}
