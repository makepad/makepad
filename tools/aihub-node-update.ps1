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
$oldPid = $listen.OwningProcess
# Two layouts: the MakepadAiNode service from
# tools/aihub-node-install-services.ps1 (the binary named in its config; the
# tunnel's account may stop and start that service and write its node
# folder, nothing more), or a .cmd launcher started by a watchdog task.
$service = Get-Service MakepadAiNode -ErrorAction SilentlyContinue
if ($service -and $service.Status -eq 'Running') {
    $cfg = 'C:\ai\services\MakepadAiNode.cfg'
    $exe = ((Get-Content $cfg | Where-Object { $_ -like 'exe=*' } | Select-Object -First 1) -replace '^exe=', '')
    if (-not $exe -or -not (Test-Path $exe)) { throw "cannot read the node binary from $cfg" }
    $launcher = $null
} else {
    $service = $null
    $old = Get-CimInstance Win32_Process -Filter "ProcessId=$oldPid"
    if ($old.Name -notmatch '^makepad-(ai-hub|ai-content|asset-ai)\.exe$') { throw 'port owner is not an AIHub executable' }
    $exe = $old.ExecutablePath
    $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$($old.ParentProcessId)"
    if ($parent.Name -ne 'cmd.exe' -or $parent.CommandLine -notmatch '(?i)/c\s+"?([^"\r\n]+\.cmd)"?\s*$') { throw 'cannot safely identify the existing AIHub launcher' }
    $launcher = $Matches[1]
}
$dir = Split-Path $exe
if ($launcher -and (-not (Test-Path $launcher) -or (Split-Path $launcher) -ne $dir)) { throw 'launcher is outside the AIHub installation directory' }
# The node serves only TLS: query /health pinned to the node's own
# certificate (made from its fleet key, so its public key is this machine's),
# proving the machine's own fleet credential (tools/fleet-identity.md; the
# node role may read /health). jobs_pending counts queued and running jobs.
if ($service) {
    $cfgArgs = @(Get-Content $cfg | Where-Object { $_ -like 'arg=*' } | ForEach-Object { $_ -replace '^arg=', '' })
    $cacheDir = $cfgArgs[[array]::IndexOf($cfgArgs, '--cache-dir') + 1]
} else {
    $cacheDir = [regex]::Match($old.CommandLine, '--cache-dir\s+("([^"]+)"|(\S+))').Groups | Where-Object { $_.Success } | Select-Object -Last 1 -ExpandProperty Value
}
# The node's fleet identity: MAKEPAD_FLEET_DIR from the service config, else
# the node process owner's %USERPROFILE%\.makepad\fleet.
$fleetDir = $null
if ($service) {
    $fleetDir = (Get-Content $cfg | Where-Object { $_ -like 'env=MAKEPAD_FLEET_DIR=*' } | Select-Object -First 1) -replace '^env=MAKEPAD_FLEET_DIR=', ''
}
if (-not $fleetDir) {
    $ownerSid = (Invoke-CimMethod -InputObject (Get-CimInstance Win32_Process -Filter "ProcessId=$oldPid") -MethodName GetOwnerSid).Sid
    $profileDir = (Get-CimInstance Win32_UserProfile -Filter "SID='$ownerSid'").LocalPath
    $fleetDir = Join-Path $profileDir '.makepad\fleet'
}
function ConvertFrom-Hex([string]$hex) {
    $out = [byte[]]::new($hex.Length / 2)
    for ($i = 0; $i -lt $out.Length; $i++) { $out[$i] = [Convert]::ToByte($hex.Substring(2 * $i, 2), 16) }
    $out
}
$credential = (Get-Content (Join-Path $fleetDir 'id.cred') -ErrorAction Stop | Select-Object -First 1).Trim()
if (-not $credential.StartsWith('mkid1.')) { throw 'id.cred is not a fleet credential' }
# id.key: X9.63 hex, 04 || X || Y || d
$key = ConvertFrom-Hex (Get-Content (Join-Path $fleetDir 'id.key') -ErrorAction Stop | Select-Object -First 1).Trim()
if ($key.Length -ne 97 -or $key[0] -ne 4) { throw 'id.key is not a P-256 key' }
$ownPoint = [byte[]]$key[0..64]
$ecParams = [System.Security.Cryptography.ECParameters]::new()
$ecParams.Curve = [System.Security.Cryptography.ECCurve+NamedCurves]::nistP256
$q = [System.Security.Cryptography.ECPoint]::new()
$q.X = [byte[]]$key[1..32]
$q.Y = [byte[]]$key[33..64]
$ecParams.Q = $q
$ecParams.D = [byte[]]$key[65..96]
$signer = [System.Security.Cryptography.ECDsa]::Create($ecParams)
if (-not ('FleetPin' -as [type])) {
    Add-Type -TypeDefinition @'
using System.Net;
using System.Net.Security;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
public static class FleetPin {
    public static string Expected;
    static bool Check(object sender, X509Certificate cert, X509Chain chain, SslPolicyErrors errors) {
        if (cert == null || Expected == null) return false;
        using (var sha = SHA256.Create()) {
            var hex = System.BitConverter.ToString(sha.ComputeHash(cert.GetRawCertData())).Replace("-", "").ToLowerInvariant();
            return hex == Expected;
        }
    }
    public static void Install(string expected) {
        Expected = expected;
        ServicePointManager.ServerCertificateValidationCallback = Check;
        ServicePointManager.SecurityProtocol = SecurityProtocolType.Tls12 | (SecurityProtocolType)12288;
    }
}
'@
}
# The node's certificate is fetched on every call (a TLS handshake and no
# request: the front does not count it as a failed credential), since a node
# makes a new one each start; it must carry this machine's own key. The proof
# is ECDSA P-256 (id.key, SHA-256, r||s hex) over
# "mkid1 proof|<certificate sha256 hex>|<unix time>".
function Get-NodeFingerprint {
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $Port)
    try {
        $ssl = [System.Net.Security.SslStream]::new($tcp.GetStream(), $false, [System.Net.Security.RemoteCertificateValidationCallback]{ $true })
        $ssl.AuthenticateAsClient('makepad-node', $null, [System.Security.Authentication.SslProtocols]::Tls12, $false)
        $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($ssl.RemoteCertificate)
        $ssl.Dispose()
    } finally { $tcp.Close() }
    if ([Convert]::ToBase64String($cert.GetPublicKey()) -ne [Convert]::ToBase64String($ownPoint)) { throw 'the node on this port does not hold this machine''s fleet key' }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    -join ($sha.ComputeHash($cert.RawData) | ForEach-Object { $_.ToString('x2') })
}
function Get-Health([int]$timeout) {
    $pin = Get-NodeFingerprint
    [FleetPin]::Install($pin)
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $sig = $signer.SignData([Text.Encoding]::ASCII.GetBytes("mkid1 proof|$pin|$now"), [System.Security.Cryptography.HashAlgorithmName]::SHA256)
    $authorization = "MKID $credential.$now." + (-join ($sig | ForEach-Object { $_.ToString('x2') }))
    Invoke-RestMethod "https://127.0.0.1:$Port/health" -Headers @{ Authorization = $authorization } -TimeoutSec $timeout
}
$before = Get-Health 5
if ($before.jobs_pending -ne 0) { throw 'AIHub has active jobs; wait for them to finish before updating' }
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
function Start-Node {
    if ($service) { Start-Service $service.Name; return }
    Start-ExistingLauncher
}
function Stop-Node {
    if ($service) {
        Stop-Service $service.Name -Force
        (Get-Service $service.Name).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
        return
    }
    $oldProcess = Get-Process -Id $oldPid
    $oldProcess.Kill()
    if (-not $oldProcess.WaitForExit(10000)) { throw 'old AIHub process did not exit' }
}
function Start-ExistingLauncher {
    # Hidden and detached from the tunnel job, with the original startup script.
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ShowWindow=[uint16]0}
    $started = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine=('cmd.exe /c "' + $launcher + '"');CurrentDirectory=$dir;ProcessStartupInformation=$startup}
    if ($started.ReturnValue -ne 0) { throw "launcher creation failed: $($started.ReturnValue)" }
    $script:launchPid = $started.ProcessId
}
function Wait-Healthy {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while ([DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 500
        try {
            $health = Get-Health 2
            $owner = Get-NetTCPConnection -LocalPort $Port -State Listen | Select-Object -First 1
            # A service node may run as another user, whose process path this
            # account cannot read; the service config names the binary.
            $path = if ($service) { $exe } else { (Get-CimInstance Win32_Process -Filter "ProcessId=$($owner.OwningProcess)").ExecutablePath }
            if ($path -eq $exe -and $owner.OwningProcess -ne $oldPid -and $health.node_key -eq $before.node_key) { return $health }
        } catch {}
    }
    throw 'replacement AIHub did not become healthy within 60 seconds'
}
try {
    foreach ($task in Get-ScheduledTask) {
        if ($task.State -ne 'Disabled' -and (($task.Actions.Arguments -join ' ') -like "*$dir*")) {
            Disable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null
            $disabled += $task
        }
    }
    # Recheck after suspending the watchdog; never deliberately interrupt work.
    if ((Get-Health 5).jobs_pending -ne 0) { throw 'a job arrived while preparing the update; retry when idle' }
    $stopped = $true
    Stop-Node
    # Even a partially failed copy must be restored from the complete backup.
    $replaced = $true
    Copy-Binary $Payload
    if ((Get-FileHash $exe -Algorithm SHA256).Hash -ne $Sha256) { throw 'installed binary hash mismatch' }
    Start-Node
    $after = Wait-Healthy
    [pscustomobject]@{host=$env:COMPUTERNAME;path=$exe;launcher=$launcher;service=$service.Name;sha256=$Sha256;cuda_arch=$CudaArch;previous_sha256=$oldHash;backup=$backup;previous_pid=$oldPid;health=$after} | ConvertTo-Json -Depth 10 -Compress
} catch {
    $failure = $_
    if ($stopped) {
        # Only this service port or a child of our replacement launcher may
        # be stopped. Another service using the same executable stays alone.
        if ($service) {
            Stop-Service $service.Name -Force -ErrorAction SilentlyContinue
        } else {
            $owner = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
            Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $exe -and (($owner -and $_.ProcessId -eq $owner.OwningProcess) -or ($launchPid -and $_.ParentProcessId -eq $launchPid)) } | ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
        }
        if ($replaced) { Copy-Binary $backup }
        Start-Node
        try { Wait-Healthy | Out-Null } catch { Write-Error "rollback failed: $_" -ErrorAction Continue }
    }
    throw $failure
} finally {
    foreach ($task in $disabled) { Enable-ScheduledTask -TaskName $task.TaskName -TaskPath $task.TaskPath | Out-Null }
}
