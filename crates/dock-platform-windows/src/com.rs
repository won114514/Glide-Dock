//! RAII COM initialization.

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

/// Keeps a single-threaded apartment alive for the lifetime of the guard.
pub struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    /// Initializes COM as an STA. If COM is already initialized on this thread
    /// with another model the guard becomes inert instead of failing hard.
    #[must_use]
    pub fn sta() -> Self {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let initialized = result.0 >= 0;
        if !initialized {
            tracing::warn!(
                code = result.0,
                "CoInitializeEx failed; shell features may degrade"
            );
        }
        Self { initialized }
    }

    /// Returns `true` when this guard owns the initialization.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
