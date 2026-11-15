//! Glide-Dock entry point: CLI, logging, window creation and message loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod guard;
mod launcher;
mod preview;
mod render_loop;
mod wndproc;

use std::path::PathBuf;
use std::time::Duration;

use dock_platform_windows::{
    ComGuard, DOCK_WINDOW_CLASS, SingleInstance, SingleInstanceOutcome, create_layered_popup,
    register_class, wait_for_desktop,
};
use dock_storage::ConfigStore;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

use crate::app::DockApp;
use crate::wndproc::dock_wndproc;

fn main() -> std::process::ExitCode {
    let options = match Options::parse(std::env::args().skip(1).collect()) {
        Ok(Some(options)) => options,
        Ok(None) => return std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    init_logging();
    let result = run(options);
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "fatal error");
            eprintln!("Glide-Dock: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Default)]
struct Options {
    selftest: bool,
    demo: bool,
    open_settings: bool,
    autostart: bool,
    guard: bool,
    guard_pid: Option<u32>,
    preview: Option<PathBuf>,
    config: Option<PathBuf>,
}

impl Options {
    fn parse(args: Vec<String>) -> Result<Option<Self>, String> {
        let mut options = Self::default();
        let mut index = 0;
        while index < args.len() {
            let argument = &args[index];
            match argument.as_str() {
                "--help" | "-h" => {
                    print_usage();
                    return Ok(None);
                }
                "--selftest" => options.selftest = true,
                "--demo" => options.demo = true,
                "--settings" => options.open_settings = true,
                "--autostart" => options.autostart = true,
                "--guard" => options.guard = true,
                "--guard-pid" => {
                    let value = args
                        .get(index + 1)
                        .filter(|value| !value.starts_with('-'))
                        .ok_or_else(|| "--guard-pid requires a number".to_string())?;
                    options.guard_pid = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid --guard-pid: {value}"))?,
                    );
                    index += 1;
                }
                "--render-preview" => {
                    let next = args.get(index + 1);
                    if let Some(path) = next.filter(|value| !value.starts_with('-')) {
                        options.preview = Some(PathBuf::from(path));
                        index += 1;
                    } else {
                        options.preview = Some(PathBuf::from("preview"));
                    }
                }
                "--config" => {
                    let path = args
                        .get(index + 1)
                        .filter(|value| !value.starts_with('-'))
                        .ok_or_else(|| "--config requires a path".to_string())?;
                    options.config = Some(PathBuf::from(path));
                    index += 1;
                }
                other => return Err(format!("unknown argument: {other}")),
            }
            index += 1;
        }
        Ok(Some(options))
    }
}

fn print_usage() {
    println!(
        "Glide-Dock {}\n\
         \n\
         Usage: glide-dock [options]\n\
         \n\
         Options:\n\
           --selftest             render one frame, verify idle state, exit\n\
           --render-preview [dir] render PNG previews into dir (default: preview)\n\
           --demo                 start with a curated sample dock\n\
           --settings             open the settings center on launch\n\
           --autostart            wait briefly for Explorer before showing\n\
           --guard                run the hidden watchdog (internal)\n\
           --guard-pid <pid>      dock process watched by --guard\n\
           --config <path>        use an explicit configuration file\n\
           -h, --help             show this help",
        env!("CARGO_PKG_VERSION")
    );
}

fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    if options.guard {
        return guard::run(
            options.guard_pid,
            options.config.as_deref(),
            options.autostart,
        );
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _com = ComGuard::sta();

    if let Some(directory) = &options.preview {
        preview::render_previews(directory)?;
        return Ok(());
    }

    let store = if let Some(path) = &options.config {
        ConfigStore::new(path.clone())
    } else if options.demo {
        ConfigStore::new("preview/demo-config.json")
    } else if options.selftest {
        // Never touch the user's configuration during a self test.
        ConfigStore::new(
            std::env::temp_dir().join(format!("glide-dock-selftest-{}.json", std::process::id())),
        )
    } else {
        ConfigStore::default_location()
    };
    let guard_config = store.path().to_path_buf();

    let _single_instance = if options.selftest {
        None
    } else {
        match SingleInstance::acquire() {
            SingleInstanceOutcome::Acquired(instance) => Some(instance),
            SingleInstanceOutcome::AlreadyRunning => {
                tracing::info!("another Glide-Dock instance is already running");
                return Ok(());
            }
            SingleInstanceOutcome::Failed(error) => {
                // Losing the mutex must not prevent the dock from working.
                tracing::warn!(%error, "single-instance mutex unavailable; continuing");
                None
            }
        }
    };

    if options.autostart {
        let ready = wait_for_desktop(Duration::from_secs(5));
        tracing::info!(ready, "autostart desktop readiness");
    }

    if options.demo && !options.selftest && !store.path().exists() {
        let demo = preview::demo_config();
        if let Err(error) = store.save(&demo) {
            tracing::warn!(%error, "could not write demo configuration");
        }
    }

    let outcome = store.load()?;
    let instance = register_class(DOCK_WINDOW_CLASS, Some(dock_wndproc))
        .map_err(|error| format!("failed to register window class: {error}"))?;
    let hwnd = create_layered_popup(instance, DOCK_WINDOW_CLASS, "Glide-Dock", 2, 2)
        .map_err(|error| format!("failed to create dock window: {error}"))?;

    let show = !options.selftest;
    let persist = !options.selftest;
    let app = match DockApp::new(hwnd, store, outcome, show, persist) {
        Ok(app) => app,
        Err(error) => {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
            return Err(Box::new(error));
        }
    };
    let app_pointer = Box::into_raw(Box::new(app));
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            app_pointer as isize,
        );
    }

    if !options.selftest && options.open_settings {
        unsafe {
            (*app_pointer).show_settings();
        }
    }

    if !options.selftest {
        if let Err(error) = guard::spawn(&guard_config, options.autostart) {
            tracing::warn!(%error, "could not start dock guard");
        }
    }

    if options.selftest {
        let passed = unsafe { (*app_pointer).selftest() };
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            let mut app = Box::from_raw(app_pointer);
            app.shutdown_without_save();
        }
        if passed {
            println!("selftest: OK");
            Ok(())
        } else {
            Err("selftest failed".into())
        }
    } else {
        message_loop();
        unsafe {
            let mut app = Box::from_raw(app_pointer);
            app.shutdown();
        }
        Ok(())
    }
}

fn message_loop() {
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false);

    let log_file = std::fs::create_dir_all(dock_storage::log_dir())
        .ok()
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dock_storage::log_dir().join("glide-dock.log"))
                .ok()
        });
    let file_layer = log_file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::sync::Mutex::new(file))
            .with_target(false)
            .with_ansi(false)
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file_layer)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_autostart_flag() {
        let options = Options::parse(vec!["--autostart".to_string()])
            .expect("parse")
            .expect("options");
        assert!(options.autostart);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(Options::parse(vec!["--nope".to_string()]).is_err());
    }

    #[test]
    fn parses_guard_arguments() {
        let options = Options::parse(vec![
            "--guard".to_string(),
            "--guard-pid".to_string(),
            "42".to_string(),
        ])
        .expect("parse")
        .expect("options");
        assert!(options.guard);
        assert_eq!(options.guard_pid, Some(42));
    }

    #[test]
    fn rejects_invalid_guard_pid() {
        assert!(
            Options::parse(vec![
                "--guard".to_string(),
                "--guard-pid".to_string(),
                "nope".to_string(),
            ])
            .is_err()
        );
    }
}
