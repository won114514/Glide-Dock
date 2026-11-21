param([switch]$SkipRestore)
# Builds the bundled Fluent UI frontend; no CDN or frontend server is used at runtime.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$ui = Join-Path $root 'installer\ui'
$node = Get-Command node -ErrorAction Stop
$npm = Get-Command npm.cmd,npm -ErrorAction SilentlyContinue | Select-Object -First 1
$npmCli = $env:GLIDE_DOCK_NPM_CLI
if (-not $npm -and [string]::IsNullOrWhiteSpace($npmCli)) {
    $npmCli = Join-Path $root 'target\tooling\npm\package\bin\npm-cli.js'
}
if (-not $npm -and -not (Test-Path -LiteralPath $npmCli)) {
    throw 'Node.js with npm is required to build the Fluent UI installer frontend. Install Node.js 22.12+ or set GLIDE_DOCK_NPM_CLI to npm-cli.js.'
}
function Invoke-UiNpm([string[]]$Arguments) {
    if ($npm) { & $npm.Source @Arguments }
    else { & $node.Source $npmCli @Arguments }
    if ($LASTEXITCODE -ne 0) { throw "Installer frontend command failed: npm $($Arguments -join ' ')" }
}
Push-Location $ui
try {
    if (-not $SkipRestore) { Invoke-UiNpm @('ci','--no-audit','--no-fund') }
    Invoke-UiNpm @('run','build')
} finally { Pop-Location }
