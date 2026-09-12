param(
    [ValidateRange(1, 86400)][int]$Seconds = 180,
    [string]$Device,
    [switch]$TestTone
)
$ErrorActionPreference = 'Stop'
$repoPath = Split-Path -Parent $PSScriptRoot
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
if ($cargoCommand) { $cargoPath = $cargoCommand.Source }
else { $cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
Push-Location $repoPath
try {
    & $cargoPath build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    $runPath = Join-Path $repoPath ('runs\' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
    New-Item -ItemType Directory -Path $runPath | Out-Null
    $exePath = Join-Path $repoPath 'target\release\sender-windows.exe'
    # Resolve default once so both measurements use the same endpoint.
    if (-not $Device) {
        $devices = & $exePath --list-devices | ForEach-Object { $_ | ConvertFrom-Json }
        if ($LASTEXITCODE -ne 0) { throw 'Device enumeration failed' }
        $Device = ($devices | Where-Object default_multimedia | Select-Object -First 1).id
        if (-not $Device) { throw 'No default render endpoint found' }
    }
    foreach ($backend in @('legacy', 'auto')) {
        $captureArgs = @('--backend', $backend, '--seconds', "$Seconds", '--device', $Device,
            '--output', (Join-Path $runPath "$backend.jsonl"))
        if ($TestTone) { $captureArgs += '--test-tone' }
        & $exePath @captureArgs
        if ($LASTEXITCODE -ne 0) { throw "Capture failed for $backend; inspect $runPath" }
    }
    Write-Host "Timing logs: $runPath"
} finally { Pop-Location }
