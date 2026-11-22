//! Filesystem/registry integration tests with thread-local, isolated OS locations.

use super::*;
use std::cell::RefCell;
use std::sync::OnceLock;
use windows::Win32::System::Registry::{
    HKEY, KEY_ALL_ACCESS, REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
};

struct Sandbox {
    directory: PathBuf,
    registry_path: String,
    user: HKEY,
    machine: HKEY,
}
thread_local! { static SANDBOX: RefCell<Option<Sandbox>> = const { RefCell::new(None) }; }

pub(super) fn sandbox_active() -> bool {
    SANDBOX.with_borrow(Option::is_some)
}
pub(super) fn registry_override(scope: InstallScope) -> Option<HKEY> {
    SANDBOX.with_borrow(|value| {
        value.as_ref().map(|sandbox| match scope {
            InstallScope::CurrentUser => sandbox.user,
            InstallScope::AllUsers => sandbox.machine,
        })
    })
}
pub(super) fn folder_override(id: &GUID) -> Option<PathBuf> {
    SANDBOX.with_borrow(|value| {
        value.as_ref().map(|sandbox| {
            sandbox.directory.join(if *id == FOLDERID_Programs {
                "Programs"
            } else if *id == FOLDERID_CommonPrograms {
                "CommonPrograms"
            } else if *id == FOLDERID_Desktop {
                "Desktop"
            } else if *id == FOLDERID_PublicDesktop {
                "PublicDesktop"
            } else if *id == FOLDERID_LocalAppData {
                "LocalAppData"
            } else if *id == FOLDERID_RoamingAppData {
                "RoamingAppData"
            } else if *id == FOLDERID_ProgramFiles {
                "ProgramFiles"
            } else {
                "Windows"
            })
        })
    })
}
pub(super) fn payload() -> &'static [u8] {
    static PAYLOAD: OnceLock<Vec<u8>> = OnceLock::new();
    PAYLOAD.get_or_init(|| {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("glide-dock.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"new application").unwrap();
        zip.finish().unwrap().into_inner()
    })
}

struct Guard {
    _serial: std::sync::MutexGuard<'static, ()>,
}
impl Guard {
    fn new() -> Self {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let token = format!("{}-{}", std::process::id(), unique_nanos());
        let directory = std::env::temp_dir().join(format!("glide-flow-test-{token}"));
        fs::create_dir(&directory).unwrap();
        let registry_path = format!(r"Software\Glide-Dock-Installer-Tests\{token}");
        let open = |suffix| {
            let mut key = HKEY::default();
            let name = wide(&format!(r"{registry_path}\{suffix}"));
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(name.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_ALL_ACCESS,
                    None,
                    &mut key,
                    None,
                )
            }
            .ok()
            .unwrap();
            key
        };
        let sandbox = Sandbox {
            directory,
            user: open("User"),
            machine: open("Machine"),
            registry_path,
        };
        SANDBOX.with_borrow_mut(|value| {
            assert!(value.is_none());
            *value = Some(sandbox);
        });
        for folder in [
            &FOLDERID_Programs,
            &FOLDERID_Desktop,
            &FOLDERID_LocalAppData,
            &FOLDERID_RoamingAppData,
        ] {
            fs::create_dir_all(known_folder(folder).unwrap()).unwrap();
        }
        Self { _serial: serial }
    }
    fn directory(&self) -> PathBuf {
        SANDBOX.with_borrow(|value| value.as_ref().unwrap().directory.clone())
    }
    fn options(&self, update: bool) -> InstallOptions {
        InstallOptions {
            scope: InstallScope::CurrentUser,
            install_directory: self.directory().join("Glide-Dock"),
            create_desktop_shortcut: true,
            enable_startup: false,
            startup_mode: dock_platform_windows::startup::StartupMode::Run,
            launch_after_install: false,
            theme: ThemeMode::System,
            selected_apps: Vec::new(),
            update,
        }
    }
    fn legacy_install(&self, version: Option<&str>) -> InstallOptions {
        let options = self.options(true);
        fs::create_dir(&options.install_directory).unwrap();
        fs::write(
            options.install_directory.join("glide-dock.exe"),
            b"old application",
        )
        .unwrap();
        fs::write(
            options.install_directory.join("Glide-Dock-Setup.exe"),
            b"old embedded installer",
        )
        .unwrap();
        let key = Key::create(registry_root(options.scope), LEGACY_UNINSTALL_KEY).unwrap();
        key.set_text(
            "InstallLocation",
            &options.install_directory.to_string_lossy(),
        )
        .unwrap();
        key.set_text("UninstallString", "old uninstall command")
            .unwrap();
        if let Some(version) = version {
            key.set_text("DisplayVersion", version).unwrap();
        }
        options
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let sandbox = SANDBOX.with_borrow_mut(Option::take).unwrap();
        assert!(
            sandbox
                .registry_path
                .starts_with(r"Software\Glide-Dock-Installer-Tests\")
        );
        let name = wide(&sandbox.registry_path);
        unsafe {
            let _ = RegCloseKey(sandbox.user);
            let _ = RegCloseKey(sandbox.machine);
            RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()))
                .ok()
                .unwrap();
        }
        assert!(
            sandbox
                .directory
                .parent()
                .is_some_and(|parent| parent == std::env::temp_dir())
        );
        assert!(
            sandbox
                .directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("glide-flow-test-")
        );
        fs::remove_dir_all(sandbox.directory).unwrap();
    }
}

#[test]
fn fresh_install_deploys_only_standalone_uninstaller_and_correct_registration() {
    let sandbox = Guard::new();
    let record = install(&sandbox.options(false), |_| {}).unwrap();
    assert_eq!(
        fs::read(record.install_directory.join("glide-dock.exe")).unwrap(),
        b"new application"
    );
    assert_eq!(
        fs::read(record.install_directory.join(UNINSTALL_EXE)).unwrap(),
        b"MZ-uninstaller-fixture"
    );
    assert!(
        !record
            .install_directory
            .join("Glide-Dock-Setup.exe")
            .exists()
    );
    let key = Key::open(registry_root(record.scope), UNINSTALL_KEY, KEY_QUERY_VALUE)
        .unwrap()
        .unwrap();
    assert_eq!(
        key.text("UninstallString").unwrap().unwrap(),
        format!(
            "\"{}\" --uninstall",
            record.install_directory.join(UNINSTALL_EXE).display()
        )
    );
    assert!(
        known_folder(&FOLDERID_Programs)
            .unwrap()
            .join("Glide-Dock/Glide-Dock.lnk")
            .is_file()
    );
}

#[test]
fn upgrades_legacy_registration_then_uninstalls_without_touching_settings() {
    let sandbox = Guard::new();
    let options = sandbox.legacy_install(Some("0.4.1"));
    let settings = known_folder(&FOLDERID_RoamingAppData)
        .unwrap()
        .join("config.json");
    fs::write(&settings, b"user settings").unwrap();
    let record = install(&options, |_| {}).unwrap();
    assert_eq!(record.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert!(
        Key::open(
            registry_root(record.scope),
            LEGACY_UNINSTALL_KEY,
            KEY_QUERY_VALUE
        )
        .unwrap()
        .is_none()
    );
    assert!(
        !record
            .install_directory
            .join("Glide-Dock-Setup.exe")
            .exists()
    );
    assert_eq!(find_install().unwrap(), record);
    uninstall(&record, |_| {}).unwrap();
    assert!(!record.install_directory.exists());
    assert!(find_install().is_none());
    assert!(
        !known_folder(&FOLDERID_Programs)
            .unwrap()
            .join("Glide-Dock")
            .exists()
    );
    assert!(
        !known_folder(&FOLDERID_Desktop)
            .unwrap()
            .join("Glide-Dock.lnk")
            .exists()
    );
    assert_eq!(fs::read(settings).unwrap(), b"user settings");
}

#[test]
fn failed_upgrade_restores_old_program_and_uninstall_registration() {
    let sandbox = Guard::new();
    let options = sandbox.legacy_install(Some("0.4.1"));
    fs::write(
        known_folder(&FOLDERID_Programs).unwrap().join("Glide-Dock"),
        b"shortcut failure",
    )
    .unwrap();
    assert!(install(&options, |_| {}).is_err());
    assert_eq!(
        fs::read(options.install_directory.join("glide-dock.exe")).unwrap(),
        b"old application"
    );
    assert!(
        options
            .install_directory
            .join("Glide-Dock-Setup.exe")
            .is_file()
    );
    assert!(!options.install_directory.join(UNINSTALL_EXE).exists());
    let key = Key::open(
        registry_root(options.scope),
        LEGACY_UNINSTALL_KEY,
        KEY_QUERY_VALUE,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        key.text("UninstallString").unwrap().as_deref(),
        Some("old uninstall command")
    );
    assert_eq!(find_install().unwrap().version.as_deref(), Some("0.4.1"));
    assert!(!fs::read_dir(sandbox.directory()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".Glide-Dock-")
    }));
}

#[test]
fn worker_rejects_equal_older_and_missing_versions_before_changing_files() {
    for version in [
        Some(env!("CARGO_PKG_VERSION")),
        Some("0.5.0"),
        None,
        Some("bad-version"),
    ] {
        let sandbox = Guard::new();
        let options = sandbox.legacy_install(version);
        assert!(install(&options, |_| {}).is_err());
        assert_eq!(
            fs::read(options.install_directory.join("glide-dock.exe")).unwrap(),
            b"old application"
        );
        assert!(
            options
                .install_directory
                .join("Glide-Dock-Setup.exe")
                .is_file()
        );
    }
}

#[test]
fn version_changed_after_staging_is_rechecked_before_replacing_files() {
    let sandbox = Guard::new();
    let options = sandbox.legacy_install(Some("0.4.1"));
    let outcome = install(&options, |progress| {
        if progress.percent == 83 {
            Key::create(registry_root(options.scope), LEGACY_UNINSTALL_KEY)
                .unwrap()
                .set_text("DisplayVersion", env!("CARGO_PKG_VERSION"))
                .unwrap();
        }
    });
    assert!(outcome.is_err());
    assert_eq!(
        fs::read(options.install_directory.join("glide-dock.exe")).unwrap(),
        b"old application"
    );
    assert!(!options.install_directory.join(UNINSTALL_EXE).exists());
}

#[test]
fn locked_empty_directory_completes_uninstall_with_a_cleanup_warning() {
    use std::os::windows::fs::OpenOptionsExt;
    let sandbox = Guard::new();
    let record = install(&sandbox.options(false), |_| {}).unwrap();
    let directory_lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .custom_flags(0x0200_0000)
        .open(&record.install_directory)
        .unwrap();
    let warnings = uninstall(&record, |_| {}).unwrap();
    assert!(!warnings.is_empty());
    assert!(warnings.iter().any(|warning| warning.contains("空目录")));
    assert!(!record.install_directory.join("glide-dock.exe").exists());
    assert!(
        fs::read_dir(&record.install_directory)
            .unwrap()
            .next()
            .is_none()
    );
    assert!(find_install().is_none());
    drop(directory_lock);
}

#[test]
fn partial_uninstall_remains_discoverable_and_retries_after_a_file_unlocks() {
    use std::os::windows::fs::OpenOptionsExt;
    let sandbox = Guard::new();
    let record = install(&sandbox.options(false), |_| {}).unwrap();
    fs::remove_file(record.install_directory.join("glide-dock.exe")).unwrap();
    let remaining = record.install_directory.join("still-in-use.dll");
    fs::write(&remaining, b"locked application file").unwrap();
    let file_lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&remaining)
        .unwrap();
    assert_eq!(find_install().unwrap(), record);
    let error = uninstall(&record, |_| {}).unwrap_err().to_string();
    assert!(error.contains("卸载尚未完成"));
    assert!(!error.contains("安装记录已变更"));
    assert_eq!(find_install().unwrap(), record);
    assert!(remaining.is_file());
    drop(file_lock);
    assert!(uninstall(&record, |_| {}).unwrap().is_empty());
    assert!(!record.install_directory.exists());
    assert!(find_install().is_none());
}

#[test]
fn missing_install_tree_can_finish_registration_cleanup_and_repeat_safely() {
    let sandbox = Guard::new();
    let record = install(&sandbox.options(false), |_| {}).unwrap();
    remove_install_tree(
        &record.install_directory,
        record.install_directory.parent().unwrap(),
    )
    .unwrap();
    assert_eq!(find_install().unwrap(), record);
    assert!(uninstall(&record, |_| {}).unwrap().is_empty());
    assert!(find_install().is_none());
    assert!(uninstall(&record, |_| {}).unwrap().is_empty());
}

#[test]
fn changed_registration_does_not_delete_another_installation() {
    let sandbox = Guard::new();
    let record = install(&sandbox.options(false), |_| {}).unwrap();
    let other = sandbox.directory().join("Other-Glide-Dock");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("glide-dock.exe"), b"other installation").unwrap();
    Key::create(registry_root(record.scope), UNINSTALL_KEY)
        .unwrap()
        .set_text("InstallLocation", &other.to_string_lossy())
        .unwrap();
    assert!(
        uninstall(&record, |_| {})
            .unwrap_err()
            .to_string()
            .contains("安装记录已变更")
    );
    assert!(record.install_directory.join("glide-dock.exe").exists());
    assert_eq!(
        fs::read(other.join("glide-dock.exe")).unwrap(),
        b"other installation"
    );
}

#[test]
fn registration_cleanup_failure_is_a_warning_and_can_be_resumed() {
    let sandbox = Guard::new();
    let record = install(&sandbox.options(false), |_| {}).unwrap();
    let child = format!(r"{UNINSTALL_KEY}\Blocked");
    drop(Key::create(registry_root(record.scope), &child).unwrap());
    let warnings = uninstall(&record, |_| {}).unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("卸载记录未能清理"))
    );
    assert!(!record.install_directory.exists());
    assert_eq!(find_install().unwrap(), record);
    Key::delete(registry_root(record.scope), &child).unwrap();
    assert!(uninstall(&record, |_| {}).unwrap().is_empty());
    assert!(find_install().is_none());
}
