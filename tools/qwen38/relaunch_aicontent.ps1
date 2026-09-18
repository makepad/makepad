# Relaunch makepad-asset-ai exactly as it was, WMI-detached (survives tunnel teardown)
$ErrorActionPreference = 'Stop'
$exe = 'C:\users\playe\makepad\local\sa3build\libs\ai_content\target\release\makepad-asset-ai.exe'
$cmd = 'cmd /c ""' + $exe + '" --port 8767 --cache-dir C:\ai\sa3\cache >> C:\ai\svc8767.log 2>&1"'
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
Write-Output ("AICONTENT_RELAUNCH pid=" + $r.ProcessId + " rc=" + $r.ReturnValue)
Start-Sleep -Seconds 5
$cn = Get-NetTCPConnection -State Listen -LocalPort 8767 -ErrorAction SilentlyContinue | Select-Object -First 1
if ($cn) { Write-Output ("LISTENING_8767 pid=" + $cn.OwningProcess) } else { Write-Output "NOT_LISTENING_YET (check C:\ai\svc8767.log)" }
