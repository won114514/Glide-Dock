# Glide-Dock installer

Build the branded WinUI 3 installer with:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

The build publishes the Rust Dock and WinUI 3 settings app, stages the runnable
distribution, compresses it into the Setup application's embedded payload, and
publishes one `Glide-Dock-v<version>-Setup.exe` to
`%LOCALAPPDATA%\Glide-Dock\installer`. A matching `.sha256` file is written
beside the installer. Use `-OutputDirectory <path>` to choose another output
directory, for example `dist\installer` in automation.

GitHub Actions runs this full build on pushes and pull requests. Pushing a tag
whose name matches the Cargo workspace version (for example `v0.4.1`) publishes
the Setup executable and its SHA-256 file as a GitHub Release after all Windows
checks pass.

The setup application is authored in `setup-app` and uses the Glide-Dock logo,
application icon, and Dock preview. It supports current-user and all-user
installation, custom install paths, an optional desktop shortcut, and a first-run
page for theme, startup, and available recommended apps. Machine-level file and
registry operations run in an elevated worker; user profile configuration stays
with the interactive user.

When a prior Inno Setup installation is detected, Setup offers in-place upgrade
or cancel. `--update` performs the registered in-place update without installation
choices or first-run setup. Parallel installs are not supported. The custom
uninstaller removes program files, shortcuts, and Glide-Dock startup entries but
preserves `%APPDATA%\Glide-Dock`.

The generated installer is not code signed. Windows SmartScreen may warn about
locally built releases.
