# Installs the AI node (:8123) and the remote tunnel (:8384) of a Windows
# AIHub box as Windows services. Run once per box from an elevated PowerShell:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File aihub-node-install-services.ps1
#
# The stage directory (default: this script's directory) holds
# makepad-service-host.exe, makepad-ai-hub.exe (the node, built per
# tools/aihub-farm.md) and makepad-remote.exe (the TLS tunnel server).
#
# Accounts and rights (least privilege):
# - MakepadAiNode: the service host runs as LocalSystem only so it can start
#   the node in the console user's session as that user (local-use
#   admission watches that session); with nobody logged on the node runs as
#   LocalSystem in session 0.
# - MakepadTunnel: runs as the virtual account NT SERVICE\MakepadTunnel, not
#   an administrator. It may modify the tunnel's working directory (the
#   checkout), the cargo cache, C:\ai\services\node (node updates) and
#   C:\ai\services\tunnel (its own binary, keys and identity); it may start,
#   stop and query MakepadAiNode and no other service. Those are the only
#   privileged actions reachable through the tunnel (see tools/remote/TUNNEL.md).
#
# The tunnel keeps its TLS identity and keys from phase 1 (the per-user
# server in %USERPROFILE%\.makepad\tunnel, or -SeedDir), so clients' pins and
# keys stay valid. The new service is probed with a real TLS handshake and
# key check before the old watchdog task is removed; if the probe fails the
# old task is restored and started again, so the box stays reachable.
#
# Layout (C:\ai\services): service host and configs, admin-only; node\ the
# node binary; tunnel\ the tunnel binary, server-keys, identity\, audit.log
# and tunnel.log, readable only by the tunnel account and administrators;
# logs\ the node log.
param(
    [string]$StageDir = $PSScriptRoot,
    [string]$InstallDir = 'C:\ai\services',
    # Default: taken from the node now serving -Port (its --cache-dir and
    # its .cmd launcher), else the usual C:\ai layout.
    [string]$CacheDir,
    [string]$Launcher,
    [string]$Repo = (Join-Path $env:USERPROFILE 'makepad'),
    # The tunnel's working directory; default: the one its current watchdog
    # task starts it in, else -Repo.
    [string]$TunnelWorkDir,
    # Phase-1 tunnel keys and identity to carry over.
    [string]$SeedDir = (Join-Path $env:USERPROFILE '.makepad\tunnel'),
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
$TunnelAccount = "NT SERVICE\$TunnelService"
$hostExe = Join-Path $InstallDir 'makepad-service-host.exe'
$nodeDir = Join-Path $InstallDir 'node'
$nodeExe = Join-Path $nodeDir 'makepad-ai-hub.exe'
$tunnelDir = Join-Path $InstallDir 'tunnel'
$tunnelExe = Join-Path $tunnelDir 'makepad-remote.exe'
$tunnelKeys = Join-Path $tunnelDir 'server-keys'
$tunnelIdentity = Join-Path $tunnelDir 'identity'
$logDir = Join-Path $InstallDir 'logs'

function Write-Step([string]$text) { Write-Host "== $text" }

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
Write-Step "node cache $CacheDir, launcher settings from $Launcher"

# The old tunnel watchdog tasks, and the working directory they use.
$oldTunnelTasks = @()
if ($doTunnel) {
    $oldTunnelTasks = @(Get-ScheduledTask | Where-Object {
        ((@($_.Actions.Execute) + @($_.Actions.Arguments)) -join ' ') -match '(?i)makepad-remote|cargo-makepad\S*\s.*tunnel'
    })
    if (-not $TunnelWorkDir) {
        foreach ($task in $oldTunnelTasks) {
            if ((($task.Actions.Arguments) -join ' ') -match "-WorkingDirectory '([^']+)'") { $TunnelWorkDir = $Matches[1]; break }
        }
    }
    if (-not $TunnelWorkDir) { $TunnelWorkDir = $Repo }
    Write-Step "tunnel working directory $TunnelWorkDir"
}

$needed = @('makepad-service-host.exe')
if ($doNode) { $needed += 'makepad-ai-hub.exe' }
if ($doTunnel) { $needed += 'makepad-remote.exe' }
foreach ($file in $needed) {
    if (-not (Test-Path (Join-Path $StageDir $file))) { throw "missing $file in $StageDir" }
}
if ($doNode -and -not (Test-Path (Join-Path $CacheDir 'node-key'))) {
    throw "no node-key in $CacheDir; pass the node's existing -CacheDir so it keeps its identity"
}
if ($doTunnel -and -not (Test-Path (Join-Path $SeedDir 'server-keys')) -and -not (Test-Path $tunnelKeys)) {
    throw "no tunnel keys: neither $SeedDir\server-keys nor $tunnelKeys exists (makepad-remote keygen <host> on the client, see tools/remote/TUNNEL.md)"
}

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
# still in use is moved aside first.
function Install-Binary([string]$name, [string]$dir) {
    $source = Join-Path $StageDir $name
    $target = Join-Path $dir $name
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

function Install-Service([string]$name, [string]$display, [string]$description, [string]$config, [string]$account) {
    $binPath = "`"$hostExe`" `"$config`""
    if (-not (Get-Service $name -ErrorAction SilentlyContinue)) {
        New-Service -Name $name -BinaryPathName $binPath -DisplayName $display -StartupType Automatic | Out-Null
    }
    & sc.exe config $name binPath= $binPath start= auto obj= $account DisplayName= $display | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe could not configure $name" }
    # A virtual account is identified by its service SID.
    & sc.exe sidtype $name unrestricted | Out-Null
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
    $null
}

function Grant([string]$path, [string]$rights) {
    if (-not (Test-Path $path)) { return }
    Write-Step "granting $TunnelAccount $rights on $path"
    & icacls.exe $path /grant "$($TunnelAccount):$rights" /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls failed on $path" }
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

# --- install directories -----------------------------------------------------
Write-Step "install directory $InstallDir"
New-Item -ItemType Directory -Force $InstallDir, $logDir, $nodeDir | Out-Null
& icacls.exe $InstallDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-32-545:(OI)(CI)RX' /Q | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'icacls failed on the install directory' }
Install-Binary 'makepad-service-host.exe' $InstallDir

# --- node ----------------------------------------------------------------------
$cudaPath = [Environment]::GetEnvironmentVariable('CUDA_PATH', 'Machine')
if ($doNode) {
    $oldNodeTasks = @(Get-ScheduledTask | Where-Object {
        ((@($_.Actions.Execute) + @($_.Actions.Arguments)) -join ' ') -match '(?i)run-node|makepad-ai-content|makepad-asset-ai|makepad-ai-hub'
    })
    foreach ($task in $oldNodeTasks) {
        Write-Step "removing scheduled task $($task.TaskPath)$($task.TaskName)"
        Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath -Confirm:$false
    }
    Stop-OurService $NodeService
    Stop-OldListener $Port
    Install-Binary 'makepad-ai-hub.exe' $nodeDir
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
    Install-Service $NodeService 'Makepad AI node' "Makepad AIHub node on port $Port (cache $CacheDir)" $config 'LocalSystem'
    Remove-ProgramRules @('*\makepad-ai-content.exe', '*\makepad-asset-ai.exe', '*\makepad-ai-hub.exe', '*\makepad-app-ai-hub.exe') @("Makepad AI Content $Port")
    Set-LanRule "Makepad AI node $Port (LAN)" $Port
}

# --- tunnel --------------------------------------------------------------------
$tunnelOk = $false
if ($doTunnel) {
    New-Item -ItemType Directory -Force $tunnelDir, $TunnelWorkDir | Out-Null
    # Seed keys and identity (never overwrite a newer service copy with an
    # older seed: the service may have rotated its keys since).
    if (-not (Test-Path $tunnelKeys)) {
        Copy-Item (Join-Path $SeedDir 'server-keys') $tunnelKeys
        if (Test-Path (Join-Path $SeedDir 'identity')) {
            New-Item -ItemType Directory -Force $tunnelIdentity | Out-Null
            Copy-Item (Join-Path $SeedDir 'identity\*') $tunnelIdentity -Force
        }
    }
    Install-Binary 'makepad-remote.exe' $tunnelDir
    $lines = @(
        "name=$TunnelService", 'session=system', "cwd=$TunnelWorkDir",
        "log=$(Join-Path $tunnelDir 'tunnel.log')",
        'env=MAKEPAD_REMOTE_SUPERVISED=1'
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
    foreach ($arg in @('--server', '--all', '--port', "$TunnelPort", '--bind', 'lan', '--keys', $tunnelKeys, '--identity', $tunnelIdentity, '--audit-log', (Join-Path $tunnelDir 'audit.log'))) { $lines += "arg=$arg" }
    $config = Write-Config $TunnelService $lines
    Install-Service $TunnelService 'Makepad remote tunnel' "Makepad TLS tunnel on port $TunnelPort in $TunnelWorkDir (LAN only, keyed)" $config $TunnelAccount

    # Rights of the tunnel account (only now does its SID resolve).
    $sid = (New-Object Security.Principal.NTAccount $TunnelAccount).Translate([Security.Principal.SecurityIdentifier]).Value
    & icacls.exe $tunnelDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' "*$($sid):(OI)(CI)M" /T /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'icacls failed on the tunnel directory' }
    & icacls.exe $InstallDir /grant "*$($sid):(OI)(CI)RX" /Q | Out-Null
    Grant $nodeDir '(OI)(CI)M'
    Write-Step 'granting the tunnel its working directory and cargo cache (can take a few minutes on a big checkout)'
    Grant $TunnelWorkDir '(OI)(CI)M'
    if ($TunnelWorkDir -ne $Repo) { Grant $Repo '(OI)(CI)M' }
    Grant (Join-Path $profileDir '.cargo') '(OI)(CI)M'
    Grant (Join-Path $profileDir '.rustup') '(OI)(CI)RX'
    # Start, stop and query MakepadAiNode: the node part of the tunnel's
    # admin allowlist. No other service is granted.
    if (Get-Service $NodeService -ErrorAction SilentlyContinue) {
        $sddl = ((& sc.exe sdshow $NodeService) -join '').Trim()
        $ace = "(A;;CCLCSWRPWPLORC;;;$sid)"
        if ($sddl -notlike "*;;;$sid)*") {
            $new = if ($sddl -match '^(D:.*?)(S:.*)$') { $Matches[1] + $ace + $Matches[2] } else { $sddl + $ace }
            & sc.exe sdset $NodeService $new | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "could not grant the tunnel control of $NodeService" }
        }
    }
    # Git as the tunnel account in a checkout owned by the user.
    $git = Get-Command git.exe -ErrorAction SilentlyContinue
    if ($git) {
        $safe = & git.exe config --system --get-all safe.directory
        foreach ($dir in @($Repo, $TunnelWorkDir) | Select-Object -Unique) {
            if (@($safe) -notcontains $dir.Replace('\', '/')) { & git.exe config --system --add safe.directory $dir.Replace('\', '/') }
        }
    }

    # Switch over: the old watchdog is disabled (not yet removed), the old
    # server stopped, the service started and probed end to end.
    foreach ($task in $oldTunnelTasks) {
        Write-Step "disabling scheduled task $($task.TaskPath)$($task.TaskName)"
        Disable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null
    }
    Stop-OurService $TunnelService
    Stop-OldListener $TunnelPort
    Set-LanRule "Makepad tunnel $TunnelPort (LAN)" $TunnelPort
    Start-Service $TunnelService
    $process = Wait-Port $TunnelPort $tunnelExe
    if ($process) {
        $ip = (Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway } | Select-Object -First 1).IPv4Address.IPAddress
        & $tunnelExe --probe "$($ip):$TunnelPort" --keys $tunnelKeys --identity $tunnelIdentity
        $tunnelOk = $LASTEXITCODE -eq 0
    }
    if ($tunnelOk) {
        foreach ($task in $oldTunnelTasks) {
            Write-Step "removing scheduled task $($task.TaskPath)$($task.TaskName)"
            Unregister-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath -Confirm:$false
        }
        Remove-ProgramRules @('*\makepad-remote.exe') @()
        # The per-user phase-1 copy of the key is no longer needed.
        if (Test-Path (Join-Path $SeedDir 'server-keys')) { Remove-Item (Join-Path $SeedDir 'server-keys') -Force }
    } else {
        Write-Step "the tunnel service did not pass its probe; see $tunnelDir\tunnel.log. Restoring the old tunnel."
        Stop-OurService $TunnelService
        & sc.exe config $TunnelService start= disabled | Out-Null
        foreach ($task in $oldTunnelTasks) {
            Enable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null
            Start-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath
        }
    }
}

# --- start and verify the node -------------------------------------------------
$result = [ordered]@{ host = $env:COMPUTERNAME; install_dir = $InstallDir }
if ($doNode) {
    Start-Service $NodeService
    $process = Wait-Port $Port $nodeExe
    if (-not $process) { throw "$nodeExe is not listening on :$Port after 90 s; see $logDir" }
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
    $fp = (Get-Content (Join-Path $tunnelIdentity 'tls-fingerprint.txt') -ErrorAction SilentlyContinue | Select-Object -First 1)
    $result.tunnel = [ordered]@{ ok = $tunnelOk; account = $TunnelAccount; cwd = $TunnelWorkDir; certificate_sha256 = $fp }
}
$result | ConvertTo-Json -Depth 4
if ($doTunnel -and -not $tunnelOk) { exit 1 }
