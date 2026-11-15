# Builds a runnable Glide-Dock distribution outside the repository.
#
# The workspace itself can carry a Low mandatory-integrity label (for example
# when it is managed by a sandbox). Copying the build to a normal user folder
# gives the new files a Medium label, which is required for child applications
# to bind AF_UNIX sockets and write AppData normally.

$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Cannot read the workspace version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value

# Resolve the real LocalAppData *before* build-settings.ps1 redirects the
# child process environment for the .NET SDK.
$local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($local)) { throw 'LOCALAPPDATA is unavailable' }
$stageRoot = Join-Path $local 'Glide-Dock'
$stage = Join-Path $stageRoot "Glide-Dock-v$version"

Write-Host "Building Glide-Dock $version..."
cargo build --release -p glide-dock | Out-Host
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

& (Join-Path $PSScriptRoot 'build-settings.ps1')
if ($LASTEXITCODE -ne 0) { throw "settings build failed with exit code $LASTEXITCODE" }

if (-not (Test-Path (Join-Path $root 'settings-app\publish\GlideDock.Settings.exe'))) {
    throw 'settings publish output is missing GlideDock.Settings.exe'
}

if (Test-Path $stage) {
    $resolvedStage = (Resolve-Path $stage).Path
    $resolvedRoot = (Resolve-Path $stageRoot).Path
    if (-not $resolvedStage.StartsWith($resolvedRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to delete a stage directory outside ${stageRoot}: $resolvedStage"
    }
    Remove-Item $resolvedStage -Recurse -Force
}
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item 'target\release\glide-dock.exe' (Join-Path $stage 'glide-dock.exe') -Force
New-Item -ItemType Directory -Force (Join-Path $stage 'settings') | Out-Null
Copy-Item 'settings-app\publish\*' (Join-Path $stage 'settings') -Recurse -Force
# Debug symbols are not needed in a user-facing package.
Get-ChildItem (Join-Path $stage 'settings') -Filter '*.pdb' -File -ErrorAction SilentlyContinue |
    Remove-Item -Force
Copy-Item 'README.md' (Join-Path $stage 'README.md') -Force
Copy-Item 'docs\ANIMATION.md' (Join-Path $stage 'ANIMATION.md') -Force
New-Item -ItemType Directory -Force (Join-Path $stage 'docs') | Out-Null
Copy-Item 'docs\*.md' (Join-Path $stage 'docs') -Force
Copy-Item 'assets' (Join-Path $stage 'assets') -Recurse -Force
Copy-Item 'LICENSE-MIT' (Join-Path $stage 'LICENSE-MIT') -Force
Copy-Item 'LICENSE-APACHE' (Join-Path $stage 'LICENSE-APACHE') -Force
if (Test-Path 'settings-app\THIRD-PARTY-NOTICES.md') {
    Copy-Item 'settings-app\THIRD-PARTY-NOTICES.md' (Join-Path $stage 'THIRD-PARTY-NOTICES.md') -Force
}

@"
Glide-Dock v$version
Build: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
Runtime: Windows 10 1809+ / Windows 11 x64
Settings: settings\GlideDock.Settings.exe (self-contained WinUI 3)

Run this folder from a normal user directory. Do not run the build output
directly from a sandboxed/Low-integrity workspace.
"@ | Set-Content (Join-Path $stage 'VERSION.txt') -Encoding UTF8

# icacls is localized; match the labels used by the common Windows UI
# languages and fail closed if the command itself fails.
function Assert-MediumIntegrity([string] $Path) {
    if (-not (Test-Path $Path)) { throw "Missing file for integrity check: $Path" }
    $acl = & icacls $Path 2>&1
    if ($LASTEXITCODE -ne 0) { throw "icacls failed for ${Path}: $acl" }
    $text = ($acl | Out-String)
    if ($text -match 'Low Mandatory Level|低强制级别|低完整性|Низкий обязательный уровень') {
        throw "Refusing to package a Low-integrity executable: $Path"
    }
}

Assert-MediumIntegrity (Join-Path $stage 'glide-dock.exe')
Assert-MediumIntegrity (Join-Path $stage 'settings\GlideDock.Settings.exe')

$zip = Join-Path $local "Glide-Dock\Glide-Dock-v$version.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

$size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Packaged: $zip ($size MB)"
Write-Host "Folder:   $stage"
