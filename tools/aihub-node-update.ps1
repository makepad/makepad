# Rolling update of one Windows AIHub, run through cargo-makepad tunnel --no-sync.
# Keep the existing executable path (firewall rules), launcher, cache and GPU policy.
param(
    [Parameter(Mandatory=$true)][string]$Payload,
    [Parameter(Mandatory=$true)][string]$Sha256,
    [Parameter(Mandatory=$true)][string]$CudaArch,
    [int]$Port = 8123
)
$ErrorActionPreference = 'Stop'
$Payload = (Resolve-Path $Payload).Path
if ((Get-FileHash $Payload -Algorithm SHA256).Hash -ne $Sha256) { throw 'payload SHA256 mismatch' }
$gpuCapabilities = & nvidia-smi --query-gpu=compute_cap --format=csv,noheader
if ($LASTEXITCODE -ne 0) { throw 'cannot query node CUDA architecture' }
$capability = @($gpuCapabilities)[0].Trim()
if ($capability -notmatch '^([0-9]+)\.([0-9]+)$') { throw 'cannot establish node CUDA architecture' }
$nodeArch = $Matches[1] + $Matches[2]
if ([int]$Matches[1] -ge 12) { $nodeArch += 'a' }
if ($CudaArch -ne $nodeArch) { throw "binary CUDA architecture $CudaArch does not match node $nodeArch" }
$listen = Get-NetTCPConnection -LocalPort $Port -State Listen | Select-Object -First 1
$old = Get-CimInstance Win32_Process -Filter "ProcessId=$($listen.OwningProcess)"
if ($old.Name -notmatch '^makepad-(ai-hub|ai-content|asset-ai)\.exe$') { throw 'port owner is not an AIHub executable' }
$exe = $old.ExecutablePath
$dir = Split-Path $exe
$parent = Get-CimInstance Win32_Process -Filter "ProcessId=$($old.ParentProcessId)"
if ($parent.Name -ne 'cmd.exe' -or $parent.CommandLine -notmatch '(?i)/c\s+"?([^"\r\n]+\.cmd)"?\s*$') { throw 'cannot safely identify the existing AIHub launcher' }
$launcher = $Matches[1]
if (-not (Test-Path $launcher) -or (Split-Path $launcher) -ne $dir) { throw 'launcher is outside the AIHub installation directory' }
$before = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 5
$jobs = Invoke-RestMethod "http://127.0.0.1:$Port/jobs" -TimeoutSec 5
if ($before.jobs_pending -ne 0 -or @($jobs.jobs).Count -ne 0) { throw 'AIHub has active jobs; wait for them to finish before updating' }
$stamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')
$backup = "$exe.rollback-$stamp"
Copy-Item $exe $backup
$oldHash = (Get-FileHash $backup -Algorithm SHA256).Hash
# Disable only watchdog tasks whose action names this installation directory.
# Restore precisely the tasks we disabled, including on rollback.
$disabled = @()
$stopped = $false
$replaced = $false
$launchPid = $null
function Copy-Binary([string]$source) {
    # Windows can retain the executable mapping briefly during GPU teardown.
    for ($attempt=0; $attempt -lt 20; $attempt++) {
        try { Copy-Item $source $exe -Force; return }
        catch { if ($attempt -eq 19) { throw }; Start-Sleep -Milliseconds 500 }
    }
}
function Start-ExistingLauncher {
    # Hidden and detached from the tunnel job, with the original startup script.
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ShowWindow=[uint16]0}
    $started = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine=('cmd.exe /c "' + $launcher + '"');CurrentDirectory=$dir;ProcessStartupInformation=$startup}
    if ($started.ReturnValue -ne 0) { throw "launcher creation failed: $($started.ReturnValue)" }
    $script:launchPid = $started.ProcessId
}
function Wait-Healthy {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 500
        try {
            $health = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 2
            $owner = Get-NetTCPConnection -LocalPort $Port -State Listen | Select-Object -First 1
            $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$($owner.OwningProcess)"
            if ($proc.ExecutablePath -eq $exe -and $proc.ProcessId -ne $old.ProcessId -and $health.node_key -eq $before.node_key) { return $health }
        } catch {}
    }
    throw 'replacement AIHub did not become healthy within 30 seconds'
}
try {
    foreach ($task in Get-ScheduledTask) {
        if ($task.State -ne 'Disabled' -and (($task.Actions.Arguments -join ' ') -like "*$dir*")) {
            Disable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null
            $disabled += $task
        }
    }
    # Recheck after suspending the watchdog; never deliberately interrupt work.
    $jobs = Invoke-RestMethod "http://127.0.0.1:$Port/jobs" -TimeoutSec 5
    if (@($jobs.jobs).Count -ne 0) { throw 'a job arrived while preparing the update; retry when idle' }
    $oldProcess = Get-Process -Id $old.ProcessId
    $oldProcess.Kill()
    $stopped = $true
    if (-not $oldProcess.WaitForExit(10000)) { throw 'old AIHub process did not exit' }
    # Even a partially failed copy must be restored from the complete backup.
    $replaced = $true
    Copy-Binary $Payload
    if ((Get-FileHash $exe -Algorithm SHA256).Hash -ne $Sha256) { throw 'installed binary hash mismatch' }
    Start-ExistingLauncher
    $after = Wait-Healthy
    [pscustomobject]@{host=$env:COMPUTERNAME;path=$exe;launcher=$launcher;sha256=$Sha256;cuda_arch=$CudaArch;previous_sha256=$oldHash;backup=$backup;previous_pid=$old.ProcessId;health=$after} | ConvertTo-Json -Depth 10 -Compress
} catch {
    $failure = $_
    if ($stopped) {
        # Only this service port or a child of our replacement launcher may
        # be stopped. Another service using the same executable stays alone.
        $owner = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $exe -and (($owner -and $_.ProcessId -eq $owner.OwningProcess) -or ($launchPid -and $_.ParentProcessId -eq $launchPid)) } | ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
        if ($replaced) { Copy-Binary $backup }
        Start-ExistingLauncher
        try { Wait-Healthy | Out-Null } catch { Write-Error "rollback failed: $_" -ErrorAction Continue }
    }
    throw $failure
} finally {
    foreach ($task in $disabled) { Enable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null }
}
