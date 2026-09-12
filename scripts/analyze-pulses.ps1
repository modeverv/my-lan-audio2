param([Parameter(Mandatory)][string]$RunPath)
$ErrorActionPreference='Stop'
$source=@(Get-Content (Join-Path $RunPath 'source.jsonl') | ForEach-Object {$_|ConvertFrom-Json} | Where-Object event -EQ source_pulse)
$result=@()
foreach ($file in Get-ChildItem -LiteralPath $RunPath -Filter '*.jsonl') {
    if ($file.Name -eq 'source.jsonl') {continue}
    $detections=@([System.IO.File]::ReadLines($file.FullName) | Where-Object {$_ -match '"event":"pulse"'} | ForEach-Object {$_|ConvertFrom-Json})
    $latencies=@()
    $unmatched=0
    $used=@{}
    foreach ($pulse in $detections) {
        $prior=$source | Where-Object {$_.time_after_release_100ns -le $pulse.capture_time_100ns} | Select-Object -Last 1
        if (-not $prior) {$unmatched++;continue}
        $delta=($pulse.capture_time_100ns-$prior.time_after_release_100ns)/10.0
        # Stimuli repeat at 500ms. Restrict matching to <250ms and report rejects;
        # this is a bounded pulse estimate, not identity-encoded arbitrary-delay correlation.
        if ($delta -ge 250000 -or $used.ContainsKey([string]$prior.source_frame)) {$unmatched++;continue}
        $used[[string]$prior.source_frame]=$true
        $latencies+=$delta
    }
    $sorted=@($latencies | Sort-Object)
    $stats=$null
    if ($sorted.Count) {
        $stats=[pscustomobject]@{count=$sorted.Count;min=$sorted[0];p50=$sorted[[math]::Ceiling($sorted.Count*.50)-1];p95=$sorted[[math]::Ceiling($sorted.Count*.95)-1];p99=$sorted[[math]::Ceiling($sorted.Count*.99)-1];max=$sorted[-1]}
    }
    $result+=[pscustomobject]@{run=$file.BaseName;detected_pulses=$detections.Count;unmatched_or_duplicate=$unmatched;pairing_ambiguous=($unmatched -gt 0 -or $detections.Count -eq 0);source_release_to_capture_us=$stats}
}
$result | ConvertTo-Json -Depth 8
