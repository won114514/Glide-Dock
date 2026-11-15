param(
    [switch]$Deploy
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$project = Join-Path $root 'settings-app\GlideDock.Settings.csproj'
$output = Join-Path $root 'settings-app\publish'

$localDotnet = Join-Path $root '.dotnet\dotnet.exe'
if (Test-Path $localDotnet) {
    $dotnet = $localDotnet
    $env:DOTNET_ROOT = Join-Path $root '.dotnet'
    $env:PATH = "$env:DOTNET_ROOT;$env:PATH"
    $env:DOTNET_CLI_HOME = Join-Path $root '.dotnet-home'
    $env:NUGET_PACKAGES = Join-Path $root '.nuget'
    # The DSH/Windows sandbox may not allow the real roaming NuGet folder.
    $env:APPDATA = Join-Path $root '.appdata'
    $env:LOCALAPPDATA = Join-Path $root '.localappdata'
    $env:TEMP = Join-Path $root 'target\temp'
    $env:TMP = $env:TEMP
    New-Item -ItemType Directory -Force $env:DOTNET_CLI_HOME, $env:NUGET_PACKAGES, $env:APPDATA, $env:LOCALAPPDATA, $env:TEMP | Out-Null
}
else {
    $dotnet = 'dotnet'
    if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
        throw 'The .NET 8 SDK was not found. Install it from https://dot.net or place a portable SDK at .dotnet\dotnet.exe.'
    }
}

$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = '1'
$env:DOTNET_NOLOGO = '1'

Write-Host 'Publishing Glide-Dock Settings (WinUI 3, self-contained)...'
# Remove stale runtime files so a previous Windows App SDK publish cannot leak
# into the new package.
if (Test-Path $output) {
    Remove-Item $output -Recurse -Force
}
& $dotnet publish $project `
    -c Release `
    -r win-x64 `
    --self-contained true `
    -p:Platform=x64 `
    -p:WindowsAppSDKSelfContained=true `
    -o $output

if ($LASTEXITCODE -ne 0) {
    throw "dotnet publish failed with exit code $LASTEXITCODE"
}

$licenseDir = Join-Path $root 'settings-app\licenses'
if (Test-Path $licenseDir) {
    Copy-Item (Join-Path $licenseDir '*') $output -Force
}

$bytes = (Get-ChildItem $output -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("Published to {0} ({1:N1} MB)" -f $output, ($bytes / 1MB))

if ($Deploy) {
    foreach ($target in @('target\release', 'target\debug')) {
        $targetRoot = Join-Path $root $target
        if (Test-Path $targetRoot) {
            $settingsDir = Join-Path $targetRoot 'settings'
            Write-Host "Deploying to $settingsDir"
            New-Item -ItemType Directory -Force $settingsDir | Out-Null
            Copy-Item (Join-Path $output '*') $settingsDir -Recurse -Force
        }
    }
}

Write-Host 'Done. Run: target\release\glide-dock.exe --settings'
