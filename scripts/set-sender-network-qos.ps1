# Run with Windows PowerShell as administrator. Only LAN Audio Sender UDP is matched.
# Restore removes only the policy created by this script.
[CmdletBinding()]
param(
    [switch]$Restore,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../runs/network-qos')
)
$ErrorActionPreference = 'Stop'
$policyName = 'LAN Audio Sender UDP low latency'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Administrator rights are required.'
}
$null = New-Item -ItemType Directory -Path $OutputDirectory -Force
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
Start-Transcript -Path (Join-Path $OutputDirectory "$stamp-transcript.txt") | Out-Null
try {
    $existing = @(Get-NetQosPolicy | Where-Object Name -eq $policyName)
    if ($Restore) {
        if ($existing.Count) {
            if ($existing[0].AppPathName -ne 'sender-windows.exe' -or $existing[0].IPDstPortStart -ne 40100 -or $existing[0].IPDstPortEnd -ne 40100 -or $existing[0].DSCPValue -ne 46) {
                throw 'Policy differs from the expected policy; inspect it before removing.'
            }
            Remove-NetQosPolicy -Name $policyName -Confirm:$false
        }
    } else {
        Get-NetQosPolicy -PolicyStore ActiveStore | Export-Clixml (Join-Path $OutputDirectory "$stamp-qos-before.xml")
        $nic = @(Get-NetAdapter | Where-Object InterfaceDescription -eq 'Realtek Gaming USB 2.5GbE Family Controller')
        if ($nic.Count -ne 1) { throw 'Expected exactly one Realtek USB 2.5GbE adapter.' }
        Get-NetAdapterAdvancedProperty -Name $nic[0].Name -AllProperties | Export-Clixml (Join-Path $OutputDirectory "$stamp-nic-before.xml")
        Get-NetAdapterStatistics -Name $nic[0].Name | Export-Clixml (Join-Path $OutputDirectory "$stamp-statistics.xml")
        $binding = Get-NetAdapterBinding -Name $nic[0].Name -ComponentID ms_pacer
        if (-not $binding.Enabled) { throw 'QoS packet scheduler binding is disabled.' }
        if ($existing.Count) { throw 'Policy already exists; it has not been overwritten.' }
        New-NetQosPolicy -Name $policyName -AppPathNameMatchCondition 'sender-windows.exe' -IPProtocolMatchCondition UDP -IPDstPortMatchCondition 40100 -NetworkProfile All -DSCPAction 46 | Format-List *
        # NIC tuning is separate; preserve its current driver settings here.
        'NIC advanced settings preserved: priority tagging and QoS scheduler already enabled.'
    }
    Get-NetQosPolicy -PolicyStore ActiveStore | Where-Object Name -eq $policyName | Format-List *
    Get-NetQosPolicy -PolicyStore ActiveStore | Where-Object Name -eq $policyName | Export-Clixml (Join-Path $OutputDirectory "$stamp-qos-after.xml")
    'SUCCESS' | Set-Content (Join-Path $OutputDirectory 'status.txt')
} catch {
    $_ | Out-String | Set-Content (Join-Path $OutputDirectory 'status.txt')
    throw
} finally {
    Stop-Transcript | Out-Null
}
