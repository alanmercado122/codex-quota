param([switch]$Test)
$ErrorActionPreference = 'Stop'
$buildTools = Join-Path $env:LOCALAPPDATA 'CodexQuotaBuild'
$isolatedCargo = Join-Path $buildTools 'cargo\bin\cargo.exe'
if (Test-Path -LiteralPath $isolatedCargo) {
    $env:CARGO_HOME = Join-Path $buildTools 'cargo'
    $env:RUSTUP_HOME = Join-Path $buildTools 'rustup'
    $env:PATH = (Join-Path $buildTools 'cargo\bin') + ';' + $env:PATH
    $cargoExe = $isolatedCargo
} else {
    $cargoExe = (Get-Command cargo -ErrorAction Stop).Source
}
# Rust can locate the installed Visual Studio Build Tools without modifying PATH.
Push-Location $PSScriptRoot
try {
    if ($Test) { & $cargoExe test --release --locked; if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' } }
    & $cargoExe build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed.' }
    New-Item -ItemType Directory -Path (Join-Path $PSScriptRoot 'dist') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'target\release\CodexQuota.exe') -Destination (Join-Path $PSScriptRoot 'dist\CodexQuota.exe') -Force
} finally { Pop-Location }
