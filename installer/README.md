# Glide-Dock installer

The installer is built with Inno Setup 6.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

The script:

1. builds the Rust dock;
2. publishes the self-contained WinUI 3 settings center;
3. creates the portable package stage and ZIP;
4. compiles `installer\Glide-Dock.iss`;
5. writes `Glide-Dock-v<version>-Setup.exe` to
   `%LOCALAPPDATA%\Glide-Dock\installer`.

If Inno Setup is missing, the script tries:

```powershell
winget install --id JRSoftware.InnoSetup --exact --silent
```

You can also set `GLIDE_DOCK_ISCC` to an existing `ISCC.exe`.

## What the installer does

- installs to `%ProgramFiles%\Glide-Dock`;
- uses the FullColor Glide-Dock brand icon for Setup, shortcuts and the dock EXE;
- creates one Start Menu shortcut for the Dock (settings is opened from the
  Dock right-click → 设置... menu, not as a separate Start Menu entry) and an
  optional desktop shortcut;
- includes the dock, the self-contained settings app, docs and icon assets;
- does not enable autostart automatically; configure it in
  Settings → Behavior;
- on uninstall, stops the dock, its hidden guard, and settings processes and removes both
  the `Glide-Dock` scheduled task and the `HKCU\...\Run\Glide-Dock` value;
- preserves `%APPDATA%\Glide-Dock` (configuration and preferences).

The setup executable is currently unsigned, so Windows SmartScreen may show a
warning. This is expected for a locally built installer.
