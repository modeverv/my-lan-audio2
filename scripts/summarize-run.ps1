param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
$startup = $null
$summary = $null
$end = $null
$errors = @()
# Stream lines; do not load minutes/hours of packet records into memory.
foreach ($line in [System.IO.File]::ReadLines((Resolve-Path -LiteralPath $Path).Path)) {
    if ($line -notmatch '"event"\s*:\s*"(startup|summary|capture_end|error)"') { continue }
    $record = $line | ConvertFrom-Json
    switch ($record.event) {
        'startup' { $startup = $record }
        'summary' { $summary = $record }
        'capture_end' { $end = $record }
        'error' { $errors += $record }
    }
}
if (-not $summary) { throw 'No final summary; run failed, was interrupted before capture, or file is incomplete.' }
$summary.PSObject.Properties.Remove('callback_histogram_us')
$summary.PSObject.Properties.Remove('frames_per_callback_histogram')
[pscustomobject]@{ startup = $startup; capture_end = $end; summary = $summary; errors = $errors } |
    ConvertTo-Json -Depth 12
