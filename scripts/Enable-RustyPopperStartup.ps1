<#
.SYNOPSIS
    Makes the cabinet start Rusty Popper at logon instead of Pinup Popper.

.DESCRIPTION
    Creates (or re-enables) the "Run Rusty Popper" scheduled task, which starts
    rusty-popper.exe when this user logs on, and disables Pinup Popper's own
    "Run Pinup Popper" task so the two front ends don't both start.

    Changing Popper's task needs administrator rights, so the script asks to
    elevate if it isn't already running as admin. Nothing is deleted: run
    Disable-RustyPopperStartup.ps1 to switch back.

.PARAMETER ExePath
    Path to rusty-popper.exe. Defaults to the copy next to this script.

.EXAMPLE
    .\Enable-RustyPopperStartup.ps1
.EXAMPLE
    .\Enable-RustyPopperStartup.ps1 -WhatIf
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$ExePath = (Join-Path $PSScriptRoot 'rusty-popper.exe'),
    [string]$TaskName = 'Run Rusty Popper',
    [string]$PopperTaskName = 'Run Pinup Popper',
    # The account the logon task is for. Defaults to whoever runs the script;
    # passed through when the script re-launches itself elevated.
    [string]$ForUser = "$env:USERDOMAIN\$env:USERNAME"
)
$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $WhatIfPreference) {
    Write-Host "Asking for administrator rights (needed to change Popper's startup task)..."
    $relaunch = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"",
        '-ExePath', "`"$ExePath`"", '-TaskName', "`"$TaskName`"",
        '-PopperTaskName', "`"$PopperTaskName`"", '-ForUser', "`"$ForUser`"")
    Start-Process powershell.exe -Verb RunAs -ArgumentList $relaunch -Wait
    exit
}

if (-not (Test-Path $ExePath)) { throw "rusty-popper.exe not found at $ExePath (pass -ExePath)" }
$ExePath = (Resolve-Path $ExePath).Path

# 1. Rusty Popper's logon task: create it, or point it at this exe and enable it.
if ($PSCmdlet.ShouldProcess($TaskName, "Start $ExePath at logon for $ForUser")) {
    $action = New-ScheduledTaskAction -Execute $ExePath -WorkingDirectory (Split-Path $ExePath)
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Set-ScheduledTask -TaskName $TaskName -Action $action | Out-Null
        Enable-ScheduledTask -TaskName $TaskName | Out-Null
    } else {
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $ForUser
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
        $principal = New-ScheduledTaskPrincipal -UserId $ForUser -LogonType Interactive -RunLevel Limited
        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal `
            -Description 'Starts Rusty Popper (alternate Pinup Popper front end) at logon.' | Out-Null
    }
    Write-Host "Enabled '$TaskName' -> $ExePath"
}

# 2. Pinup Popper's task: disable it so it doesn't start as well.
$popper = Get-ScheduledTask -TaskName $PopperTaskName -ErrorAction SilentlyContinue
if (-not $popper) {
    Write-Host "No '$PopperTaskName' task found, nothing to disable (check Popper isn't in the Startup folder)."
} elseif ($popper.State -eq 'Disabled') {
    Write-Host "'$PopperTaskName' is already disabled."
} elseif ($PSCmdlet.ShouldProcess($PopperTaskName, 'Disable')) {
    Disable-ScheduledTask -TaskName $PopperTaskName | Out-Null
    Write-Host "Disabled '$PopperTaskName'."
}

Write-Host 'Done: Rusty Popper will start at the next logon.'
if ($isAdmin -and -not $WhatIfPreference) { Start-Sleep -Seconds 3 }  # let the elevated window be read
