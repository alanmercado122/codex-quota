$ErrorActionPreference = 'Stop'
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
$output = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Path $output -Force | Out-Null
& $compiler /nologo /target:winexe /optimize+ /platform:anycpu /codepage:65001 "/out:$output\CodexQuota.exe" /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.Web.Extensions.dll (Join-Path $PSScriptRoot 'QuotaTray.cs') (Join-Path $PSScriptRoot 'TaskbarPanel.cs')
if ($LASTEXITCODE -ne 0) { throw 'La compilación falló.' }
Write-Host "Compilado: $output\CodexQuota.exe"
