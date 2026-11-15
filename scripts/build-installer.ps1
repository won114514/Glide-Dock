param(
    [string]$OutputDirectory
)

# Builds the branded WinUI 3 Glide-Dock installer as one self-contained EXE.

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Cannot read the workspace version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value

$local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($local)) { throw 'LOCALAPPDATA is unavailable' }
$stage = Join-Path $local "Glide-Dock\Glide-Dock-v$version"
$output = if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    Join-Path $local 'Glide-Dock\installer'
} else {
    [System.IO.Path]::GetFullPath($OutputDirectory)
}
$payload = Join-Path $output "Glide-Dock-v$version-payload.zip"
$publish = Join-Path $root "target\setup-publish-v$version"
$setupProject = Join-Path $root 'setup-app\GlideDock.Setup.csproj'

$localDotnet = Join-Path $root '.dotnet\dotnet.exe'
if (Test-Path $localDotnet) {
    $dotnet = $localDotnet
    $env:DOTNET_ROOT = Join-Path $root '.dotnet'
    $env:PATH = "$env:DOTNET_ROOT;$env:PATH"
    $env:DOTNET_CLI_HOME = Join-Path $root '.dotnet-home'
    $env:NUGET_PACKAGES = Join-Path $root '.nuget'
    $env:APPDATA = Join-Path $root '.appdata'
    $env:LOCALAPPDATA = Join-Path $root '.localappdata'
    $env:TEMP = Join-Path $root 'target\temp'
    $env:TMP = $env:TEMP
    New-Item -ItemType Directory -Force $env:DOTNET_CLI_HOME, $env:NUGET_PACKAGES, $env:APPDATA, $env:LOCALAPPDATA, $env:TEMP | Out-Null
}
elseif (Get-Command dotnet -ErrorAction SilentlyContinue) {
    $dotnet = (Get-Command dotnet).Source
}
else {
    throw '.NET 8 SDK was not found. Install it or place a portable SDK at .dotnet\dotnet.exe.'
}

$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = '1'
$env:DOTNET_NOLOGO = '1'

& (Join-Path $PSScriptRoot 'package.ps1')
if ($LASTEXITCODE -ne 0) { throw "Package build failed with exit code $LASTEXITCODE" }
if (-not (Test-Path (Join-Path $stage 'glide-dock.exe'))) {
    throw "Package stage is missing glide-dock.exe: $stage"
}

New-Item -ItemType Directory -Force $output | Out-Null
Remove-Item -LiteralPath $payload -Force -ErrorAction SilentlyContinue
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $stage,
    $payload,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $false)

Write-Host 'Publishing branded WinUI 3 setup application...'
& $dotnet publish $setupProject `
    -c Release `
    -r win-x64 `
    --self-contained true `
    -p:Platform=x64 `
    -p:WindowsAppSDKSelfContained=true `
    -p:PublishSingleFile=true `
    -p:IncludeNativeLibrariesForSelfExtract=true `
    -p:IncludeAllContentForSelfExtract=true `
    -p:EnableCompressionInSingleFile=true `
    -p:Version=$version `
    -p:InformationalVersion=$version `
    "-p:PayloadArchive=$payload" `
    -o $publish
if ($LASTEXITCODE -ne 0) { throw "Setup publish failed with exit code $LASTEXITCODE" }

$builtSetup = Join-Path $publish 'GlideDock.Setup.exe'
if (-not (Test-Path $builtSetup)) { throw "Published Setup.exe is missing: $builtSetup" }
$setup = Join-Path $output "Glide-Dock-v$version-Setup.exe"
Copy-Item -LiteralPath $builtSetup -Destination $setup -Force
$size = [math]::Round((Get-Item $setup).Length / 1MB, 1)
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash
"$hash  $(Split-Path $setup -Leaf)" | Set-Content -LiteralPath "$setup.sha256" -Encoding ascii
Write-Host "Installer: $setup ($size MB)"
Write-Host "SHA-256:   $hash"
