# Builds the Inno Setup installer for Glide-Dock.
#
# Prerequisites:
#   - Inno Setup 6 (ISCC.exe). The script tries winget when it is missing.
#   - The package stage produced by scripts/package.ps1.

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Cannot read the workspace version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value

$local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
$stage = Join-Path $local "Glide-Dock\Glide-Dock-v$version"
$output = Join-Path $local 'Glide-Dock\installer'

function Find-Iscc([string] $localAppData) {
    if ($env:GLIDE_DOCK_ISCC -and (Test-Path $env:GLIDE_DOCK_ISCC)) {
        return $env:GLIDE_DOCK_ISCC
    }
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }

    $candidates = @(
        (Join-Path $localAppData 'Programs\Inno Setup 6\ISCC.exe'),
        'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
        'C:\Program Files\Inno Setup 6\ISCC.exe'
    )
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

# Resolve before package.ps1 because build-settings.ps1 temporarily redirects
# LOCALAPPDATA to the repository for the .NET SDK sandbox.
$iscc = Find-Iscc $local
if (-not $iscc) {
    Write-Host 'Inno Setup 6 not found; trying winget...'
    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        throw 'Inno Setup 6 is required and winget is unavailable. Install it from https://jrsoftware.org/isdl.php'
    }
    & winget.exe install --id JRSoftware.InnoSetup --exact --silent --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget could not install Inno Setup (exit $LASTEXITCODE)."
    }
    $iscc = Find-Iscc $local
}
if (-not $iscc) {
    throw 'ISCC.exe was not found after installing Inno Setup 6.'
}

& (Join-Path $PSScriptRoot 'package.ps1')
if ($LASTEXITCODE -ne 0) { throw "package stage failed with exit code $LASTEXITCODE" }
if (-not (Test-Path (Join-Path $stage 'glide-dock.exe'))) {
    throw "Package stage is missing glide-dock.exe: $stage"
}

New-Item -ItemType Directory -Force $output | Out-Null
$iss = Join-Path $root 'installer\Glide-Dock.iss'
Write-Host "Building installer with $iscc"
& $iscc "/DStageDir=$stage" "/DVersion=$version" "/DOutputDir=$output" $iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed with exit code $LASTEXITCODE" }

$setup = Join-Path $output "Glide-Dock-v$version-Setup.exe"
if (-not (Test-Path $setup)) { throw "Installer output is missing: $setup" }
$size = [math]::Round((Get-Item $setup).Length / 1MB, 1)
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash
Write-Host "Installer: $setup ($size MB)"
Write-Host "SHA-256:   $hash"
