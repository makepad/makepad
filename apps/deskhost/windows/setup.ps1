# Desk Host setup, all in one: installs the built host, writes its settings (served only to the
# listed viewer addresses), registers the at-logon task in this user's session and the inbound
# firewall rule, then starts it. Asks for elevation itself (one UAC prompt).
#   powershell -ep bypass -f setup.ps1 [-Exe <built exe>] [-Peers 10.0.0.222] [-Port 8790]
# Undo: Unregister-ScheduledTask -TaskName "Makepad Desk Host"; Remove-NetFirewallRule -DisplayName "Makepad Desk Host"
param(
    [string]$Exe = "$env:USERPROFILE\rc\rdesk\target\release\makepad-app-deskhost.exe",
    [string[]]$Peers = @("10.0.0.222"),
    [int]$Port = 8790,
    [string]$ForUser = "$env:USERDOMAIN\$env:USERNAME",
    [string]$ForProfile = $env:USERPROFILE
)
$ErrorActionPreference = "Stop"
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    # the elevated copy acts for this user and this profile
    $a = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Exe `"$Exe`" -Port $Port -ForUser `"$ForUser`" -ForProfile `"$ForProfile`" -Peers $($Peers -join ',')"
    Start-Process powershell -Verb RunAs -Wait -ArgumentList $a
    exit
}
if (-not (Test-Path $Exe)) { throw "no Desk Host build at $Exe" }

# 1. the installed copy (rebuilds in the build folder do not touch the running host)
$dir = Join-Path $ForProfile "AppData\Local\Makepad\DeskHost"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Get-ScheduledTask -TaskName "Makepad Desk Host" -ErrorAction SilentlyContinue | Stop-ScheduledTask -ErrorAction SilentlyContinue
$installed = Join-Path $dir "makepad-app-deskhost.exe"
Copy-Item -Force $Exe $installed

# 2. settings: this machine's LAN address, the viewers' addresses only
$home_ = Join-Path $ForProfile ".makepad"
New-Item -ItemType Directory -Force -Path $home_ | Out-Null
$ip = (Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -like "10.*" -or $_.IPAddress -like "192.168.*" -or $_.IPAddress -like "172.*" } | Where-Object { $_.PrefixOrigin -ne "WellKnown" } | Select-Object -First 1).IPAddress
if (-not $ip) { throw "no private LAN address on this machine" }
$fleet = Join-Path $home_ "weights\fleet"
if (-not (Test-Path (Join-Path $fleet "fleet.key"))) { Write-Warning "no fleet key in $fleet yet: the host will not serve until fleet.key is there" }
@(
    "# Desk Host (written by setup.ps1)",
    "enabled = on",
    "host = $ip",
    "port = $Port",
    "allow_peers = $($Peers -join ', ')"
) | Set-Content -Encoding ascii (Join-Path $home_ "deskhost.conf")

# 3. at logon, in this user's interactive session (desktop duplication needs it)
$action = New-ScheduledTaskAction -Execute $installed -WorkingDirectory $dir
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $ForUser
$principal = New-ScheduledTaskPrincipal -UserId $ForUser -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName "Makepad Desk Host" -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null

# 4. inbound TCP from the viewers only, private networks only
Remove-NetFirewallRule -DisplayName "Makepad Desk Host" -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName "Makepad Desk Host" -Direction Inbound -Action Allow -Protocol TCP `
    -LocalPort $Port -RemoteAddress $Peers -Program $installed -Profile Private | Out-Null

Start-ScheduledTask -TaskName "Makepad Desk Host"
Write-Host "Desk Host serves $ip`:$Port to $($Peers -join ', ') (log: $home_\deskhost.log)"
