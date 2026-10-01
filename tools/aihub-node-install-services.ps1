# Installs the AI node (:8123) and the remote tunnel (:8384) of a Windows
# AIHub box as Windows services. Run once per box from an elevated PowerShell;
# afterwards the tunnel runs as LocalSystem and can rerun it with -Only Node.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File aihub-node-install-services.ps1
#
# The stage directory (default: this script's directory) holds
# makepad-service-host.exe, makepad-ai-hub.exe (the node, built per
# tools/aihub-farm.md) and makepad-remote.exe. They are copied to
# -InstallDir, which only administrators can write, because the services run
# them as LocalSystem.
#
# Both services start at boot without a logged-on user and restart on
# failure. The node runs in the console user's session as that user when
# someone is logged on (its local-use admission watches that session), and
# as LocalSystem in session 0 otherwise. The tunnel always runs as
# LocalSystem in the checkout. Inbound firewall rules allow both ports from
# the local subnet only, and the earlier program-scoped allow rules for the
# node and tunnel executables are removed.
#
# The node keeps its cache directory, so its model weights and durable node
# identity (node-key, peer-secret) are unchanged. Launcher environment from
# the previous run-node.cmd (`set "KEY=VALUE"` lines) is carried over.
param(
    [string]$StageDir = $PSScriptRoot,
    [string]$InstallDir = 'C:\ai\services',
    # Default: taken from the node now serving -Port (its --cache-dir and
    # its .cmd launcher), else the usual C:\ai layout.
    [string]$CacheDir,
    [string]$Launcher,
    [string]$Repo = (Join-Path $env:USERPROFILE 'makepad'),
    [string]$Fleet = 'gen',
    [string]$RemoteAddress = 'LocalSubnet',
    [int]$Port = 8123,
    [int]$TunnelPort = 8384,
    [ValidateSet('Both','Node','Tunnel')][string]$Only = 'Both',
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'run this from an elevated (Administrator) PowerShell'
}
$doNode = $Only -ne 'Tunnel'
$doTunnel = $Only -ne 'Node'
$NodeService = 'MakepadAiNode'
$TunnelService = 'MakepadTunnel'
$hostExe = Join-Path $InstallDir 'makepad-service-host.exe'
$nodeExe = Join-Path $InstallDir 'makepad-ai-hub.exe'
$tunnelExe = Join-Path $InstallDir 'makepad-remote.exe'
$logDir = Join-Path $InstallDir 'logs'

# The running node's cache and launcher, found the way aihub-node-update.ps1
# finds them, so one command line fits every box layout.
$running = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
if ($running) {
    $runningNode = Get-CimInstance Win32_Process -Filter "ProcessId=$($running.OwningProcess)"
    if (-not $CacheDir -and $runningNode.CommandLine -match '--cache-dir\s+("([^"]+)"|(\S+))') { $CacheDir = if ($Matches[2]) { $Matches[2] } else { $Matches[3] } }
    $runningParent = Get-CimInstance Win32_Process -Filter "ProcessId=$($runningNode.ParentProcessId)"
    if (-not $Launcher -and $runningParent -and $runningParent.Name -eq 'cmd.exe' -and $runningParent.CommandLine -match '(?i)/c\s+"?([^"\r\n]+\.cmd)"?\s*$') { $Launcher = $Matches[1] }
}
if (-not $CacheDir) { $CacheDir = 'C:\ai\makepad-ai-content-cache' }
if (-not $Launcher) { $Launcher = 'C:\ai\makepad-ai-content\run-node.cmd' }
Write-Host "== node cache $CacheDir, launcher settings from $Launcher"

$needed = @('makepad-service-host.exe')
if ($doNode) { $needed += 'makepad-ai-hub.exe' }
if ($doTunnel) { $needed += 'makepad-remote.exe' }
foreach ($file in $needed) {
    if (-not (Test-Path (Join-Path $StageDir $file))) { throw "missing $file in $StageDir" }
}
if ($doNode -and -not (Test-Path (Join-Path $CacheDir 'node-key'))) {
    throw "no node-key in $CacheDir; pass the node's existing -CacheDir so it keeps its identity"
}

function Write-Step([string]$text) { Write-Host "== $text" }

function Get-Listener([int]$port) {
    $listen = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listen) { Get-CimInstance Win32_Process -Filter "ProcessId=$($listen.OwningProcess)" }
}

function Stop-OurService([string]$name) {
    $service = Get-Service $name -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped') {
        Stop-Service $name -Force
        $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    }
}

# Stops whatever still serves a port outside our services (the old
# scheduled-task launchers), with the cmd.exe that started it.
function Stop-OldListener([int]$port) {
    $process = Get-Listener $port
    if (-not $process) { return }
    $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$($process.ParentProcessId)"
    Write-Step "stopping old $($process.Name) (pid $($process.ProcessId)) on :$port"
    Stop-Process -Id $process.ProcessId -Force
    if ($parent -and $parent.Name -eq 'cmd.exe') { Stop-Process -Id $parent.ProcessId -Force -ErrorAction SilentlyContinue }
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ((Get-Listener $port) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 300 }
    if (Get-Listener $port) { throw "port $port is still in use" }
}

# A running executable cannot be overwritten but can be renamed, so a binary
# still in use by the other service is moved aside first.
function Install-Binary([string]$name) {
    $source = Join-Path $StageDir $name
    $target = Join-Path $InstallDir $name
    if ((Test-Path $target) -and (Get-FileHash $target).Hash -eq (Get-FileHash $source).Hash) { return }
    try { Copy-Item $source $target -Force }
    catch {
        $aside = "$target.old"
        Remove-Item $aside -Force -ErrorAction SilentlyContinue
        if (Test-Path $aside) { $aside = "$target.old-$([DateTime]::UtcNow.ToString('yyyyMMddHHmmss'))" }
        Rename-Item $target (Split-Path $aside -Leaf)
        Copy-Item $source $target -Force
    }
}

# KEY=VALUE pairs from the old launcher's `set "KEY=VALUE"` lines, with
# %VAR% references to earlier lines expanded. PATH is rebuilt by the host.
function Read-LauncherEnv([string]$path) {
    $vars = [ordered]@{}
    if (-not (Test-Path $path)) { return $vars }
    foreach ($line in Get-Content $path) {
        if ($line -notmatch '^\s*set\s+"([^=]+)=(.*)"\s*$') { continue }
        $key = $Matches[1]; $value = $Matches[2]
        if ($key -ieq 'PATH') { continue }
        foreach ($known in @($vars.Keys)) { $value = $value -replace [regex]::Escape("%$known%"), $vars[$known] }
        $vars[$key] = $value
    }
    $vars
}

function Write-Config([string]$name, [string[]]$lines) {
    $path = Join-Path $InstallDir "$name.cfg"
    [IO.File]::WriteAllLines($path, $lines, (New-Object Text.UTF8Encoding $false))
    $path
}

function Install-Service([string]$name, [string]$display, [string]$description, [string]$config) {
    $binPath = "`"$hostExe`" `"$config`""
    if (Get-Service $name -ErrorAction SilentlyContinue) {
        & sc.exe config $name binPath= $binPath start= auto obj= LocalSystem DisplayName= $display | Out-Null
    } else {
        New-Service -Name $name -BinaryPathName $binPath -DisplayName $display -StartupType Automatic | Out-Null
    }
    & sc.exe description $name $description | Out-Null
    # Restart the host if it dies itself; it restarts its program on its own.
    & sc.exe failure $name reset= 86400 actions= restart/5000/restart/10000/restart/30000 | Out-Null
    & sc.exe failureflag $name 1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe could not configure $name" }
}

function Set-LanRule([string]$display, [int]$port) {
    Get-NetFirewallRule -DisplayName $display -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    New-NetFirewallRule -DisplayName $display -Direction Inbound -Protocol TCP -LocalPort $port `
        -RemoteAddress $RemoteAddress -Action Allow -Profile Any | Out-Null
}

# Earlier allow rules scoped to the node or tunnel executables, open to any
# address. The port rules above replace them.
function Remove-ProgramRules([string[]]$programPatterns, [string[]]$names) {
    $rules = @(Get-NetFirewallApplicationFilter | Where-Object {
        $program = $_.Program; @($programPatterns | Where-Object { $program -like $_ }).Count -gt 0
    } | Get-NetFirewallRule)
    foreach ($name in $names) { $rules += @(Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue) }
    foreach ($rule in $rules | Where-Object { $_.Direction -eq 'Inbound' }) {
        Write-Step "removing firewall rule '$($rule.DisplayName)'"
        $rule | Remove-NetFirewallRule -ErrorAction SilentlyContinue
    }
}

function Wait-Port([int]$port, [string]$exe) {
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    while ([DateTime]::UtcNow -lt $deadline) {
        $process = Get-Listener $port
        if ($process -and $process.ExecutablePath -eq $exe) { return $process }
        Start-Sleep -Milliseconds 500
    }
    throw "$exe is not listening on :$port after 90 s; see $logDir"
}

# --- preflight: never interrupt node work -----------------------------------
# A busy fleet node has short gaps between jobs; wait up to 15 minutes for one.
$before = $null
if ($doNode -and (Get-Listener $Port)) {
    $deadline = [DateTime]::UtcNow.AddMinutes(15)
    while ($true) {
        try {
            $before = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 5
            $jobs = Invoke-RestMethod "http://127.0.0.1:$Port/jobs" -TimeoutSec 5
        } catch {
            Write-Step "node on :$Port does not answer; replacing it"
            break
        }
        if ((@($jobs.jobs).Count -eq 0 -and $before.jobs_pending -eq 0) -or $Force) { break }
        if ([DateTime]::UtcNow -gt $deadline) { throw 'the node stayed busy for 15 minutes; rerun later (or with -Force)' }
        Write-Host -NoNewline '.'
        Start-Sleep -Seconds 2
    }
}

# --- install directory: administrators write, users read ---------------------
Write-Step "install directory $InstallDir"
New-Item -ItemType Directory -Force $InstallDir, $logDir | Out-Null
& icacls.exe $InstallDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-32-545:(OI)(CI)RX' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'icacls failed on the install directory' }

# --- the old launchers: scheduled tasks and their processes ------------------
$oldTasks = Get-ScheduledTask | Where-Object {
    $action = (@($_.Actions.Execute) + @($_.Actions.Arguments)) -join ' '
    ($doNode -and $action -match '(?i)run-node|makepad-ai-content|makepad-asset-ai|makepad-ai-hub') -or
    ($doTunnel -and $action -match '(?i)makepad-remote')
}
foreach ($task in $oldTasks) {
    Write-Step "removing scheduled task $($task.TaskPath)$($task.TaskName)"
    Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath -Confirm:$false
}

if ($doNode) {
    Stop-OurService $NodeService
    Stop-OldListener $Port
    Install-Binary 'makepad-ai-hub.exe'
}
if ($doTunnel) {
    Stop-OurService $TunnelService
    Stop-OldListener $TunnelPort
    Install-Binary 'makepad-remote.exe'
}
Install-Binary 'makepad-service-host.exe'

# --- service configs ---------------------------------------------------------
$cudaPath = [Environment]::GetEnvironmentVariable('CUDA_PATH', 'Machine')
if ($doNode) {
    $nodeEnv = Read-LauncherEnv $Launcher
    if ($nodeEnv.Contains('CUDA_PATH')) { $cudaPath = $nodeEnv['CUDA_PATH'] }
    if (-not $cudaPath) { throw 'CUDA_PATH is not set; the node needs the CUDA runtime' }
    $nodeEnv['CUDA_PATH'] = $cudaPath
    $nodeEnv['MAKEPAD_ASSET_AI_PORT'] = "$Port"
    $nodeEnv['MAKEPAD_ASSET_AI_FLEET'] = $Fleet
    $lines = @(
        "name=$NodeService", 'session=user', "cwd=$CacheDir",
        "log=$(Join-Path $logDir 'node.log')", "path=$cudaPath\bin"
    )
    foreach ($key in $nodeEnv.Keys) { $lines += "env=$key=$($nodeEnv[$key])" }
    $lines += "exe=$nodeExe"
    foreach ($arg in @('--port', "$Port", '--host', '0.0.0.0', '--fleet', $Fleet, '--cache-dir', $CacheDir)) { $lines += "arg=$arg" }
    $config = Write-Config $NodeService $lines
    Install-Service $NodeService 'Makepad AI node' "Makepad AIHub node on port $Port (cache $CacheDir)" $config
}
if ($doTunnel) {
    # The tunnel works in the checkout; a box without one gets an empty
    # directory to clone into through the tunnel.
    New-Item -ItemType Directory -Force $Repo | Out-Null
    $lines = @(
        "name=$TunnelService", 'session=system', "cwd=$Repo",
        "log=$(Join-Path $logDir 'tunnel.log')"
    )
    # The checkout owner's Rust toolchain, so builds through the tunnel use it.
    $profileDir = Split-Path $Repo
    if (Test-Path (Join-Path $profileDir '.cargo\bin')) {
        $lines += "path=$(Join-Path $profileDir '.cargo\bin')"
        $lines += "env=CARGO_HOME=$(Join-Path $profileDir '.cargo')"
        $lines += "env=RUSTUP_HOME=$(Join-Path $profileDir '.rustup')"
    }
    if ($cudaPath) { $lines += "path=$cudaPath\bin"; $lines += "env=CUDA_PATH=$cudaPath" }
    $lines += "exe=$tunnelExe"
    foreach ($arg in @('--server', '--port', "$TunnelPort", '--all')) { $lines += "arg=$arg" }
    $config = Write-Config $TunnelService $lines
    Install-Service $TunnelService 'Makepad remote tunnel' "Makepad remote tunnel on port $TunnelPort in $Repo (LAN only)" $config
    # Git as LocalSystem in a checkout owned by the user.
    $git = Get-Command git.exe -ErrorAction SilentlyContinue
    if ($git) {
        $safe = & git.exe config --system --get-all safe.directory
        if (@($safe) -notcontains $Repo.Replace('\', '/')) { & git.exe config --system --add safe.directory $Repo.Replace('\', '/') }
    }
}

# --- firewall: LAN only ------------------------------------------------------
if ($doNode) {
    Remove-ProgramRules @('*\makepad-ai-content.exe', '*\makepad-asset-ai.exe', '*\makepad-ai-hub.exe', '*\makepad-app-ai-hub.exe') @("Makepad AI Content $Port")
    Set-LanRule "Makepad AI node $Port (LAN)" $Port
}
if ($doTunnel) {
    Remove-ProgramRules @('*\makepad-remote.exe') @()
    Set-LanRule "Makepad tunnel $TunnelPort (LAN)" $TunnelPort
}

# --- start and verify ---------------------------------------------------------
$result = [ordered]@{ host = $env:COMPUTERNAME; install_dir = $InstallDir }
if ($doNode) {
    Start-Service $NodeService
    $process = Wait-Port $Port $nodeExe
    $health = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while (-not $health -and [DateTime]::UtcNow -lt $deadline) {
        try { $health = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 3 } catch { Start-Sleep -Seconds 1 }
    }
    if (-not $health) { throw "node does not answer /health; see $logDir\node.log" }
    if ($before -and $before.node_key -and $health.node_key -ne $before.node_key) {
        throw "node identity changed ($($before.node_key) -> $($health.node_key))"
    }
    $result.node = [ordered]@{ pid = $process.ProcessId; version = $health.version; node_key = $health.node_key; admission_open = $health.activity.admission_open; sha256 = (Get-FileHash $nodeExe -Algorithm SHA256).Hash }
}
if ($doTunnel) {
    Start-Service $TunnelService
    $process = Wait-Port $TunnelPort $tunnelExe
    $result.tunnel = [ordered]@{ pid = $process.ProcessId; cwd = $Repo }
}
$result | ConvertTo-Json -Depth 4
