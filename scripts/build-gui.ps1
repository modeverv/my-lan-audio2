$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    New-Item -ItemType Directory -Force dist | Out-Null
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    & $compiler /nologo /target:winexe /platform:x64 /optimize+ /warnaserror+ /out:dist\LAN-Audio-Sender.exe /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.Management.dll /resource:target\release\sender-windows.exe,sender-windows.exe apps\sender-gui\Program.cs
    if ($LASTEXITCODE -ne 0) { throw 'GUI build failed' }
    Get-Item dist\LAN-Audio-Sender.exe | Select-Object FullName,Length
} finally { Pop-Location }
