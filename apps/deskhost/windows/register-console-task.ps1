# Shared by setup.ps1 and install-task.ps1 (redeploy). Idempotent registration only:
# this does not start either task or change a logged-in user's session. Run elevated.
# The task finds the Desk Host to move by the exe of the "Makepad Desk Host" logon task, read when it
# runs, so a redeploy to another install folder needs no re-registration.
param(
    [string]$ForUser = [Security.Principal.WindowsIdentity]::GetCurrent().Name
)
$ErrorActionPreference = "Stop"
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) { throw "Registering the console task requires an elevated shell: run setup.ps1" }
$sid = (New-Object Security.Principal.NTAccount($ForUser)).Translate([Security.Principal.SecurityIdentifier]).Value
$svc = New-Object -ComObject Schedule.Service
$svc.Connect()
$def = $svc.NewTask(0)
$def.RegistrationInfo.Description = "Moves the Desk Host user's session to the console so a Desk Link viewer can watch it (desktop duplication needs the console)."
$def.Settings.AllowDemandStart = $true
$def.Settings.Hidden = $true
$def.Settings.DisallowStartIfOnBatteries = $false
$def.Settings.StopIfGoingOnBatteries = $false
$def.Settings.ExecutionTimeLimit = "PT1M"
$def.Settings.MultipleInstances = 2 # ignore a second request while one runs
$act = $def.Actions.Create(0)
$act.Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$act.Arguments = "-NoProfile -NonInteractive -WindowStyle Hidden -Command `"`$exe = (Get-ScheduledTask -TaskName 'Makepad Desk Host' -ErrorAction Stop).Actions[0].Execute; `$p = Get-Process -Name makepad-app-deskhost -ErrorAction Stop | Where-Object { `$_.Path -eq `$exe } | Select-Object -First 1; if (-not `$p) { exit 1 }; & `$env:SystemRoot\System32\tscon.exe `$p.SessionId /dest:console; exit `$LASTEXITCODE`""
$sddl = "D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;$sid)"
# TASK_CREATE_OR_UPDATE, SYSTEM, TASK_LOGON_SERVICE_ACCOUNT. No trigger: demand only.
$svc.GetFolder("\").RegisterTaskDefinition("Makepad Desk Host console", $def, 6, "SYSTEM", $null, 5, $sddl) | Out-Null
Write-Host "registered 'Makepad Desk Host console' for $ForUser (hidden, demand only)"
