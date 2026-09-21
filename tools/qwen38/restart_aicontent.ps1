# Stop the makepad-asset-ai service on 8767 by exact PID (frees warm flux VRAM).
# Relaunch later with relaunch_aicontent.ps1. NEVER blanket-kill (fleet rule).
$ErrorActionPreference = 'Stop'
$cn = Get-NetTCPConnection -State Listen -LocalPort 8767 -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $cn) { Write-Output "NO_SERVICE_ON_8767"; }
else {
  $wp = Get-CimInstance Win32_Process -Filter ("ProcessId=" + $cn.OwningProcess)
  Write-Output ("stopping pid=" + $cn.OwningProcess)
  Write-Output ("cmdline_was=" + $wp.CommandLine)
  if ($wp.ExecutablePath -notmatch 'makepad-asset-ai') { throw ("refusing: pid on 8767 is " + $wp.ExecutablePath) }
  Stop-Process -Id $cn.OwningProcess -Force
  Start-Sleep -Seconds 4
}
nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader
