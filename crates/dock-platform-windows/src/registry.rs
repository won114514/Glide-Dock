//! Exact-value registry operations for startup and installation records.

use std::io;
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, WIN32_ERROR,
};
use windows::Win32::System::Registry::*;
use windows::core::PCWSTR;

use crate::wide;

/// An owned registry key in the 64-bit view.
pub struct Key(HKEY);

/// Raw value saved for rollback, including its original type.
pub struct Value(REG_VALUE_TYPE, Vec<u8>);

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

fn missing(status: WIN32_ERROR) -> bool {
    matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
}

impl Key {
    /// Opens an existing key, distinguishing access errors from missing keys.
    pub fn open(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> io::Result<Option<Self>> {
        let path = wide(path);
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                root,
                PCWSTR(path.as_ptr()),
                None,
                access | KEY_WOW64_64KEY,
                &mut key,
            )
        };
        if missing(status) {
            return Ok(None);
        }
        check(status)?;
        Ok(Some(Self(key)))
    }

    /// Opens or creates a key for reading and writing.
    pub fn create(root: HKEY, path: &str) -> io::Result<Self> {
        let path = wide(path);
        let mut key = HKEY::default();
        check(unsafe {
            RegCreateKeyExW(
                root,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
                None,
                &mut key,
                None,
            )
        })?;
        Ok(Self(key))
    }

    /// Removes a product leaf key if present. Does not recursively delete keys.
    pub fn delete(root: HKEY, path: &str) -> io::Result<()> {
        let path = wide(path);
        let status =
            unsafe { RegDeleteKeyExW(root, PCWSTR(path.as_ptr()), KEY_WOW64_64KEY.0, None) };
        if missing(status) {
            Ok(())
        } else {
            check(status)
        }
    }

    /// Reads a value without changing its bytes.
    pub fn read(&self, name: &str) -> io::Result<Option<Value>> {
        let name = wide(name);
        let mut size = 0;
        let mut kind = REG_VALUE_TYPE::default();
        for _ in 0..4 {
            let mut data = vec![0; size as usize];
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    Some(&mut kind),
                    (!data.is_empty()).then_some(data.as_mut_ptr()),
                    Some(&mut size),
                )
            };
            if missing(status) {
                return Ok(None);
            }
            if size > 1024 * 1024 {
                return Err(io::Error::other("registry value is too large"));
            }
            if status == ERROR_MORE_DATA {
                continue;
            }
            check(status)?;
            if size as usize > data.len() {
                continue;
            }
            data.truncate(size as usize);
            return Ok(Some(Value(kind, data)));
        }
        Err(io::Error::other("registry value changed during read"))
    }

    /// Reads a `REG_SZ` or unexpanded `REG_EXPAND_SZ` as text.
    pub fn text(&self, name: &str) -> io::Result<Option<String>> {
        Ok(self.read(name)?.and_then(|Value(kind, data)| {
            if !matches!(kind, REG_SZ | REG_EXPAND_SZ) {
                return None;
            }
            let (words, _) = data.as_chunks::<2>();
            let units: Vec<_> = words
                .iter()
                .map(|word| u16::from_le_bytes(*word))
                .take_while(|unit| *unit != 0)
                .collect();
            String::from_utf16(&units).ok()
        }))
    }

    /// Reads a `REG_DWORD` value.
    pub fn dword(&self, name: &str) -> io::Result<Option<u32>> {
        let Some(Value(kind, data)) = self.read(name)? else {
            return Ok(None);
        };
        if kind != REG_DWORD {
            return Ok(None);
        }
        let Ok(bytes) = <[u8; 4]>::try_from(data.as_slice()) else {
            return Ok(None);
        };
        Ok(Some(u32::from_le_bytes(bytes)))
    }

    /// Writes a string value.
    pub fn set_text(&self, name: &str, text: &str) -> io::Result<()> {
        self.write(
            name,
            &Value(
                REG_SZ,
                wide(text).into_iter().flat_map(u16::to_le_bytes).collect(),
            ),
        )
    }

    /// Writes a DWORD value.
    pub fn set_dword(&self, name: &str, number: u32) -> io::Result<()> {
        self.write(name, &Value(REG_DWORD, number.to_le_bytes().to_vec()))
    }

    /// Writes or restores an exact value.
    pub fn write(&self, name: &str, value: &Value) -> io::Result<()> {
        let name = wide(name);
        check(unsafe {
            RegSetValueExW(self.0, PCWSTR(name.as_ptr()), None, value.0, Some(&value.1))
        })
    }

    /// Removes just the named value if present.
    pub fn remove(&self, name: &str) -> io::Result<()> {
        let name = wide(name);
        let status = unsafe { RegDeleteValueW(self.0, PCWSTR(name.as_ptr())) };
        if missing(status) {
            Ok(())
        } else {
            check(status)
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
