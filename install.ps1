param([switch]$StartWithWindows, [switch]$Legacy)
$ErrorActionPreference = 'Stop'
if ($Legacy) { & (Join-Path $PSScriptRoot 'build.ps1') }
else { & (Join-Path $PSScriptRoot 'build-rust.ps1') }
$target = Join-Path $env:LOCALAPPDATA 'CodexQuota'
New-Item -ItemType Directory -Path $target -Force | Out-Null
$exe = Join-Path $target 'CodexQuota.exe'
if (-not $Legacy) {
    $check = Start-Process -FilePath (Join-Path $PSScriptRoot 'dist\CodexQuota.exe') -ArgumentList '--smoke' -Wait -PassThru -WindowStyle Hidden
    if ($check.ExitCode -ne 0) { throw 'El panel Rust no pasó la comprobación de integración. No se reemplazó la instalación.' }
}
Get-Process CodexQuota -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | ForEach-Object {
    Stop-Process -Id $_.Id
    $_.WaitForExit()
}
if (Test-Path -LiteralPath $exe) { Copy-Item -LiteralPath $exe -Destination (Join-Path $target 'CodexQuota.previous.exe') -Force }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'dist\CodexQuota.exe') -Destination $exe -Force
$notice = Join-Path $target 'licenses'
New-Item -ItemType Directory -Path $notice -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'assets\NOTICE.md'),(Join-Path $PSScriptRoot 'LICENSE') -Destination $notice -Force
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Programs')) 'Codex Quota.lnk'))
$link.TargetPath = $exe
$link.Description = '5-hour and weekly Codex quota'
$link.WorkingDirectory = $target
$link.Save()
if ($StartWithWindows) {
    New-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'CodexQuota' -Value ('"' + $exe + '"') -PropertyType String -Force | Out-Null
}
Start-Process -FilePath $exe -WindowStyle Hidden
Write-Host "Installed and launched: $exe"
