param([string]$OutputDirectory, [switch]$SkipRestore)
# Builds the standalone Rust + Tauri Glide-Dock installer.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Cannot read the workspace version from Cargo.toml' }
$version = $versionLine.Matches[0].Groups[1].Value
$local = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($local)) { throw 'LOCALAPPDATA is unavailable' }
$output = if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { Join-Path $local 'Glide-Dock\installer' } else { [IO.Path]::GetFullPath($OutputDirectory) }
$payload = Join-Path $output "Glide-Dock-v$version-payload.zip"
$providedPayload = $env:GLIDE_DOCK_PAYLOAD_ZIP
$buildPayload = [string]::IsNullOrWhiteSpace($providedPayload)
$payloadSource = if ($buildPayload) { Join-Path $local "Glide-Dock\Glide-Dock-v$version-payload.zip" } else { [IO.Path]::GetFullPath($providedPayload) }
$setup = Join-Path $output "Glide-Dock-v$version-Setup.exe"
$hashFile = "$setup.sha256"
function Set-InstallerIntegrity([string]$Path) {
    if (-not ('GlideDockDistributionSecurity' -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
public static class GlideDockDistributionSecurity {
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
    static extern uint GetNamedSecurityInfoW(string path, int type, uint info,
        IntPtr owner, IntPtr group, IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetAce(IntPtr acl, uint index, out IntPtr ace);
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr memory);
    public static string LabelSid(string path) {
        IntPtr sacl, descriptor;
        uint error = GetNamedSecurityInfoW(path, 1, 0x10,
            IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, out sacl, out descriptor);
        if (error != 0) throw new Win32Exception((int)error);
        try {
            if (sacl == IntPtr.Zero) return null;
            int count = (ushort)Marshal.ReadInt16(sacl, 4);
            for (uint index = 0; index < count; index++) {
                IntPtr ace;
                if (!GetAce(sacl, index, out ace)) throw new Win32Exception(Marshal.GetLastWin32Error());
                if (Marshal.ReadByte(ace) == 0x11)
                    return new SecurityIdentifier(IntPtr.Add(ace, 8)).Value;
            }
            return null;
        } finally { LocalFree(descriptor); }
    }
}
"@
    }
    # Normalize only the generated executable, preserving its original access rules.
    $originalAcl = Get-Acl -LiteralPath $Path
    $temporaryAcl = Get-Acl -LiteralPath $Path
    $userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $rule = New-Object Security.AccessControl.FileSystemAccessRule($userSid, [Security.AccessControl.FileSystemRights]::TakeOwnership, [Security.AccessControl.AccessControlType]::Allow)
    $temporaryAcl.AddAccessRule($rule)
    try {
        Set-Acl -LiteralPath $Path -AclObject $temporaryAcl
        $result = & icacls $Path /setintegritylevel Medium 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Cannot set Medium integrity on installer ${Path}: $result. Choose an output directory owned by the current user." }
    } finally {
        Set-Acl -LiteralPath $Path -AclObject $originalAcl
    }
    $labelSid = [GlideDockDistributionSecurity]::LabelSid($Path)
    if ($labelSid -ne 'S-1-16-8192') {
        throw "Installer integrity verification failed for ${Path}: $labelSid"
    }
}
function Remove-SafeFile([string]$Path,[string]$AllowedRoot) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $full = [IO.Path]::GetFullPath($Path)
    $base = [IO.Path]::GetFullPath($AllowedRoot).TrimEnd([IO.Path]::DirectorySeparatorChar,[IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($base,[StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to delete outside ${AllowedRoot}: $full" }
    Remove-Item -LiteralPath $full -Force
}
New-Item -ItemType Directory -Force -Path $output | Out-Null
& (Join-Path $PSScriptRoot 'build-installer-ui.ps1') -SkipRestore:$SkipRestore
if ($buildPayload) {
    & (Join-Path $PSScriptRoot 'package.ps1')
    if ($LASTEXITCODE -ne 0) { throw "Package build failed with exit code $LASTEXITCODE" }
}
if (-not (Test-Path -LiteralPath $payloadSource)) { throw "Package payload is missing: $payloadSource" }
$payloadSource = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $payloadSource).Path)
$payloadDestination = [IO.Path]::GetFullPath($payload)
$samePayloadPath = [string]::Equals($payloadSource, $payloadDestination, [StringComparison]::OrdinalIgnoreCase)
if (-not $samePayloadPath) { Copy-Item -LiteralPath $payloadSource -Destination $payloadDestination -Force }
if (-not (Test-Path -LiteralPath $payload)) { throw "Package payload copy is missing: $payload" }
$hadPayloadEnvironment = Test-Path Env:GLIDE_DOCK_PAYLOAD_ZIP
$previousPayloadEnvironment = $env:GLIDE_DOCK_PAYLOAD_ZIP
if (-not $hadPayloadEnvironment -or -not $samePayloadPath) {
    $env:GLIDE_DOCK_PAYLOAD_ZIP = $payloadDestination
}
$hadUninstallerEnvironment = Test-Path Env:GLIDE_DOCK_UNINSTALL_EXE
$previousUninstallerEnvironment = $env:GLIDE_DOCK_UNINSTALL_EXE
try {
    Write-Host "Building Glide-Dock uninstaller $version..."
    cargo build --locked --release -p glide-uninstall | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Uninstaller build failed with exit code $LASTEXITCODE" }
    $uninstaller = Join-Path $root 'target\release\glide-uninstall.exe'
    if (-not (Test-Path -LiteralPath $uninstaller)) { throw "Uninstaller output is missing: $uninstaller" }
    $env:GLIDE_DOCK_UNINSTALL_EXE = [IO.Path]::GetFullPath($uninstaller)
    Write-Host "Building Tauri installer $version..."
    cargo build --locked --release -p glide-setup | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Tauri installer build failed with exit code $LASTEXITCODE" }
} finally {
    if ($hadUninstallerEnvironment) { $env:GLIDE_DOCK_UNINSTALL_EXE = $previousUninstallerEnvironment }
    else { Remove-Item Env:GLIDE_DOCK_UNINSTALL_EXE -ErrorAction SilentlyContinue }
    if ($hadPayloadEnvironment) { $env:GLIDE_DOCK_PAYLOAD_ZIP = $previousPayloadEnvironment }
    else { Remove-Item Env:GLIDE_DOCK_PAYLOAD_ZIP -ErrorAction SilentlyContinue }
}
$builtSetup = Join-Path $root 'target\release\glide-setup.exe'
if (-not (Test-Path -LiteralPath $builtSetup)) { throw "Tauri installer output is missing: $builtSetup" }
Copy-Item -LiteralPath $builtSetup -Destination $setup -Force
Set-InstallerIntegrity $setup
$size = [math]::Round((Get-Item -LiteralPath $setup).Length / 1MB,1)
$hash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash
"$hash  $(Split-Path $setup -Leaf)" | Set-Content -LiteralPath $hashFile -Encoding ascii
if (-not $samePayloadPath) { Remove-SafeFile $payload $output }
Write-Host "Installer: $setup ($size MB)"
Write-Host "SHA-256:   $hash"
