# Desk Host setup, all in one: installs the built host, writes its settings (it serves only the
# fleet admin), registers the at-logon task in this user's session, the console task and the
# inbound firewall rule (the listed viewer addresses only), then starts it. Asks for elevation itself (one UAC prompt).
#   powershell -ep bypass -f setup.ps1 [-Exe <built exe>] [-Peers 10.0.0.222] [-Port 8790]
#   -SettingsOnly: rewrites deskhost.conf (and the firewall rule's peers) and restarts the host
# Undo: Unregister-ScheduledTask -TaskName "Makepad Desk Host"; Unregister-ScheduledTask -TaskName "Makepad Desk Host console"
#       Remove-NetFirewallRule -DisplayName "Makepad Desk Host"
#
# The console task: desktop duplication and the GPU's encoder need the session on the console.
# A session shown over Remote Desktop (or disconnected from it) has no GPU output, so when a
# viewer wants to watch it, the host runs "Makepad Desk Host console" (as SYSTEM: tscon of the
# host's session to the console; this user may run it, nobody else). The Remote Desktop client
# is disconnected; the desktop and its apps stay as they are.
param(
    [string]$Exe = "$env:USERPROFILE\rc\windesk\target\release\makepad-app-deskhost.exe",
    [string[]]$Peers = @("10.0.0.222"),
    [int]$Port = 8790,
    [int]$Fps = 120,
    [string]$ForUser = [Security.Principal.WindowsIdentity]::GetCurrent().Name,
    [string]$ForProfile = $env:USERPROFILE,
    [switch]$SettingsOnly
)
$ErrorActionPreference = "Stop"
$Peers = @($Peers | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    # the elevated copy acts for this user and this profile
    $a = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Exe `"$Exe`" -Port $Port -Fps $Fps -ForUser `"$ForUser`" -ForProfile `"$ForProfile`" -Peers $($Peers -join ',')"
    if ($SettingsOnly) { $a += " -SettingsOnly" }
    Start-Process powershell -Verb RunAs -Wait -ArgumentList $a
    exit
}
$task = "Makepad Desk Host"
$dir = Join-Path $ForProfile "AppData\Local\Makepad\DeskHost"
$installed = Join-Path $dir "makepad-app-deskhost.exe"
Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue | Stop-ScheduledTask -ErrorAction SilentlyContinue
# the at-logon task's process outlives Stop-ScheduledTask when it was started by hand
$running = @(Get-Process -Name makepad-app-deskhost -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $installed })
$running | Stop-Process -Force -ErrorAction SilentlyContinue
$running | ForEach-Object { $_.WaitForExit(10000) | Out-Null }

# 1. the installed copy (rebuilds in the build folder do not touch the running host)
if (-not $SettingsOnly) {
    if (-not (Test-Path $Exe)) { throw "no Desk Host build at $Exe" }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -Force $Exe $installed
}

# 2. settings: this machine's LAN address
$home_ = Join-Path $ForProfile ".makepad"
New-Item -ItemType Directory -Force -Path $home_ | Out-Null
$ip = (Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -like "10.*" -or $_.IPAddress -like "192.168.*" -or $_.IPAddress -like "172.*" } | Where-Object { $_.PrefixOrigin -ne "WellKnown" } | Select-Object -First 1).IPAddress
if (-not $ip) { throw "no private LAN address on this machine" }
$fleet = Join-Path $home_ "fleet"
if (-not ((Test-Path (Join-Path $fleet "id.key")) -and (Test-Path (Join-Path $fleet "id.cred")) -and (Test-Path (Join-Path $fleet "root.pub")))) {
    Write-Warning "no fleet identity (id.key, id.cred, root.pub) in $fleet yet: the host waits for it (tools/fleet-identity.md)"
}
@(
    "# Desk Host (written by setup.ps1)",
    "enabled = on",
    "host = $ip",
    "port = $Port",
    "fps = $Fps"
) | Set-Content -Encoding ascii (Join-Path $home_ "deskhost.conf")

if (-not $SettingsOnly) {
    # 3. at logon, in this user's interactive session (desktop duplication needs it)
    $action = New-ScheduledTaskAction -Execute $installed -WorkingDirectory $dir
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $ForUser
    $principal = New-ScheduledTaskPrincipal -UserId $ForUser -LogonType Interactive -RunLevel Limited
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
}

# 4. Repair the console task on every setup/redeploy, including -SettingsOnly.
& "$PSScriptRoot\register-console-task.ps1" -ForUser $ForUser

# 5. inbound TCP from the viewers only (any network profile: the rule names the peers; an OS
#    firewall in front of the host's own admin-only check)
Remove-NetFirewallRule -DisplayName "Makepad Desk Host" -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName "Makepad Desk Host" -Direction Inbound -Action Allow -Protocol TCP `
    -LocalPort $Port -RemoteAddress $Peers -Program $installed -Profile Any | Out-Null

Start-ScheduledTask -TaskName $task
Write-Host "Desk Host serves $ip`:$Port to $($Peers -join ', ') (log: $home_\deskhost.log, status: $home_\deskhost.status)"
