param(
    [Parameter(Mandatory)][int]$SourcePid,
    [Parameter(Mandatory)][string]$Destination,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Executable = '.\target\release\sender-windows.exe',
    [ValidateRange(90,86400)][int]$Seconds = 90,
    [ValidateRange(1,20)][int]$Repeats = 2,
    [ValidateRange(1,3)][int]$Workers = 1,
    [switch]$Events
)
$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path -LiteralPath $Executable).Path
$null = Get-Process -Id $SourcePid
$active = Get-CimInstance Win32_Process | Where-Object {
    $_.CommandLine -and $_.Name -ne 'powershell.exe' -and $_.Name -ne 'pwsh.exe' -and
    $_.CommandLine.Contains('--udp-to ' + $Destination)
}
if ($active) { throw 'Another sender uses this destination. Stop it before measuring to avoid mixed streams.' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new output directory; measurement logs are never overwritten.' }
$outPath = (New-Item -ItemType Directory -Path $OutputDirectory).FullName
$manifest = [ordered]@{
    commit = (git rev-parse HEAD); diff_stat = (git diff --stat | Out-String)
    executable = $exePath; sha256 = (Get-FileHash -LiteralPath $exePath).Hash
    source_pid = $SourcePid; destination = $Destination
    nic = @(Get-NetAdapter | Select-Object Name,InterfaceDescription,Status,LinkSpeed,DriverInformation)
    addresses = @(Get-NetIPAddress -AddressFamily IPv4 | Select-Object InterfaceAlias,IPAddress)
    runs = @()
}
for ($index = 1; $index -le $Repeats; $index++) {
    $prefix = Join-Path $outPath "run-$index"
    $argsList = @('--backend','process-include','--pid',"$SourcePid",'--udp-to',$Destination,
        '--udp-workers',"$Workers",'--udp-deadline-ms','5','--udp-frames','128',
        '--seconds',"$Seconds",'--measure-level','--output',('"' + $prefix + '.jsonl"'))
    if ($Events) { $argsList += '--events' }
    $started = Get-Date -Format o
    $child = Start-Process -FilePath $exePath -ArgumentList $argsList -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput "$prefix.stdout" -RedirectStandardError "$prefix.stderr"
    try {
        $child.WaitForExit()
        $row = [ordered]@{ started=$started; ended=(Get-Date -Format o); arguments=$argsList
            exit_code=$child.ExitCode; process_cpu_ms=$child.TotalProcessorTime.TotalMilliseconds }
        $manifest.runs += $row
        $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $outPath 'manifest.json') -Encoding utf8
        if ($child.ExitCode -ne 0) { throw "Sender failed; see $prefix.stderr" }
        & node (Join-Path $PSScriptRoot 'analyze-sender-latency.mjs') "$prefix.jsonl" "$prefix.analysis.json" > "$prefix.analysis.stdout"
        if ($LASTEXITCODE -ne 0) { throw 'Metadata analysis failed' }
        Write-Output "Completed $index / $Repeats : $prefix.analysis.json"
    } finally { $child.Dispose() }
}
