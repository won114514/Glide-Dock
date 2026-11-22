//! Keeps every setup-owned UAC request attached to the installer's product identity.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use dock_platform_windows::startup::{self, LogonTaskAction};

pub const STARTUP_WORKER_FLAG: &str = "--glide-startup-worker";
const WORKER_FILE_NAME: &str = "Glide-Dock 卸载程序.exe";

pub fn is_inside(source: &Path, directory: &Path) -> bool {
    let source = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    source
        .ancestors()
        .any(|ancestor| super::same_path(ancestor, &directory))
}

pub fn operation_executable(source: &Path, installed: Option<&Path>) -> io::Result<PathBuf> {
    if installed.is_some_and(|directory| is_inside(source, directory)) {
        return Err(io::Error::other(
            "请从安装目录外运行安装程序；卸载程序必须先切换到临时目录。",
        ));
    }
    Ok(source.to_path_buf())
}

pub fn uninstall_working_directory(
    source: &Path,
    installed: &Path,
    temporary: &Path,
) -> io::Result<PathBuf> {
    let directory = if is_inside(source, installed) {
        temporary
    } else {
        source
            .parent()
            .ok_or_else(|| io::Error::other("卸载程序路径无效。"))?
    };
    if is_inside(directory, installed) {
        return Err(io::Error::other("卸载程序的工作目录必须位于安装目录之外。"));
    }
    Ok(directory.to_path_buf())
}

pub fn create_uninstall_copy(source: &Path, root: &Path) -> io::Result<PathBuf> {
    let token = format!("{}-{}", std::process::id(), super::unique_nanos());
    copy_uninstaller(source, root, &token)
}

fn copy_uninstaller(source: &Path, root: &Path, token: &str) -> io::Result<PathBuf> {
    if !super::valid_token(token) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid worker identity",
        ));
    }
    let mut source = File::open(source)?;
    let directory = root.join(format!("Glide-Dock-Uninstall-{token}"));
    fs::create_dir(&directory)?;
    let target = directory.join(WORKER_FILE_NAME);
    let mut destination = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(destination) => destination,
        Err(error) => {
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
    };
    if let Err(error) = io::copy(&mut source, &mut destination).and_then(|_| destination.flush()) {
        drop(destination);
        remove_worker_copy(&target);
        return Err(error);
    }
    Ok(target)
}

pub fn remove_worker_copy(copy: &Path) {
    let Some(directory) = copy.parent() else {
        return;
    };
    if copy.file_name().is_none_or(|name| name != WORKER_FILE_NAME)
        || directory
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().starts_with("Glide-Dock-Uninstall-"))
    {
        return;
    }
    let _ = fs::remove_file(copy);
    // Never recurse: a collision or an unrelated file must remain untouched.
    let _ = fs::remove_dir(directory);
}

pub fn elevate_startup_task(action: LogonTaskAction) -> io::Result<()> {
    elevate_startup_task_with(action, &std::env::current_exe()?, &|copy, arguments| {
        dock_platform_windows::process::run_elevated(copy, arguments).map_err(|error| {
            if dock_platform_windows::process::elevation_canceled(&error) {
                io::Error::from_raw_os_error(1223)
            } else {
                io::Error::other(error.to_string())
            }
        })
    })
}

fn elevate_startup_task_with(
    action: LogonTaskAction,
    source: &Path,
    run: &impl Fn(&Path, &str) -> io::Result<u32>,
) -> io::Result<()> {
    let payload = super::encode(&action).map_err(io::Error::other)?;
    let token = format!("{}-{}", std::process::id(), super::unique_nanos());
    let result = run(source, &format!("{STARTUP_WORKER_FLAG} {token} {payload}"));
    match result? {
        0 => Ok(()),
        code => Err(io::Error::other(format!(
            "登录启动项设置未能完成（退出码 {code}）。"
        ))),
    }
}

fn parse_startup_request(args: &[String]) -> Result<LogonTaskAction, String> {
    if args.len() != 3 || args[0] != STARTUP_WORKER_FLAG || !super::valid_token(&args[1]) {
        return Err("启动项工作进程参数无效。".into());
    }
    let action = super::decode::<LogonTaskAction>(&args[2])?;
    if let LogonTaskAction::Create { executable } = &action
        && (!executable.is_absolute()
            || executable.file_name().is_none_or(|name| {
                !name
                    .to_string_lossy()
                    .eq_ignore_ascii_case("glide-dock.exe")
            }))
    {
        return Err("登录启动项必须指向已安装的 Glide-Dock。".into());
    }
    Ok(action)
}

pub fn run_startup_worker(args: &[String]) -> i32 {
    let Ok(action) = parse_startup_request(args) else {
        return 2;
    };
    if !dock_platform_windows::is_elevated() {
        return 5;
    }
    // This process already has consent; the system tool inherits its token.
    match startup::apply_logon_task(action) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        directory: PathBuf,
        source: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "Glide-Dock-identity-test-{}-{}",
                std::process::id(),
                super::super::unique_nanos()
            ));
            fs::create_dir(&directory).unwrap();
            let source = directory.join("original-setup.exe");
            fs::write(&source, b"original executable").unwrap();
            Self { directory, source }
        }
        fn assert_no_copy(&self) {
            assert_eq!(fs::read_dir(&self.directory).unwrap().count(), 1);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.source);
            let _ = fs::remove_dir(&self.directory);
        }
    }

    #[test]
    fn uninstall_uses_a_working_directory_outside_the_install_tree() {
        let installed = Path::new(r"C:\Apps\Glide-Dock");
        let temporary = Path::new(r"C:\Users\User\Temp");
        assert_eq!(
            uninstall_working_directory(&installed.join(WORKER_FILE_NAME), installed, temporary)
                .unwrap(),
            temporary
        );
        assert_eq!(
            uninstall_working_directory(&temporary.join(WORKER_FILE_NAME), installed, temporary)
                .unwrap(),
            temporary
        );
        assert!(
            uninstall_working_directory(&installed.join(WORKER_FILE_NAME), installed, installed)
                .is_err()
        );
    }

    #[test]
    fn install_and_update_keep_original_source_without_creating_a_copy() {
        let fixture = Fixture::new();
        let installed = fixture.directory.join("installed");
        for existing in [None, Some(installed.as_path())] {
            assert_eq!(
                operation_executable(&fixture.source, existing).unwrap(),
                fixture.source
            );
            fixture.assert_no_copy();
        }
        assert!(operation_executable(&fixture.source, Some(&fixture.directory)).is_err());
        fixture.assert_no_copy();
    }

    #[test]
    fn uninstall_worker_reuses_the_single_relocated_copy() {
        let fixture = Fixture::new();
        let installed = fixture.directory.join("installed");
        fs::create_dir(&installed).unwrap();
        let source = installed.join(WORKER_FILE_NAME);
        fs::write(&source, b"uninstaller").unwrap();
        let copy = create_uninstall_copy(&source, &fixture.directory).unwrap();
        assert_eq!(operation_executable(&copy, Some(&installed)).unwrap(), copy);
        assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 3);
        remove_worker_copy(&copy);
        fs::remove_file(source).unwrap();
        fs::remove_dir(installed).unwrap();
        fixture.assert_no_copy();
    }

    #[test]
    fn uninstall_copy_has_uninstaller_identity_and_preserves_contents() {
        let fixture = Fixture::new();
        let copy = copy_uninstaller(&fixture.source, &fixture.directory, "1-1").unwrap();
        assert_eq!(copy.file_name().unwrap(), WORKER_FILE_NAME);
        assert_eq!(fs::read(&copy).unwrap(), fs::read(&fixture.source).unwrap());
        assert!(!is_inside(&copy, &fixture.directory.join("installed")));
        remove_worker_copy(&copy);
        fixture.assert_no_copy();
    }

    #[test]
    fn colliding_uninstall_directory_is_not_overwritten_or_deleted() {
        let fixture = Fixture::new();
        let copy = copy_uninstaller(&fixture.source, &fixture.directory, "1-1").unwrap();
        fs::write(&copy, b"existing copy").unwrap();
        let error = copy_uninstaller(&fixture.source, &fixture.directory, "1-1").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&copy).unwrap(), b"existing copy");
        remove_worker_copy(&copy);
        fixture.assert_no_copy();
    }

    #[test]
    fn source_conflict_checks_nested_and_case_insensitive_paths() {
        assert!(is_inside(
            Path::new(r"D:\Apps\Glide-Dock\Glide-Dock 卸载程序.exe"),
            Path::new(r"d:\apps\glide-dock")
        ));
        assert!(is_inside(
            Path::new(r"D:\Apps\Glide-Dock\nested\setup.exe"),
            Path::new(r"D:\Apps\Glide-Dock")
        ));
        assert!(!is_inside(
            Path::new(r"D:\Apps\Glide-Dock-new\setup.exe"),
            Path::new(r"D:\Apps\Glide-Dock")
        ));
    }

    #[test]
    fn missing_source_and_invalid_token_create_no_copy() {
        let fixture = Fixture::new();
        assert!(
            copy_uninstaller(
                &fixture.directory.join("missing.exe"),
                &fixture.directory,
                "1-1"
            )
            .is_err()
        );
        assert!(copy_uninstaller(&fixture.source, &fixture.directory, "../bad").is_err());
        fixture.assert_no_copy();
    }

    #[test]
    fn startup_uac_runs_original_executable_without_copying() {
        let fixture = Fixture::new();
        let action = LogonTaskAction::Create {
            executable: PathBuf::from(r"D:\Program Files\中文目录\Glide-Dock\glide-dock.exe"),
        };
        elevate_startup_task_with(action.clone(), &fixture.source, &|source, arguments| {
            assert_eq!(source, fixture.source);
            let args = arguments
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>();
            assert_eq!(parse_startup_request(&args).unwrap(), action);
            fixture.assert_no_copy();
            Ok(0)
        })
        .unwrap();
        fixture.assert_no_copy();
    }

    #[test]
    fn canceled_startup_uac_preserves_run_fallback_and_original_file() {
        let fixture = Fixture::new();
        let error =
            elevate_startup_task_with(LogonTaskAction::Delete {}, &fixture.source, &|_, _| {
                Err(io::Error::from_raw_os_error(1223))
            })
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(1223));
        assert_eq!(fs::read(&fixture.source).unwrap(), b"original executable");
        fixture.assert_no_copy();
    }

    #[test]
    fn failed_startup_worker_preserves_original_file() {
        let fixture = Fixture::new();
        let error =
            elevate_startup_task_with(LogonTaskAction::Delete {}, &fixture.source, &|_, _| Ok(5))
                .unwrap_err();
        assert!(error.to_string().contains('5'));
        fixture.assert_no_copy();
    }

    #[test]
    fn startup_entry_rejects_arbitrary_commands_and_malformed_requests() {
        let request = |payload: &serde_json::Value| {
            vec![
                STARTUP_WORKER_FLAG.into(),
                "1-1".into(),
                super::super::encode(payload).unwrap(),
            ]
        };
        for payload in [
            serde_json::json!({"action":"run","command":"cmd.exe"}),
            serde_json::json!({"action":"delete","command":"cmd.exe"}),
            serde_json::json!({"action":"create","executable":"glide-dock.exe"}),
            serde_json::json!({"action":"create","executable":r"C:\Windows\System32\cmd.exe"}),
        ] {
            assert!(parse_startup_request(&request(&payload)).is_err());
        }
        assert!(parse_startup_request(&[]).is_err());
    }
}
