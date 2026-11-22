<#
.SYNOPSIS
    Switches the cabinet back to starting Pinup Popper at logon.

.DESCRIPTION
    Disables the "Run Rusty Popper" scheduled task (it is kept, not deleted, so
    Enable-RustyPopperStartup.ps1 can turn it back on) and re-enables Pinup
    Popper's own "Run Pinup Popper" task.

    Changing Popper's task needs administrator rights, so the script asks to
    elevate if it isn't already running as admin.

.EXAMPLE
    .\Disable-RustyPopperStartup.ps1
.EXAMPLE
    .\Disable-RustyPopperStartup.ps1 -WhatIf
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$TaskName = 'Run Rusty Popper',
    [string]$PopperTaskName = 'Run Pinup Popper'
)
$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $WhatIfPreference) {
    Write-Host "Asking for administrator rights (needed to change Popper's startup task)..."
    $relaunch = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"",
        '-TaskName', "`"$TaskName`"", '-PopperTaskName', "`"$PopperTaskName`"")
    Start-Process powershell.exe -Verb RunAs -ArgumentList $relaunch -Wait
    exit
}

# 1. Rusty Popper's task: disable (keep it for later).
$ours = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $ours) {
    Write-Host "No '$TaskName' task found."
} elseif ($ours.State -eq 'Disabled') {
    Write-Host "'$TaskName' is already disabled."
} elseif ($PSCmdlet.ShouldProcess($TaskName, 'Disable')) {
    Disable-ScheduledTask -TaskName $TaskName | Out-Null
    Write-Host "Disabled '$TaskName'."
}

# 2. Pinup Popper's task: enable it again.
$popper = Get-ScheduledTask -TaskName $PopperTaskName -ErrorAction SilentlyContinue
if (-not $popper) {
    Write-Warning "No '$PopperTaskName' task found. Re-create it in PinUp Popper's setup, or start PinUpMenu.exe another way."
} elseif ($popper.State -ne 'Disabled') {
    Write-Host "'$PopperTaskName' is already enabled."
} elseif ($PSCmdlet.ShouldProcess($PopperTaskName, 'Enable')) {
    Enable-ScheduledTask -TaskName $PopperTaskName | Out-Null
    Write-Host "Enabled '$PopperTaskName'."
}

Write-Host 'Done: Pinup Popper will start at the next logon.'
if ($isAdmin -and -not $WhatIfPreference) { Start-Sleep -Seconds 3 }  # let the elevated window be read
