# The devhub NODE on a Windows box (node 105, 10.0.0.105), as its desktop user: the devhub tray in the
# notification area, started at logon. Never the master (that is the user's Mac).
#   powershell -ep bypass -f tools\devhub\windows\install-node.ps1 [-Name 105] [-Checkout <dir>] [-Start] [-Enable]
# 1. writes %USERPROFILE%\.makepad\devhub\node.conf when there is none (name, host, port 8797,
#    discovery = off: the master dials the node from its nodes.conf) and sync.conf (the synced
#    checkout; no WM to build or restart);
# 2. writes node-start.cmd next to them: `cargo run --release -p makepad-app-devhub` in the checkout,
#    so the hub that runs is always the checkout's (no installed copy goes stale); its log is node.log;
# 3. registers the per-user task "Makepad devhub node" (at logon, restarted when it fails) that runs
#    node-start.cmd under a headless console: no console window, and nothing the hub starts (git,
#    cargo, lease runs) opens one either;
# 4. allows inbound TCP 8797 to the hub's exe from the local subnet (the fleet identity decides who
#    gets in; the rule keeps Windows from asking on the desktop when it first listens).
# -Start starts the task now; -Enable turns a node the tray's Quit switched off back on.
# The node proves itself with the machine's fleet identity (%USERPROFILE%\.makepad\fleet,
# tools/fleet-identity.md); without one it serves this machine's JITs locally only.
# Role and budgets are the master's: `makepad devhub node 105 role pool` (kept in lease.conf here).
# Undo: Unregister-ScheduledTask -TaskName "Makepad devhub node" -Confirm:$false;
#       Remove-NetFirewallRule -DisplayName "Makepad devhub node"; the pid in .makepad\devhub\node.pid ends the hub.
param(
    [string]$Name = "",
    [string]$Checkout = "$env:USERPROFILE\makepad",
    [string]$HostAddr = "",
    [int]$Port = 8797,
    [switch]$Start,
    [switch]$Enable
)
$ErrorActionPreference = "Stop"
$task = "Makepad devhub node"
$h = if ($env:MAKEPAD_HOME) { $env:MAKEPAD_HOME } else { Join-Path $env:USERPROFILE ".makepad" }
$dir = Join-Path $h "devhub"
$conf = Join-Path $dir "node.conf"
$sync = Join-Path $dir "sync.conf"
$startCmd = Join-Path $dir "node-start.cmd"
$log = Join-Path $dir "node.log"
$target = Join-Path $env:USERPROFILE "devhub\target\hub"
$exe = Join-Path $target "release\makepad-app-devhub.exe"
if (-not (Test-Path (Join-Path $Checkout "apps\devhub\Cargo.toml"))) { throw "no makepad checkout at $Checkout" }
New-Item -ItemType Directory -Force -Path $dir | Out-Null

# 1. the node's settings (kept when they exist)
if (-not (Test-Path $conf) -or (Get-Item $conf).Length -eq 0) {
    $lines = @("# devhub node (tools/devhub/windows/install-node.ps1)")
    if ($Name) { $lines += "name = $Name" }
    if ($HostAddr) { $lines += "host = $HostAddr" }
    $lines += "port = $Port", "discovery = off"
    Set-Content -Path $conf -Value $lines -Encoding ascii
    Write-Host "wrote $conf"
} else {
    Write-Host "kept $conf"
}
if ($Enable) {
    $kept = @(Get-Content $conf | Where-Object { $_ -notmatch '^\s*enabled' })
    Set-Content -Path $conf -Value $kept -Encoding ascii
    Write-Host "enabled the node in $conf"
}
if (-not (Test-Path $sync)) {
    Set-Content -Path $sync -Value @("# devhub sync on this node (no WM: nothing to build or restart)", "checkout = $Checkout", "build = off", "wm_service = none") -Encoding ascii
    Write-Host "wrote $sync"
}

# 2. the start script
Set-Content -Path $startCmd -Encoding ascii -Value @(
    "@echo off",
    "rem devhub node at logon (tools/devhub/windows/install-node.ps1): cargo run, so the hub is always the checkout's",
    "cd /d `"$Checkout`"",
    "set CARGO_TARGET_DIR=$target",
    "set CARGO_INCREMENTAL=0",
    "set CARGO_NET_OFFLINE=true",
    "echo ---- %DATE% %TIME% devhub node start >> `"$log`"",
    "cargo run --release -p makepad-app-devhub >> `"$log`" 2>&1"
)

# 3. the at-logon task in this user's session (the tray lives there)
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\conhost.exe" -Argument "--headless cmd.exe /c `"$startCmd`"" -WorkingDirectory $Checkout
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
Write-Host "registered '$task' for $user (at logon)"

# 4. the inbound rule (needs an elevated shell; ssh as this admin user is one)
try {
    Remove-NetFirewallRule -DisplayName $task -ErrorAction SilentlyContinue
    New-NetFirewallRule -DisplayName $task -Direction Inbound -Action Allow -Protocol TCP -LocalPort $Port `
        -RemoteAddress LocalSubnet -Program $exe -Profile Any | Out-Null
    Write-Host "allowed TCP $Port to $exe from the local subnet"
} catch {
    Write-Host "firewall rule not set (run elevated): $_"
}

if ($Start) {
    Start-ScheduledTask -TaskName $task
    Write-Host "started '$task'"
}
