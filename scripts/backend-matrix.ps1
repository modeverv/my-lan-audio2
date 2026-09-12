param(
    [Parameter(Mandatory)][string]$Device,
    [ValidateRange(3,600)][int]$Seconds=20,
    [switch]$MinimumRender,
    [switch]$RawRender,
    [string]$Name='matrix'
)
$ErrorActionPreference='Stop'
$repoPath=Split-Path -Parent $PSScriptRoot
$exePath=Join-Path $repoPath 'target\release\sender-windows.exe'
$runPath=Join-Path $repoPath ('runs\'+$Name+'-'+(Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
New-Item -ItemType Directory $runPath | Out-Null
$cases=@(
    @{name='endpoint-default';backend='legacy';tap='default';mute='off'},
    @{name='endpoint-pre';backend='legacy';tap='pre';mute='off'},
    @{name='endpoint-pre-muted';backend='legacy';tap='pre';mute='on'},
    @{name='process-include-muted';backend='process-include';tap='default';mute='on'},
    @{name='process-exclude-self-muted';backend='process-exclude';tap='default';mute='on'},
    @{name='endpoint-post-muted';backend='legacy';tap='post';mute='on'}
)
$sourceArgs=@('--tone-only','--pulse-probe','--seconds',($Seconds*$cases.Count+8),'--device',$Device,
    '--output',(Join-Path $runPath 'source.jsonl'))
if ($MinimumRender) {$sourceArgs+=@('--render-period-frames','0')}
if ($RawRender) {$sourceArgs+='--render-raw'}
# Start-Process ArgumentList needs quoting for absolute output paths containing spaces.
$sourceArgs=$sourceArgs | ForEach-Object {'"'+[string]$_+'"'}
$source=Start-Process -FilePath $exePath -ArgumentList $sourceArgs -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $runPath 'source.log')
try {
    $readyDeadline=(Get-Date).AddSeconds(10)
    do {
        if ($source.HasExited) {throw "Source failed: $(Get-Content (Join-Path $runPath 'source.log') -Raw)"}
        $ready=(Get-Content (Join-Path $runPath 'source.log') -Raw -ErrorAction SilentlyContinue) -match 'source_ready'
        if (-not $ready) {Start-Sleep -Milliseconds 100}
    } while (-not $ready -and (Get-Date) -lt $readyDeadline)
    if (-not $ready) {throw 'Source readiness timeout'}
    $statuses=@()
    foreach ($case in $cases) {
        $captureArgs=@('--backend',$case.backend,'--tap',$case.tap,'--endpoint-mute',$case.mute,
            '--device',$Device,'--seconds',$Seconds,'--measure-level','--pulse-probe','--output',(Join-Path $runPath ($case.name+'.jsonl')))
        if ($case.backend -eq 'process-include') {$captureArgs+=@('--pid',$source.Id)}
        & $exePath @captureArgs 1> (Join-Path $runPath ($case.name+'.summary.json')) 2> (Join-Path $runPath ($case.name+'.console.log'))
        $statuses+=[pscustomobject]@{name=$case.name;exit=$LASTEXITCODE}
        Write-Output "$($case.name): exit=$LASTEXITCODE"
        if ($source.HasExited) {throw 'Source ended before matrix completion'}
    }
    $statuses | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $runPath 'statuses.json')
    # Finite source duration permits graceful JSONL flushing without killing it.
    $source.WaitForExit()
    if ($source.ExitCode -ne 0) {throw 'Source failed during matrix'}
    & (Join-Path $PSScriptRoot 'analyze-pulses.ps1') -RunPath $runPath | Set-Content -Encoding utf8 (Join-Path $runPath 'pulse-analysis.json')
    Write-Output "RESULT_DIR=$runPath"
} finally {
    if (-not $source.HasExited) {$source.Kill();$source.WaitForExit()}
}
