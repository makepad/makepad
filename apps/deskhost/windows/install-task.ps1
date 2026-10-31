# Desk Host at logon: runs makepad-app-deskhost in the logged-in user's interactive session
# (desktop duplication needs it; a service in session 0 sees no desktop). Run as that user:
#   powershell -ExecutionPolicy Bypass -File install-task.ps1 -Exe C:\path\to\makepad-app-deskhost.exe
# Run elevated: the companion console task runs as SYSTEM. Registration never runs it.
# Remove with: Unregister-ScheduledTask -TaskName "Makepad Desk Host" -Confirm:$false
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $Exe)) { throw "no such file: $Exe" }
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
# A redeploy must repair both tasks, even when the console task was removed.
& "$PSScriptRoot\register-console-task.ps1" -ForUser $user
$action = New-ScheduledTaskAction -Execute $Exe -WorkingDirectory (Split-Path $Exe)
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -Priority 4
Register-ScheduledTask -TaskName "Makepad Desk Host" -Action $action -Trigger $trigger `
    -Principal $principal -Settings $settings -Force | Out-Null
Write-Host "registered 'Makepad Desk Host' for $user (starts at the next logon; Start-ScheduledTask to start now)"
Write-Host "it serves only with %USERPROFILE%\.makepad\deskhost.conf: enabled = on, and the fleet identity in %USERPROFILE%\.makepad\fleet"
