# Administrator Windows PowerShell; changes only the observed Realtek flow-control property.
[CmdletBinding()]
param([string]$RestoreFrom)
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Administrator rights required.' }
$directory = Join-Path $PSScriptRoot '../runs/realtek-low-latency'
$null = New-Item -ItemType Directory -Path $directory -Force
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
$prefix = Join-Path $directory $stamp
Start-Transcript "$prefix-transcript.txt" | Out-Null
try {
    $nics = @(Get-NetAdapter | Where-Object InterfaceDescription -eq 'Realtek Gaming USB 2.5GbE Family Controller')
    if ($nics.Count -ne 1) { throw 'Expected exactly one Realtek USB 2.5GbE adapter.' }
    $nic = $nics[0]
    $properties = @(Get-NetAdapterAdvancedProperty -Name $nic.Name -AllProperties)
    $flow = @($properties | Where-Object RegistryKeyword -eq '*FlowControl')
    if ($flow.Count -ne 1 -or $flow[0].ValidRegistryValues -notcontains '0') { throw 'Expected flow-control setting is unavailable.' }
    $before = [string[]]$flow[0].RegistryValue
    $desired = @('0')
    if ($RestoreFrom) {
        $saved = Import-Clixml -LiteralPath $RestoreFrom
        if ([string]$saved.InterfaceGuid -ne [string]$nic.InterfaceGuid -or $saved.RegistryKeyword -ne '*FlowControl') { throw 'Backup belongs to a different adapter or property.' }
        $desired = [string[]]$saved.RegistryValue
        foreach ($value in $desired) {
            if ($flow[0].ValidRegistryValues -notcontains $value) { throw 'Backup value is not supported by the current driver.' }
        }
    }
    $properties | Export-Clixml "$prefix-properties-before.xml"
    Get-NetAdapterStatistics -Name $nic.Name | Export-Clixml "$prefix-statistics-before.xml"
    Get-NetQosPolicy -PolicyStore ActiveStore | Export-Clixml "$prefix-qos.xml"
    Get-NetQosPolicy -PolicyStore ActiveStore | Where-Object Name -eq 'LAN Audio Sender UDP low latency' | Format-List Name,AppPathName,IPProtocol,IPDstPortStart,IPDstPortEnd,DSCPValue
    [pscustomobject]@{InterfaceGuid=[string]$nic.InterfaceGuid; RegistryKeyword='*FlowControl'; RegistryValue=$before} | Export-Clixml "$prefix-restore.xml"
    if (($before -join ',') -ne ($desired -join ',')) {
        try {
            Set-NetAdapterAdvancedProperty -Name $nic.Name -RegistryKeyword '*FlowControl' -RegistryValue $desired -NoRestart
            Restart-NetAdapter -Name $nic.Name -Confirm:$false
            $deadline = (Get-Date).AddSeconds(35)
            do {
                Start-Sleep -Seconds 1
                $link = Get-NetAdapter -Name $nic.Name
            } until ($link.Status -eq 'Up' -or (Get-Date) -gt $deadline)
            if ($link.Status -ne 'Up') { throw 'Ethernet link did not recover within 35 seconds.' }
            $readback = Get-NetAdapterAdvancedProperty -Name $nic.Name -RegistryKeyword '*FlowControl'
            if (($readback.RegistryValue -join ',') -ne ($desired -join ',')) { throw 'Flow-control readback does not match.' }
        } catch {
            $failure = $_
            Set-NetAdapterAdvancedProperty -Name $nic.Name -RegistryKeyword '*FlowControl' -RegistryValue $before -NoRestart
            Restart-NetAdapter -Name $nic.Name -Confirm:$false
            throw $failure
        }
    }
    Get-NetAdapterAdvancedProperty -Name $nic.Name | Export-Clixml "$prefix-properties-after.xml"
    Get-NetAdapter -Name $nic.Name | Format-List Name,Status,LinkSpeed
    Get-NetAdapterAdvancedProperty -Name $nic.Name -RegistryKeyword '*FlowControl' | Format-List DisplayName,DisplayValue,RegistryValue
    Get-NetAdapterStatistics -Name $nic.Name | Export-Clixml "$prefix-statistics-after.xml"
    "SUCCESS`nBackup: $prefix-restore.xml" | Set-Content (Join-Path $directory 'status.txt')
} catch {
    $_ | Out-String | Set-Content (Join-Path $directory 'status.txt')
    throw
} finally { Stop-Transcript | Out-Null }
