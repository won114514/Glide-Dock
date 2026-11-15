# Glide-Dock Settings

Glide-Dock's settings center is a small native **WinUI 3 / Windows App SDK** application.
It deliberately lives in a separate process:

- it gets a normal application window (focus, taskbar, z-order) while the dock stays at
  desktop level;
- a XAML renderer/event-loop cannot disturb the dock's zero-timer Win32 message loop;
- the settings process can be closed, restarted or rebuilt independently of the dock.

There is no WebView and no Slint dependency.

## Requirements

- Windows 10 1809+ / Windows 11 x64
- .NET 8 SDK (build only; the published output is self-contained)
- Windows App SDK 1.7 (restored from NuGet)
- Visual C++ runtime as required by Windows App SDK

## Build

From the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build-settings.ps1
```

The script publishes a self-contained build to `settings-app/publish/`.
The dock automatically finds it at:

1. `%GLIDE_DOCK_SETTINGS_EXE%`
2. `<glide-dock.exe directory>\GlideDock.Settings.exe`
3. `<glide-dock.exe directory>\settings\GlideDock.Settings.exe`
4. the repository's `settings-app/publish/GlideDock.Settings.exe` (development)
5. the repository's `settings-app/bin/x64/Release/.../GlideDock.Settings.exe` (development)

For distribution, copy the whole `settings-app/publish` folder next to the dock as
`settings/` (about 184 MB, including the bundled Noto Sans SC font). The folder already
contains `.NET`, Windows App SDK and the WinUI runtime, so users do not need a separate
.NET installation.

Manual build:

```powershell
dotnet publish settings-app/GlideDock.Settings.csproj `
  -c Release -r win-x64 --self-contained true `
  -p:Platform=x64 -p:WindowsAppSDKSelfContained=true `
  -o settings-app/publish
```

## Run

The dock launches it automatically:

```powershell
glide-dock.exe --settings
```

Manual diagnostic launch:

```powershell
settings-app\publish\GlideDock.Settings.exe --owner 0 --config preview\config.json
```

- `--owner <hwnd>` — dock window handle. When the dock exits it stops the settings
  process; the window also self-closes if the owner disappears. `0` means standalone.
- `--config <path>` — configuration file. When omitted:
  `%APPDATA%\Glide-Dock\config.json`.
- `--dock-exe <path>` — dock executable used for autostart registration. The dock
  passes it automatically; standalone launches fall back to `..\glide-dock.exe`
  next to the settings directory.

## Live sync protocol

For every edit the C# side:

1. updates its `JsonNode` copy of `config.json` (items and unknown fields preserved);
2. serializes it and sends `WM_COPYDATA` to the owner HWND (1 s timeout, abort-if-hung);
3. restarts a 350 ms status timer. **The dock is the single writer of `config.json`**:
   when attached, this process never rewrites the file. The dock validates the sender
   process, applies only `appearance`, `behavior`, `motion` and `monitor`, and saves after
   a 300 ms debounce. Standalone (`--owner 0`) mode still writes atomically with a
   temporary file + `Flush(true)` + rename.

`WM_COPYDATA.dwData` is the ASCII magic `GLID` (`0x474C4944`) and `wParam` is the
settings window handle. The payload starts with two little-endian `u32` values: protocol
version (`1`) and UTF-8 JSON byte length, followed by the JSON. The dock rejects unknown
versions, mismatched lengths, and payloads above 256 KiB before deserializing. It verifies
that the sender process is the settings child it spawned and applies only `appearance`,
`behavior`, `motion` and `monitor` fields. It keeps its own item list, so a settings
window opened with a stale copy can never reorder, duplicate, or delete dock entries.

## UI

- Custom extended title bar with the FullColor Glide-Dock brand icon, Mica backdrop,
  light/dark Fluent theme.
- The whole empty title-bar strip is draggable; the search box and caption buttons remain
  interactive. Window position is remembered in `settings-window.txt` next to the
  configuration file, so it does not jump back to the center on every launch.
- Left navigation: 常规 / 外观 / 行为 / 动画 / 诊断与维护 / 关于; the pane adapts to
  the available width and the window can be resized.
- **常规**: theme, accent, a bundled Noto Sans SC / Windows system-font switch, config
  path, reset, Dock layer and the expandable startup card. The font choice is stored in
  `settings-ui.json` beside the config and does not enter the Dock's runtime config.
- **常规**: theme, accent, config path, reset, Dock layer and the expandable startup card.
- Page switches use a short fade/slide; expandable cards fade their content in.
- Search shows a dedicated result page with a count and empty state, matching titles/descriptions/keywords.
- **关于**: compact version hero, config-folder/diagnostics actions, license and config path.
- The settings document watches `config.json`; tray-menu theme/layer changes are reflected live.
- Diagnostics page checks Dock / settings / Explorer process integrity, UAC registry
  values, guard-process presence, autostart mode, shortcut, compatibility flags and
  paths. It can refresh, copy the report, and open the log directory; the repair
  button is intentionally disabled because diagnostics is read-only.
- Global search with results that jump to the matching category.
- Native `Slider`, `ToggleSwitch`, `ComboBox`, `SelectorBar` and `Button` controls.
- Live values: icon size, spacing, padding, corner radius, opacity, edge margin,
  horizontal offset, maximum scale (1.05–3.50×), hover duration, elasticity and range.
  Each value label is bound to its `Slider.Value` through `ValueLabelConverter`, so the
  number follows the thumb continuously while dragging (no code-behind text updates).
- Animation page: six hover presets (Glide Zoom / Bounce / Wave / Fluid Press / Glow /
  Perspective), four launch presets (Glide Launch / Triple Bounce / Ripple Launch /
  Pulse Launch), animation on/off, reduced-motion following, recommended pairing hint,
  and **复制动画配置** which copies the `motion` JSON object to the clipboard. The page
  deliberately contains no embedded animation preview: the live dock is the preview.
- Accent swatches, running-indicator toggle, tooltip/launch-animation toggles.
- General → **开机自启动** is an expandable card: the header has the on/off
  toggle and the chevron reveals two startup modes:
  - **快速**: current-user logon task, no scheduler delay, needs UAC to register;
  - **兼容**: HKCU Run, no elevation, may be delayed by Windows.
  The mode is stored in `startup-mode.txt` next to `config.json`; autostart
  state is owned by Windows, not by config.json.
- Reset to defaults, open config folder, reload config.
- Window size is DPI-scaled and fixed so the card layout stays stable.

## Installer

`scripts/build-installer.ps1` publishes the custom WinUI 3 Setup.exe. It supports
per-user and per-machine installs, first-run configuration, in-place upgrades,
and a branded uninstaller. Settings is opened from the Dock right-click → 设置...
menu; there is no separate settings shortcut. User configuration remains under
`%APPDATA%\Glide-Dock` and is preserved on uninstall.

## Config fields written by the settings app

`appearance`: `theme`, `accent`, `layer` (`desktop` / `dynamic` / `topmost`),
`icon_size`, `spacing`, `padding`, `corner_radius`, `opacity`, `edge_margin`;
`monitor.offset`; `behavior`: `magnification`,
`launch_animation`, `show_tooltips`, `show_running_indicator`; `motion`: `enabled`,
`respect_reduced_motion`, `hover_preset`, `launch_preset`, `max_scale`, `range_slots`,
`hover_ms`, `elasticity`. The settings app normalizes `version` to 3. All other fields
(`items` including each item's `launch_type`, `next_item_id`, `appearance.position`,
`appearance.frosted_glass`, unknown keys) are preserved verbatim.
`frosted_glass` currently has no UI control; edit it in `config.json` if you need to
toggle the backdrop material.

## License

The Glide-Dock source is MIT OR Apache-2.0 (see `LICENSE-MIT` and `LICENSE-APACHE`). WinUI 3 and the Windows App SDK are Microsoft
software governed by the license terms included in the NuGet package and the published
folder (`WindowsAppSDK-license.txt`). Keep those notices when redistributing the self-contained
settings build.
