//! Serializable IPC payloads exchanged between `glide-dock.exe` and the
//! on-demand `glide-settings.exe` UI process via stdin bootstrap and
//! `WM_COPYDATA`.

use serde::{Deserialize, Serialize};

use crate::{Appearance, Behavior, DockConfig, MonitorConfig, MotionConfig, ThemeMode};

/// Magic `COPYDATASTRUCT.dwData` tag identifying Glide-Dock UI JSON frames.
pub const COPYDATA_UI_MAGIC: usize = 0x4744_5549;

/// Maximum accepted `WM_COPYDATA` payload length in bytes (1 MiB).
pub const MAX_COPYDATA_BYTES: usize = 1024 * 1024;

/// Persisted startup mechanism selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupChoice {
    /// Task Scheduler logon task.
    Task,
    /// Current-user Run registry value.
    Run,
}

impl StartupChoice {
    /// Returns the Slint combo/button identifier (`"task"` or `"run"`).
    #[must_use]
    pub const fn as_id(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Run => "run",
        }
    }

    /// Parses `"run"` or `"task"`.
    #[must_use]
    pub fn from_id(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("run") {
            Self::Run
        } else {
            Self::Task
        }
    }
}

/// Startup privilege/configuration notice variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeKind {
    /// Built-in Administrator running without Admin Approval Mode.
    BuiltinAdminApproval,
    /// Non-built-in process running elevated.
    ElevatedWarning,
}

impl NoticeKind {
    /// Returns the UI variant identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::BuiltinAdminApproval => "builtin-admin",
            Self::ElevatedWarning => "elevated",
        }
    }

    /// Parses a CLI / UI notice identifier.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "builtin-admin" => Some(Self::BuiltinAdminApproval),
            "elevated" => Some(Self::ElevatedWarning),
            _ => None,
        }
    }
}

/// Mode requested when spawning `glide-settings.exe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiLaunchMode {
    /// Open the main settings window.
    Settings,
    /// Open a startup privilege notice dialog.
    Notice(NoticeKind),
}

/// Normalized editable settings slice shared between Dock and Settings UI.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsValues {
    /// Appearance configuration.
    pub appearance: Appearance,
    /// Interaction behavior configuration.
    pub behavior: Behavior,
    /// Spring motion parameters.
    pub motion: MotionConfig,
    /// Monitor selection configuration.
    pub monitor: MonitorConfig,
}

impl SettingsValues {
    /// Extracts settings fields from a full [`DockConfig`].
    #[must_use]
    pub fn from_config(config: &DockConfig) -> Self {
        Self {
            appearance: config.appearance.clone(),
            behavior: config.behavior.clone(),
            motion: config.motion.clone(),
            monitor: config.monitor.clone(),
        }
    }

    /// Clamps and repairs all nested configuration subsections.
    pub fn normalize(&mut self) {
        self.appearance.normalize();
        self.behavior.normalize();
        self.motion.normalize();
        self.monitor.normalize();
    }
}

/// Serializable summary of `UpdateService` status for the About/Updates tab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsUpdateView {
    /// Whether update checking/installing is supported by the installation mode.
    pub managed: bool,
    /// Whether a manifest check is currently running.
    pub checking: bool,
    /// Whether an installer download is in progress.
    pub downloading: bool,
    /// Whether an installer package has finished downloading.
    pub ready: bool,
    /// Whether the installer is launching.
    pub launching: bool,
    /// Download progress in `0..=100`.
    pub progress: i32,
    /// Localized status text.
    pub status: String,
    /// Installed application version string.
    pub current_version: String,
    /// Latest available version string (empty when none/unknown).
    pub latest_version: String,
    /// Release notes text.
    pub release_notes: String,
    /// Active source name (CNB / GitHub).
    pub source: String,
    /// CNB mirror check status line.
    pub cnb_status: String,
    /// GitHub check status line.
    pub github_status: String,
    /// Human-readable timestamp of the last check.
    pub last_check_text: String,
}

/// Incremental update pushed from `glide-dock.exe` to the open settings window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsUpdate {
    /// Optional status banner text.
    pub status: Option<String>,
    /// Optional diagnostics dump.
    pub diagnostics: Option<String>,
    /// Updated configuration values.
    pub values: Option<SettingsValues>,
    /// Startup toggle + mode.
    pub startup: Option<(bool, StartupChoice)>,
    /// Active settings UI font id.
    pub font_id: Option<String>,
    /// Whether Windows apps/system theme is dark.
    pub system_dark: Option<bool>,
    /// Latest updater snapshot.
    pub update_view: Option<SettingsUpdateView>,
    /// Whether the settings window should be shown/focused.
    pub show: bool,
}

/// Incremental update pushed from `glide-dock.exe` to the notice dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeUpdate {
    /// Optional notice variant change.
    pub kind: Option<NoticeKind>,
    /// Theme preference.
    pub theme: Option<ThemeMode>,
    /// Whether Windows theme is dark.
    pub system_dark: Option<bool>,
    /// Status feedback text (e.g. copy confirmation).
    pub feedback: Option<String>,
    /// Whether the notice dialog should be shown/focused.
    pub show: bool,
}

/// Initial state piped via `stdin` when spawning `glide-settings.exe`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiBootstrap {
    /// Initial window mode to display.
    pub mode: UiLaunchMode,
    /// Owner Dock HWND as a signed pointer-sized integer.
    pub dock_hwnd: isize,
    /// Application semantic version shown in Settings -> About.
    pub app_version: String,
    /// Current settings font id (`"noto-sans-sc"` or `"system"`).
    pub font_id: String,
    /// Initial settings values.
    pub values: SettingsValues,
    /// Whether logon startup is enabled.
    pub startup_enabled: bool,
    /// Preferred startup mode.
    pub startup_mode: StartupChoice,
    /// Current Windows dark theme flag.
    pub system_dark: bool,
    /// Initial updater status snapshot.
    pub update_view: SettingsUpdateView,
}

/// Messages sent from `glide-settings.exe` -> `glide-dock.exe` via `WM_COPYDATA`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SettingsCommand {
    /// Registers the settings process's IPC HWND with `glide-dock.exe`.
    RegisterUiWindow(isize),
    /// Applies live appearance/behavior/motion/monitor values.
    Apply(Box<SettingsValues>),
    /// Opens the configuration directory in Explorer.
    OpenConfigDirectory,
    /// Opens the log directory in Explorer.
    OpenLogDirectory,
    /// Resets settings to defaults.
    Reset,
    /// Reloads `config.json` from disk.
    Reload,
    /// Changes logon autostart enabled state and mode.
    SetStartup {
        /// Whether autostart is enabled.
        enabled: bool,
        /// Requested startup mechanism.
        mode: StartupChoice,
    },
    /// Changes logon autostart mode without altering enablement.
    SetStartupMode(StartupChoice),
    /// Persists settings UI font preference.
    SetFont(String),
    /// Requests a fresh runtime diagnostics report.
    RefreshDiagnostics,
    /// Copies runtime diagnostics to the clipboard.
    CopyDiagnostics,
    /// Copies current motion tuning JSON to the clipboard.
    CopyMotionConfig,
    /// Triggers a manual update check.
    CheckForUpdates,
    /// Downloads/launches the downloaded update installer.
    InstallUpdate,
    /// Opens CNB releases page.
    OpenCnbReleases,
    /// Opens GitHub releases page.
    OpenGithubReleases,
    /// Launches `secpol.msc` for the built-in admin notice dialog.
    OpenLocalSecurityPolicy,
    /// Copies the built-in admin registry command to the clipboard.
    CopyNoticeCommand,
    /// Requests `glide-dock.exe` to exit cleanly.
    QuitDock,
    /// Reports an async UI error from the helper process.
    LaunchFailed(String),
}

/// Messages pushed from `glide-dock.exe` -> `glide-settings.exe` via `WM_COPYDATA`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DockToUiEvent {
    /// Updates settings window state (and optionally shows/focuses it).
    UpdateSettings(Box<SettingsUpdate>),
    /// Updates notice dialog state (and optionally shows/focuses it).
    UpdateNotice(NoticeUpdate),
    /// Requests `glide-settings.exe` to close cleanly.
    CloseUi,
}
