//! Shared current-user logon startup registration.

use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{LookupAccountNameW, PSID, SID_NAME_USE};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::TaskScheduler::{
    IExecAction, ILogonTrigger, ITaskDefinition, ITaskFolder, ITaskService, TASK_ACTION_EXEC,
    TASK_CREATE_OR_UPDATE, TASK_LOGON_GROUP, TASK_RUNLEVEL_LUA, TASK_TRIGGER_LOGON, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, HRESULT, Interface, PCWSTR, PWSTR};

use crate::com::ComGuard;
use crate::registry::Key;

const TASK_NAME: &str = "Glide-Dock";
const USERS_GROUP_SID: &str = "S-1-5-32-545";
const RUN_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Glide-Dock";
/// Internal elevated worker entry point used by the Dock process.
pub const STARTUP_WORKER_FLAG: &str = "--glide-startup-worker";
const MAX_WORKER_PAYLOAD_BYTES: usize = 16 * 1024;

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

/// A fixed operation on the Glide-Dock logon task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum LogonTaskAction {
    /// Creates or updates the task with the usual Glide-Dock startup arguments.
    Create {
        /// Absolute path to the installed Glide-Dock executable.
        executable: PathBuf,
    },
    /// Removes only the Glide-Dock logon task.
    Delete {},
}

/// Applies an operation using the current process's permissions, without UAC.
pub fn apply_logon_task(action: LogonTaskAction) -> io::Result<()> {
    match action {
        LogonTaskAction::Create { executable } => create_task(&canonical_win32_path(&executable)?),
        LogonTaskAction::Delete {} => delete_task(),
    }
}

fn elevate_logon_task(action: LogonTaskAction) -> io::Result<()> {
    elevate_logon_task_with(action, &std::env::current_exe()?, &|source, arguments| {
        crate::process::run_elevated(source, arguments).map_err(|error| {
            if crate::process::elevation_canceled(&error) {
                io::Error::from_raw_os_error(1223)
            } else {
                io::Error::other(error.to_string())
            }
        })
    })
}

fn elevate_logon_task_with(
    action: LogonTaskAction,
    source: &Path,
    run: &impl Fn(&Path, &str) -> io::Result<u32>,
) -> io::Result<()> {
    let payload = encode_worker_action(&action)?;
    let code = run(source, &format!("{STARTUP_WORKER_FLAG} {payload}"))?;
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "登录启动项设置未能完成（退出码 {code}）。"
        )))
    }
}

/// Runs the elevated startup worker used by the Dock executable.
///
/// The worker accepts only the fixed create/delete actions encoded by
/// `elevate_logon_task`; it never evaluates arbitrary commands.
pub fn run_startup_worker(args: &[String]) -> i32 {
    let Ok(action) = parse_worker_request(args) else {
        return 2;
    };
    if let LogonTaskAction::Create { executable } = &action
        && !std::env::current_exe().is_ok_and(|source| same_path(&source, executable))
    {
        return 2;
    }
    if !crate::system::is_elevated() {
        return 5;
    }
    match apply_logon_task(action) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn encode_worker_action(action: &LogonTaskAction) -> io::Result<String> {
    let bytes = serde_json::to_vec(action).map_err(io::Error::other)?;
    if bytes.len() > MAX_WORKER_PAYLOAD_BYTES {
        return Err(io::Error::other("登录启动项工作进程参数过长。"));
    }
    let mut payload = String::with_capacity(bytes.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        payload.push(HEX[(byte >> 4) as usize] as char);
        payload.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(payload)
}

fn parse_worker_request(args: &[String]) -> Result<LogonTaskAction, ()> {
    if args.len() != 2 || args[0] != STARTUP_WORKER_FLAG {
        return Err(());
    }
    let payload = &args[1];
    if payload.len() > MAX_WORKER_PAYLOAD_BYTES * 2 || !payload.len().is_multiple_of(2) {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(payload.len() / 2);
    for pair in payload.as_bytes().as_chunks::<2>().0 {
        let text = std::str::from_utf8(pair).map_err(|_| ())?;
        bytes.push(u8::from_str_radix(text, 16).map_err(|_| ())?);
    }
    let action = serde_json::from_slice::<LogonTaskAction>(&bytes).map_err(|_| ())?;
    if let LogonTaskAction::Create { executable } = &action
        && (!executable.is_absolute()
            || executable.file_name().is_none_or(|name| {
                !name
                    .to_string_lossy()
                    .eq_ignore_ascii_case("glide-dock.exe")
            }))
    {
        return Err(());
    }
    Ok(action)
}

fn run_logon_task(
    action: LogonTaskAction,
    apply: &impl Fn(LogonTaskAction) -> io::Result<()>,
    elevate: &impl Fn(LogonTaskAction) -> io::Result<()>,
) -> io::Result<()> {
    match apply(action.clone()) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) => elevate(action),
        Err(error) => Err(error),
    }
}

/// Repairs an existing task without creating one that the user disabled.
/// Returns `true` only when a mismatched existing task was updated.
pub fn repair_existing_task(executable: &Path) -> io::Result<bool> {
    let executable = canonical_win32_path(executable)?;
    let Some(task) = task_query()? else {
        return Ok(false);
    };
    if task.matches_target(&executable) {
        return Ok(false);
    }
    create_task(&executable)?;
    Ok(true)
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
        .as_ref()
        .and_then(LogonTaskDefinition::executable)
        .or_else(|| run.as_deref().and_then(extract_executable));
    Ok(StartupState {
        enabled: task.is_some() || run.is_some(),
        preferred,
        executable,
    })
}

/// Enables startup in the selected mode. A denied task update prompts through
/// UAC, then stores the requested mode; canceled consent falls back to Run.
pub fn enable(
    mode: StartupMode,
    executable: &Path,
    config_directory: &Path,
) -> io::Result<Option<String>> {
    enable_with_elevation(mode, executable, config_directory, &elevate_logon_task)
}

/// Enables startup, using the supplied application-owned entry point when UAC is needed.
/// Canceled consent still falls back to the current-user Run registration.
pub fn enable_with_elevation(
    mode: StartupMode,
    executable: &Path,
    config_directory: &Path,
    elevate: &impl Fn(LogonTaskAction) -> io::Result<()>,
) -> io::Result<Option<String>> {
    let executable = canonical_win32_path(executable)?;
    match mode {
        StartupMode::Task => match run_logon_task(
            LogonTaskAction::Create {
                executable: executable.clone(),
            },
            &apply_logon_task,
            elevate,
        ) {
            Ok(()) => {
                let warning = disable_run()
                    .err()
                    .map(|error| format!("登录任务已启用，但旧版启动项未能移除：{error}"));
                write_preference(config_directory, StartupMode::Task)?;
                Ok(warning)
            }
            Err(error) if error.raw_os_error() == Some(1223) => {
                enable_run(&executable)?;
                write_preference(config_directory, StartupMode::Run)?;
                Ok(Some(
                    "未授予计划任务权限，已改用无需管理员权限的兼容启动方式。".into(),
                ))
            }
            Err(error) => {
                enable_run(&executable)?;
                write_preference(config_directory, StartupMode::Run)?;
                Ok(Some(format!(
                    "计划任务无法创建，已改用兼容启动方式：{error}"
                )))
            }
        },
        StartupMode::Run => {
            enable_run(&executable)?;
            let warning = run_logon_task(LogonTaskAction::Delete {}, &apply_logon_task, elevate)
                .err()
                .map(|error| format!("兼容启动已启用，但计划任务未能移除：{error}"));
            write_preference(config_directory, StartupMode::Run)?;
            Ok(warning)
        }
    }
}

/// Removes this user's recognized Task Scheduler and Run registrations.
pub fn disable() -> io::Result<()> {
    disable_with_elevation(&elevate_logon_task)
}

fn disable_with_elevation(elevate: &impl Fn(LogonTaskAction) -> io::Result<()>) -> io::Result<()> {
    run_logon_task(LogonTaskAction::Delete {}, &apply_logon_task, elevate)?;
    disable_run()
}

/// Stores a mode preference without changing the current registration.
pub fn set_preferred_mode(mode: StartupMode, config_directory: &Path) -> io::Result<()> {
    write_preference(config_directory, mode)
}

/// Removes a startup registration only when it points into `install_directory`.
pub fn remove_for_install(install_directory: &Path, config_directory: &Path) -> io::Result<()> {
    remove_for_install_with_elevation(install_directory, config_directory, &elevate_logon_task)
}

/// Removes registrations for this installation, using the supplied UAC entry point.
pub fn remove_for_install_with_elevation(
    install_directory: &Path,
    config_directory: &Path,
    elevate: &impl Fn(LogonTaskAction) -> io::Result<()>,
) -> io::Result<()> {
    let state = query(config_directory)?;
    if state
        .executable
        .is_some_and(|path| path.starts_with(install_directory))
    {
        disable_with_elevation(elevate)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct LogonTaskDefinition {
    command: String,
    arguments: String,
    user_sid: Option<String>,
    group_sid: Option<String>,
    logon_type: Option<String>,
    run_level: Option<String>,
    trigger_user_sid: Option<String>,
    delay: Option<String>,
}

impl LogonTaskDefinition {
    fn from_xml(xml: &str) -> Option<Self> {
        if !xml.contains("<Task") {
            return None;
        }
        Some(Self {
            command: section_text(xml, "Exec", "Command")?,
            arguments: section_text(xml, "Exec", "Arguments").unwrap_or_default(),
            user_sid: section_text(xml, "Principal", "UserId"),
            group_sid: section_text(xml, "Principal", "GroupId"),
            logon_type: section_text(xml, "Principal", "LogonType"),
            run_level: section_text(xml, "Principal", "RunLevel"),
            trigger_user_sid: section_text(xml, "LogonTrigger", "UserId"),
            delay: section_text(xml, "LogonTrigger", "Delay"),
        })
    }

    fn executable(&self) -> Option<PathBuf> {
        let command = self.command.trim();
        if command.is_empty() {
            return None;
        }
        Some(PathBuf::from(command.trim_matches('"')))
    }

    fn matches_target(&self, executable: &Path) -> bool {
        self.user_sid.is_none()
            && user_matches_sid(self.group_sid.as_deref(), USERS_GROUP_SID)
            && self.trigger_user_sid.is_none()
            && matches!(self.logon_type.as_deref(), None | Some("Group"))
            && matches!(self.run_level.as_deref(), None | Some("LeastPrivilege"))
            && matches!(self.delay.as_deref(), None | Some("PT0S"))
            && self.arguments.trim() == "--autostart"
            && self
                .executable()
                .is_some_and(|path| same_path(&path, executable))
    }
}

fn create_task(executable: &Path) -> io::Result<()> {
    let executable = canonical_win32_path(executable)?;
    if task_query()?.is_some_and(|task| task.matches_target(&executable)) {
        return Ok(());
    }
    with_task_folder(|service, folder| {
        let definition = build_task_definition(service, &executable)?;
        let empty = VARIANT::default();
        let group = VARIANT::from(BSTR::from(USERS_GROUP_SID));
        unsafe {
            folder.RegisterTaskDefinition(
                &BSTR::from(TASK_NAME),
                &definition,
                TASK_CREATE_OR_UPDATE.0,
                &group,
                &empty,
                TASK_LOGON_GROUP,
                &empty,
            )
        }
        .map_err(com_error)?;
        Ok(())
    })?;
    let task = task_query()?.ok_or_else(|| io::Error::other("created logon task is missing"))?;
    if task.matches_target(&executable) {
        Ok(())
    } else {
        Err(io::Error::other(
            "created logon task has an unexpected definition",
        ))
    }
}

fn delete_task() -> io::Result<()> {
    if task_query()?.is_none() {
        return Ok(());
    }
    with_task_folder(
        |_, folder| match unsafe { folder.DeleteTask(&BSTR::from(TASK_NAME), 0) } {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => Ok(()),
            Err(error) => Err(com_error(error)),
        },
    )
}

fn task_query() -> io::Result<Option<LogonTaskDefinition>> {
    with_task_folder(|_, folder| {
        let task = match unsafe { folder.GetTask(&BSTR::from(TASK_NAME)) } {
            Ok(task) => task,
            Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
                return Ok(None);
            }
            Err(error) => return Err(com_error(error)),
        };
        let xml = unsafe { task.Xml() }.map_err(com_error)?;
        let xml = String::from_utf16_lossy(&xml);
        let definition = LogonTaskDefinition::from_xml(&xml)
            .ok_or_else(|| io::Error::other("existing logon task has an invalid definition"))?;
        Ok(Some(definition))
    })
}

fn with_task_folder<T>(
    operation: impl FnOnce(&ITaskService, &ITaskFolder) -> io::Result<T>,
) -> io::Result<T> {
    let _com = ComGuard::sta();
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_ALL) }.map_err(com_error)?;
    let empty = VARIANT::default();
    unsafe { service.Connect(&empty, &empty, &empty, &empty) }.map_err(com_error)?;
    let root = BSTR::from("\\");
    let folder = unsafe { service.GetFolder(&root) }.map_err(com_error)?;
    operation(&service, &folder)
}

fn build_task_definition(service: &ITaskService, executable: &Path) -> io::Result<ITaskDefinition> {
    let definition = unsafe { service.NewTask(0) }.map_err(com_error)?;
    let principal = unsafe { definition.Principal() }.map_err(com_error)?;
    unsafe { principal.SetId(&BSTR::from("GlideDockPrincipal")) }.map_err(com_error)?;
    unsafe { principal.SetGroupId(&BSTR::from(USERS_GROUP_SID)) }.map_err(com_error)?;
    unsafe { principal.SetLogonType(TASK_LOGON_GROUP) }.map_err(com_error)?;
    unsafe { principal.SetRunLevel(TASK_RUNLEVEL_LUA) }.map_err(com_error)?;

    let triggers = unsafe { definition.Triggers() }.map_err(com_error)?;
    let trigger = unsafe { triggers.Create(TASK_TRIGGER_LOGON) }.map_err(com_error)?;
    let logon = trigger.cast::<ILogonTrigger>().map_err(com_error)?;
    unsafe { logon.SetId(&BSTR::from("GlideDockLogon")) }.map_err(com_error)?;
    unsafe { logon.SetDelay(&BSTR::from("PT0S")) }.map_err(com_error)?;
    unsafe { logon.SetEnabled(true.into()) }.map_err(com_error)?;

    let actions = unsafe { definition.Actions() }.map_err(com_error)?;
    let action = unsafe { actions.Create(TASK_ACTION_EXEC) }.map_err(com_error)?;
    let action = action.cast::<IExecAction>().map_err(com_error)?;
    let executable = executable.to_string_lossy();
    unsafe { action.SetPath(&BSTR::from(executable.as_ref())) }.map_err(com_error)?;
    unsafe { action.SetArguments(&BSTR::from("--autostart")) }.map_err(com_error)?;

    let settings = unsafe { definition.Settings() }.map_err(com_error)?;
    unsafe { settings.SetPriority(4) }.map_err(com_error)?;
    unsafe { settings.SetStartWhenAvailable(true.into()) }.map_err(com_error)?;
    Ok(definition)
}

fn user_matches_sid(value: Option<&str>, expected_sid: &str) -> bool {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if value.eq_ignore_ascii_case(expected_sid) {
        return true;
    }
    account_name_to_sid(value).is_some_and(|sid| sid.eq_ignore_ascii_case(expected_sid))
}

fn account_name_to_sid(account: &str) -> Option<String> {
    let wide_account = crate::window::wide(account);
    let mut sid_len = 0u32;
    let mut domain_len = 0u32;
    let mut sid_use = SID_NAME_USE::default();
    let _ = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR::from_raw(wide_account.as_ptr()),
            None,
            &mut sid_len,
            None,
            &mut domain_len,
            &mut sid_use,
        )
    };
    if sid_len == 0
        || unsafe { windows::Win32::Foundation::GetLastError() } != ERROR_INSUFFICIENT_BUFFER
    {
        return None;
    }
    let mut sid_buffer = vec![0u64; sid_len.div_ceil(size_of::<u64>() as u32) as usize];
    let mut domain_buffer = vec![0u16; domain_len.max(1) as usize];
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR::from_raw(wide_account.as_ptr()),
            Some(PSID(sid_buffer.as_mut_ptr().cast())),
            &mut sid_len,
            Some(PWSTR::from_raw(domain_buffer.as_mut_ptr())),
            &mut domain_len,
            &mut sid_use,
        )
    }
    .ok()?;
    let mut sid_pwstr = PWSTR::null();
    unsafe { ConvertSidToStringSidW(PSID(sid_buffer.as_mut_ptr().cast()), &mut sid_pwstr) }.ok()?;
    let text = unsafe { sid_pwstr.to_string() }.ok();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sid_pwstr.0.cast())));
    }
    text
}

fn canonical_win32_path(path: &Path) -> io::Result<PathBuf> {
    Ok(strip_verbatim_prefix(&path.canonicalize()?))
}

fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        PathBuf::from(local)
    } else {
        path.to_path_buf()
    }
}

fn com_error(error: windows::core::Error) -> io::Error {
    let code = error.code();
    if code == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) {
        io::Error::from_raw_os_error(ERROR_ACCESS_DENIED.0 as i32)
    } else if code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) {
        io::Error::from_raw_os_error(ERROR_FILE_NOT_FOUND.0 as i32)
    } else {
        io::Error::other(error)
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

fn section_text(xml: &str, section: &str, tag: &str) -> Option<String> {
    let marker = format!("<{section}");
    let start = xml.match_indices(&marker).find_map(|(index, _)| {
        let after = index + marker.len();
        match xml[after..].chars().next() {
            Some('>') => Some(after),
            Some(character) if character.is_whitespace() => Some(after),
            _ => None,
        }
    })?;
    let open_end = start + xml[start..].find('>')? + 1;
    let close = format!("</{section}>");
    let end = open_end + xml[open_end..].find(&close)?;
    tag_text(&xml[open_end..end], tag)
}

fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(
        xml[start..end]
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'"),
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

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()),
    }
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

    fn definition() -> LogonTaskDefinition {
        LogonTaskDefinition {
            command: r"C:\Apps\Glide-Dock\glide-dock.exe".into(),
            arguments: "--autostart".into(),
            user_sid: None,
            group_sid: Some(USERS_GROUP_SID.into()),
            logon_type: Some("Group".into()),
            run_level: Some("LeastPrivilege".into()),
            trigger_user_sid: None,
            delay: Some("PT0S".into()),
        }
    }

    #[test]
    fn successful_task_operation_does_not_request_elevation() {
        run_logon_task(LogonTaskAction::Delete {}, &|_| Ok(()), &|_| {
            panic!("successful operation must not ask for UAC")
        })
        .unwrap();
    }

    #[test]
    fn access_denied_uses_supplied_product_entry_point_once() {
        let expected = LogonTaskAction::Create {
            executable: PathBuf::from("installed-glide-dock.exe"),
        };
        let invoked = std::cell::Cell::new(0);
        run_logon_task(
            expected.clone(),
            &|_| Err(io::Error::from_raw_os_error(5)),
            &|action| {
                assert_eq!(action, expected);
                invoked.set(invoked.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(invoked.get(), 1);
    }

    #[test]
    fn unrelated_task_failure_does_not_request_elevation() {
        let error = run_logon_task(
            LogonTaskAction::Delete {},
            &|_| Err(io::Error::other("Task Scheduler is unavailable")),
            &|_| panic!("unrelated failure must not ask for UAC"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unavailable"));
    }

    #[test]
    fn task_consent_cancellation_reaches_the_run_fallback_unchanged() {
        let error = run_logon_task(
            LogonTaskAction::Delete {},
            &|_| Err(io::Error::from_raw_os_error(5)),
            &|_| Err(io::Error::from_raw_os_error(1223)),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(1223));
    }

    #[test]
    fn elevated_worker_round_trips_only_fixed_startup_actions() {
        let action = LogonTaskAction::Create {
            executable: PathBuf::from(r"C:\Program Files\Glide-Dock\glide-dock.exe"),
        };
        let payload = encode_worker_action(&action).unwrap();
        assert_eq!(
            parse_worker_request(&[STARTUP_WORKER_FLAG.into(), payload]).unwrap(),
            action
        );
        assert!(parse_worker_request(&[STARTUP_WORKER_FLAG.into()]).is_err());
        assert!(parse_worker_request(&[STARTUP_WORKER_FLAG.into(), "zz".into()]).is_err());
        assert!(parse_worker_request(&["--other".into(), "00".into()]).is_err());
    }

    #[test]
    fn worker_rejects_relative_or_unexpected_executables() {
        for executable in [
            PathBuf::from("glide-dock.exe"),
            PathBuf::from(r"C:\Windows\System32\cmd.exe"),
        ] {
            let payload = encode_worker_action(&LogonTaskAction::Create { executable }).unwrap();
            assert!(parse_worker_request(&[STARTUP_WORKER_FLAG.into(), payload]).is_err());
        }
    }

    #[test]
    fn access_denied_launches_the_dock_worker_through_uac() {
        let action = LogonTaskAction::Delete {};
        let source = Path::new(r"C:\Apps\Glide-Dock\glide-dock.exe");
        elevate_logon_task_with(action, source, &|executable, arguments| {
            assert_eq!(executable, source);
            let args = arguments
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>();
            assert_eq!(args[0], STARTUP_WORKER_FLAG);
            assert_eq!(
                parse_worker_request(&args).unwrap(),
                LogonTaskAction::Delete {}
            );
            Ok(0)
        })
        .unwrap();
    }

    #[test]
    fn canceled_uac_keeps_the_existing_error_contract() {
        let error = elevate_logon_task_with(
            LogonTaskAction::Delete {},
            Path::new("dock.exe"),
            &|_, _| Err(io::Error::from_raw_os_error(1223)),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(1223));
    }

    #[test]
    fn target_definition_requires_users_group_least_privilege() {
        let target = definition();
        assert!(target.matches_target(Path::new(r"C:\Apps\Glide-Dock\glide-dock.exe")));
        for changed in [
            LogonTaskDefinition {
                group_sid: None,
                ..target.clone()
            },
            LogonTaskDefinition {
                group_sid: Some("S-1-5-32-544".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                user_sid: Some("S-1-5-21-1-2-3-1002".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                trigger_user_sid: Some("S-1-5-21-1-2-3-1002".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                logon_type: Some("InteractiveToken".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                logon_type: Some("Password".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                run_level: Some("HighestAvailable".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                delay: Some("PT5S".into()),
                ..target.clone()
            },
            LogonTaskDefinition {
                command: r"C:\Other\glide-dock.exe".into(),
                ..target.clone()
            },
            LogonTaskDefinition {
                arguments: String::new(),
                ..target.clone()
            },
        ] {
            assert!(!changed.matches_target(Path::new(r"C:\Apps\Glide-Dock\glide-dock.exe")));
        }
    }

    #[test]
    fn parses_principal_trigger_and_action_from_task_xml() {
        let xml = r#"<Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
            <Principals><Principal id="Author"><GroupId>S-1-5-32-545</GroupId><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
            <Triggers><LogonTrigger id="GlideDockLogon"><Delay>PT0S</Delay></LogonTrigger></Triggers>
            <Actions><Exec><Command>C:\Apps &amp; Tools\glide-dock.exe</Command><Arguments>--autostart</Arguments></Exec></Actions>
        </Task>"#;
        let task = LogonTaskDefinition::from_xml(xml).expect("task definition");
        assert_eq!(task.command, r"C:\Apps & Tools\glide-dock.exe");
        assert_eq!(task.arguments, "--autostart");
        assert_eq!(task.user_sid, None);
        assert_eq!(task.group_sid.as_deref(), Some(USERS_GROUP_SID));
        assert_eq!(task.trigger_user_sid, None);
        assert_eq!(task.logon_type, None);
        assert_eq!(task.run_level.as_deref(), Some("LeastPrivilege"));
        assert_eq!(task.delay.as_deref(), Some("PT0S"));
        assert!(task.matches_target(Path::new(r"C:\Apps & Tools\glide-dock.exe")));
    }

    #[test]
    fn rejects_missing_or_malformed_task_xml() {
        assert!(LogonTaskDefinition::from_xml("<Task></Task>").is_none());
        assert!(LogonTaskDefinition::from_xml("not task XML").is_none());
    }

    #[test]
    fn users_group_sid_resolves_to_builtin_users() {
        assert!(user_matches_sid(Some(r"BUILTIN\Users"), USERS_GROUP_SID));
    }

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
    fn parses_quoted_run_command() {
        let xml = "<Command>C:\\Apps &amp; Tools\\glide-dock.exe</Command><Arguments>--autostart</Arguments>";
        let command = tag_text(xml, "Command").expect("command");
        assert_eq!(
            extract_executable(&format!("{} --autostart", windows_quote(&command))),
            Some(PathBuf::from(r"C:\Apps & Tools\glide-dock.exe"))
        );
    }

    #[test]
    fn matches_windows_exported_task_xml_with_omitted_defaults_and_group_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals>
    <Principal id="GlideDockPrincipal">
      <GroupId>BUILTIN\Users</GroupId>
    </Principal>
  </Principals>
  <Triggers>
    <LogonTrigger id="GlideDockLogon" />
  </Triggers>
  <Actions Context="GlideDockPrincipal">
    <Exec>
      <Command>C:\Apps\Glide-Dock\glide-dock.exe</Command>
      <Arguments>--autostart</Arguments>
    </Exec>
  </Actions>
</Task>"#;
        let parsed = LogonTaskDefinition::from_xml(xml).expect("exported task definition");
        assert_eq!(parsed.run_level, None);
        assert_eq!(parsed.delay, None);
        assert_eq!(parsed.logon_type, None);
        assert!(parsed.matches_target(Path::new(r"C:\Apps\Glide-Dock\glide-dock.exe")));
    }

    #[test]
    fn strips_verbatim_prefix_for_task_and_run_paths() {
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\C:\Apps\Glide-Dock\glide-dock.exe")),
            PathBuf::from(r"C:\Apps\Glide-Dock\glide-dock.exe")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"\\?\UNC\server\share\glide-dock.exe")),
            PathBuf::from(r"\\server\share\glide-dock.exe")
        );
        assert_eq!(
            strip_verbatim_prefix(Path::new(r"C:\Apps\Glide-Dock\glide-dock.exe")),
            PathBuf::from(r"C:\Apps\Glide-Dock\glide-dock.exe")
        );
    }
}
