<#
.SYNOPSIS
    Lets phones on your home network reach Rusty Popper's phone remote.

.DESCRIPTION
    Adds a Windows Firewall rule that allows incoming connections to
    rusty-popper.exe on the phone remote's port, from the local subnet only.
    Run it once after turning the phone remote on. Without it, Windows may ask
    whether to allow Rusty Popper the first time the remote starts, and that
    prompt can end up hidden behind the cabinet's screens.

    If that prompt was already answered with "Cancel", Windows added rules that
    block rusty-popper.exe; this script removes those. Changing the firewall
    needs administrator rights, so the script asks to elevate.

.PARAMETER Port
    The phone remote's port ([remote] port in rusty-popper.toml). Default 8642.

.PARAMETER ExePath
    Path to rusty-popper.exe. Defaults to the copy next to this script.

.PARAMETER Remove
    Remove the rule again.

.EXAMPLE
    .\Allow-RustyPopperRemote.ps1
.EXAMPLE
    .\Allow-RustyPopperRemote.ps1 -Remove
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [int]$Port = 8642,
    [string]$ExePath = (Join-Path $PSScriptRoot 'rusty-popper.exe'),
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'
$RuleName = 'Rusty Popper phone remote'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $WhatIfPreference) {
    Write-Host 'Asking for administrator rights (needed to change the firewall)...'
    $relaunch = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"",
        '-Port', $Port, '-ExePath', "`"$ExePath`"")
    if ($Remove) { $relaunch += '-Remove' }
    Start-Process powershell.exe -Verb RunAs -ArgumentList $relaunch -Wait
    exit
}

try {
$old = Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue
if ($old -and $PSCmdlet.ShouldProcess($RuleName, 'Remove the old rule')) {
    $old | Remove-NetFirewallRule
    if ($Remove) { Write-Host "Removed '$RuleName'." }
} elseif ($Remove -and -not $old) {
    Write-Host "No '$RuleName' rule found, nothing to remove."
}

if (-not $Remove) {
    if (-not (Test-Path $ExePath)) { throw "rusty-popper.exe not found at $ExePath (pass -ExePath)" }
    $ExePath = (Resolve-Path $ExePath).Path

    # Block rules from a cancelled firewall prompt win over any allow rule.
    $blocks = Get-NetFirewallApplicationFilter -Program $ExePath -ErrorAction SilentlyContinue |
        Get-NetFirewallRule | Where-Object { $_.Action -eq 'Block' -and $_.Direction -eq 'Inbound' }
    foreach ($b in $blocks) {
        if ($PSCmdlet.ShouldProcess($b.DisplayName, 'Remove blocking rule')) {
            Remove-NetFirewallRule -Name $b.Name
            Write-Host "Removed blocking rule '$($b.DisplayName)'."
        }
    }

    if ($PSCmdlet.ShouldProcess($RuleName, "Allow TCP $Port to $ExePath from the local subnet")) {
        New-NetFirewallRule -DisplayName $RuleName -Direction Inbound -Action Allow -Protocol TCP `
            -LocalPort $Port -Program $ExePath -RemoteAddress LocalSubnet -Profile Any | Out-Null
        Write-Host "Added '$RuleName': TCP $Port, local network only."
    }
}
} catch {
    Write-Host $_ -ForegroundColor Red
} finally {
    if ($isAdmin -and -not $WhatIfPreference) { Start-Sleep -Seconds 5 }  # let the elevated window be read
}
