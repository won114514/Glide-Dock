//! Platform error type.

/// Errors raised by the Windows platform layer.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// A Win32 call failed.
    #[error("Win32 failure: {0}")]
    Win32(String),
    /// A COM call failed.
    #[error("COM failure: {0}")]
    Com(String),
    /// A shell operation failed.
    #[error("shell failure: {0}")]
    Shell(String),
    /// Icon extraction failed.
    #[error("icon extraction failed: {0}")]
    Icon(String),
    /// Window creation or positioning failed.
    #[error("window failure: {0}")]
    Window(String),
}

impl PlatformError {
    /// Builds a [`PlatformError::Win32`] from a `windows` error.
    #[must_use]
    pub fn win32(error: &windows::core::Error) -> Self {
        Self::Win32(format!("{} (0x{:08X})", error.message(), error.code().0))
    }

    /// Builds a [`PlatformError::Com`] from a `windows` error.
    #[must_use]
    pub fn com(error: &windows::core::Error) -> Self {
        Self::Com(format!("{} (0x{:08X})", error.message(), error.code().0))
    }

    /// Builds an [`PlatformError::Icon`] from a `windows` error.
    #[must_use]
    pub fn icon(error: &windows::core::Error) -> Self {
        Self::Icon(format!("{} (0x{:08X})", error.message(), error.code().0))
    }
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, PlatformError>;
