//! Win32 process and elevation helpers.

use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, w};

/// Runs a fixed executable through UAC, then returns its exit code.
/// `arguments` is an already encoded Windows command line without argv[0].
pub fn run_elevated(exe: &Path, arguments: &str) -> windows::core::Result<u32> {
    let exe: Vec<_> = exe.as_os_str().encode_wide().chain([0]).collect();
    let arguments = crate::wide(arguments);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)?;
    }
    let process = OwnedHandle(info.hProcess);
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(windows::core::Error::from_thread());
    }
    let mut code = 0;
    unsafe {
        GetExitCodeProcess(process.0, &mut code)?;
    }
    Ok(code)
}

/// Starts a UAC-elevated worker without waiting for it to finish.
pub fn spawn_elevated_detached(exe: &Path, arguments: &str) -> windows::core::Result<()> {
    let exe: Vec<_> = exe.as_os_str().encode_wide().chain([0]).collect();
    let arguments = crate::wide(arguments);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)?;
    }
    if !info.hProcess.is_invalid() {
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

/// Encodes one argument according to the Windows command-line quoting rules.
pub fn quote_argument(argument: &str) -> String {
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                quoted.push(character);
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// Whether a UAC prompt was dismissed.
pub fn elevation_canceled(error: &windows::core::Error) -> bool {
    error.code() == windows::Win32::Foundation::ERROR_CANCELLED.to_hresult()
}

pub(crate) struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
