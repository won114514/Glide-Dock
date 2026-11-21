# Glide-Dock installer

The standalone installer uses Rust + Tauri with Microsoft Fluent UI React v9
and the official Fluent UI System Icons. Buttons, fields, radio buttons,
checkboxes, switches, progress bars, tabs, accordions, and confirmation dialogs
use the library's components. CSS controls the window layout and spacing.

## Build

Install Rust and Node.js 22.12+ with npm, then run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

The script first runs `npm ci` against `installer/ui/package-lock.json`,
type-checks the React frontend, and builds its production assets in
`target/installer-ui`. It then creates the Rust Dock distribution, compresses
it as a payload, sets `GLIDE_DOCK_PAYLOAD_ZIP`, and embeds the payload and
frontend while building the `glide-setup` crate. No CDN or frontend server is
used at runtime. Notices for bundled third-party modules are embedded as
`THIRD-PARTY-NOTICES.txt` alongside the web assets.

The resulting `Glide-Dock-v<version>-Setup.exe` and matching `.sha256` file
are written to `%LOCALAPPDATA%\Glide-Dock\installer`, or to the directory
passed with `-OutputDirectory` (for example `dist\installer` in CI).
The intermediate payload copy is removed after writing the installer and
checksum. An existing payload can be supplied through `GLIDE_DOCK_PAYLOAD_ZIP`.

For frontend development and checks:

```powershell
.\scripts\build-installer-ui.ps1
cd installer\ui
npx playwright install chromium
npm test
```

The frontend helper accepts `-SkipRestore` when dependencies are already
installed. If npm is unavailable on PATH, set `GLIDE_DOCK_NPM_CLI` to an
existing `npm-cli.js`. Tests run the production build in a browser with a
simulated Tauri backend; they never install, update, or uninstall the real app.
Set `PLAYWRIGHT_CHANNEL=msedge` to use an installed Edge browser.

## Startup and runtime

The build explicitly sets and verifies a Medium integrity label on the generated
installer, including outputs placed in a Low-integrity workspace. It preserves
the file's access rules and does not change directory permissions. If this step
fails, the build stops before publishing a checksum. The wizard opens by
double-clicking without elevation. Installing or maintaining an all-user
installation can still request UAC when the operation begins.

Running a Low-integrity executable directly from Cargo's target directory
shows an explanation before starting WebView2. Window creation failures
display their actual error instead of panicking or assuming WebView2 is missing.
The installer requires the Microsoft Edge WebView2 Runtime. The installed Dock
and its Slint settings UI remain .NET-free; `settings-app` and `setup-app`
retain their existing source but do not participate in current packaging.

## Interface and operations

The initial window is 840x620 logical pixels, bounded by the monitor's work area
and Windows display scaling. Its minimum size is 560x400. A compact title bar
uses 16px SVG icons and equal 40x32px caption controls. The left navigation
tracks one selected step; narrow windows keep the step icons and hide the
secondary text. The main content scrolls while the footer remains accessible.
The first-run page groups preferences into three rows and optionally expands
the application picker. Light and dark themes follow Windows.

Long errors appear in a scrolling Fluent MessageBar, with a short status in the
footer. Installation and uninstall operations prevent duplicate submissions
and window closure until the worker finishes. The installer supports current-
user and all-user installation, custom folders, optional desktop shortcuts,
first-run preferences, updates, and uninstall. Updates keep existing settings;
uninstall preserves personal data unless the user explicitly selects deletion.

The Dock payload includes the bundled Chinese font and its OFL license under
`assets/fonts`. The executable is not code signed, so Windows SmartScreen may
warn about locally built releases.
