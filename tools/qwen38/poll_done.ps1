# One-line completion check for the two detached jobs
$dl = ""; $pv = ""
if (Test-Path C:\ai\models\qwen38\dl.log) { $dl = (Get-Content C:\ai\models\qwen38\dl.log -Raw) }
if (Test-Path C:\ai\qwen38\provision.log) { $pv = (Get-Content C:\ai\qwen38\provision.log -Raw) }
$sz = 0
if (Test-Path C:\ai\models\qwen38\hf) { $sz = (Get-ChildItem C:\ai\models\qwen38\hf -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum }
$dlDone = $dl -match 'SNAPSHOT_DONE'
$pvDone = $pv -match 'PROVISION_DONE'
$dlErr = $dl -match 'Error|Traceback'
$pvErr = ($pv -match 'error|fatal') -and (-not $pvDone)
Write-Output ("dl_gb=" + [math]::Round($sz/1GB,1) + " dl_done=" + $dlDone + " dl_err=" + $dlErr + " pv_done=" + $pvDone + " pv_err=" + $pvErr)
if ($dlDone -and $pvDone) { Write-Output "ALL_DONE" }
if ($dlErr) { Write-Output "=== dl.log tail ==="; Get-Content C:\ai\models\qwen38\dl.log -Tail 10 }
if ($pvErr) { Write-Output "=== provision.log tail ==="; Get-Content C:\ai\qwen38\provision.log -Tail 10 }
