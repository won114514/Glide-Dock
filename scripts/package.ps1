# Builds a runnable Glide-Dock distribution outside the repository.
# The package contains the Rust Dock and its documentation. The standalone
# Tauri installer is built separately by build-installer.ps1.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Cannot read the workspace version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value
$local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($local)) { throw 'LOCALAPPDATA is unavailable' }
$stageRoot = Join-Path $local 'Glide-Dock'
$stage = Join-Path $stageRoot "Glide-Dock-v$version"
Write-Host "Building Glide-Dock $version..."
cargo build --locked --release -p glide-dock | Out-Host
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
if (-not (Test-Path -LiteralPath (Join-Path $root 'target\release\glide-dock.exe'))) { throw 'Rust release output is missing target\release\glide-dock.exe' }
if (Test-Path -LiteralPath $stage) {
    $resolvedStage = (Resolve-Path -LiteralPath $stage).Path
    $resolvedRoot = (Resolve-Path -LiteralPath $stageRoot).Path
    $prefix = $resolvedRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedStage.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to delete a stage directory outside ${stageRoot}: $resolvedStage" }
    Remove-Item -LiteralPath $resolvedStage -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item -LiteralPath 'target\release\glide-dock.exe' -Destination (Join-Path $stage 'glide-dock.exe') -Force
Copy-Item -LiteralPath 'README.md' -Destination (Join-Path $stage 'README.md') -Force
Copy-Item -LiteralPath 'docs\ANIMATION.md' -Destination (Join-Path $stage 'ANIMATION.md') -Force
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null
Copy-Item -Path 'docs\*.md' -Destination (Join-Path $stage 'docs') -Force
Copy-Item -LiteralPath 'assets' -Destination (Join-Path $stage 'assets') -Recurse -Force
# Slint loads the bundled Chinese font from the installed assets directory.
$fontDirectory = Join-Path $stage 'assets\fonts'
New-Item -ItemType Directory -Force -Path $fontDirectory | Out-Null
Copy-Item -LiteralPath 'settings-app\Assets\Fonts\NotoSansSC-VF.ttf' -Destination (Join-Path $fontDirectory 'NotoSansSC-VF.ttf') -Force
Copy-Item -LiteralPath 'settings-app\licenses\OFL-1.1.txt' -Destination (Join-Path $fontDirectory 'OFL-1.1.txt') -Force
Copy-Item -LiteralPath 'LICENSE-MIT' -Destination (Join-Path $stage 'LICENSE-MIT') -Force
Copy-Item -LiteralPath 'LICENSE-APACHE' -Destination (Join-Path $stage 'LICENSE-APACHE') -Force
@"
Glide-Dock v$version
Build: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
Runtime: Windows 10 1809+ / Windows 11 x64
Installer: standalone Rust + Tauri (requires Microsoft Edge WebView2 Runtime)
Dock: Rust + Slint/Win32 (no .NET runtime required)

Run this folder from a normal user directory. Do not run the build output
directly from a sandboxed/Low-integrity workspace.
"@ | Set-Content -LiteralPath (Join-Path $stage 'VERSION.txt') -Encoding UTF8
function Assert-MediumIntegrity([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "Missing file for integrity check: $Path" }
    $acl = & icacls $Path 2>&1
    if ($LASTEXITCODE -ne 0) { throw "icacls failed for ${Path}: $acl" }
    if (($acl | Out-String) -match 'Low Mandatory Level|低强制级别|低完整性|Низкий обязательный уровень') { throw "Refusing to package a Low-integrity executable: $Path" }
}
Assert-MediumIntegrity (Join-Path $stage 'glide-dock.exe')
$zip = Join-Path $stageRoot "Glide-Dock-v$version-payload.zip"
if (Test-Path -LiteralPath $zip) {
    $resolvedZip = [IO.Path]::GetFullPath($zip)
    $resolvedRoot = [IO.Path]::GetFullPath($stageRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedZip.StartsWith($resolvedRoot, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to delete a payload outside ${stageRoot}: $resolvedZip" }
    Remove-Item -LiteralPath $resolvedZip -Force
}
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
Write-Host "Payload:  $zip"
Write-Host "Folder:   $stage"
