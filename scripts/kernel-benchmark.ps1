param(
    [Parameter(Mandatory)][string]$Device,
    [Parameter(Mandatory)][string]$Filter,
    [Parameter(Mandatory)][uint32]$Pin,
    [ValidateRange(3,600)][int]$Seconds=20,
    [ValidateRange(1,4096)][int[]]$Frames=@(32,48,128)
)
$ErrorActionPreference='Stop'
$repoPath=Split-Path -Parent $PSScriptRoot
$exePath=Join-Path $repoPath 'target\release\sender-windows.exe'
$runPath=Join-Path $repoPath ('runs\ks-'+(Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
New-Item -ItemType Directory $runPath | Out-Null
$sourceArgs=@('--tone-only','--pulse-probe','--render-raw','--seconds',($Seconds*$Frames.Count+12),
    '--device',$Device,'--output',(Join-Path $runPath 'source.jsonl')) | ForEach-Object {'"'+[string]$_+'"'}
$source=Start-Process -FilePath $exePath -ArgumentList $sourceArgs -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $runPath 'source.log')
try {
    $readyDeadline=(Get-Date).AddSeconds(10)
    do {
        if ($source.HasExited) {throw 'Source failed; inspect source.log'}
        $ready=(Get-Content (Join-Path $runPath 'source.log') -Raw -ErrorAction SilentlyContinue) -match 'source_ready'
        if (-not $ready) {Start-Sleep -Milliseconds 100}
    } while (-not $ready -and (Get-Date) -lt $readyDeadline)
    if (-not $ready) {throw 'Source readiness timeout'}
    $statuses=@()
    foreach ($frameCount in $Frames) {
        $captureArgs=@('--backend','ks','--ks-filter',$Filter,'--ks-pin',$Pin,'--ks-frames',$frameCount,
            '--ks-position','--measure-level','--pulse-probe','--seconds',$Seconds,
            '--output',(Join-Path $runPath "ks-$frameCount.jsonl"))
        & $exePath @captureArgs 1> (Join-Path $runPath "ks-$frameCount.summary.json") 2> (Join-Path $runPath "ks-$frameCount.console.log")
        $statuses+=[pscustomobject]@{frames=$frameCount;exit=$LASTEXITCODE}
        Write-Output "KS requested $frameCount frames: exit=$LASTEXITCODE"
        if ($source.HasExited) {throw 'Source ended before capture completion'}
    }
    $statuses | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $runPath 'statuses.json')
    $source.WaitForExit()
    if ($source.ExitCode -ne 0) {throw 'Source failed during benchmark'}
    & (Join-Path $PSScriptRoot 'analyze-pulses.ps1') -RunPath $runPath | Set-Content -Encoding utf8 (Join-Path $runPath 'pulse-analysis.json')
    Write-Output "RESULT_DIR=$runPath"
} finally {
    if (-not $source.HasExited) {$source.Kill();$source.WaitForExit()}
}
