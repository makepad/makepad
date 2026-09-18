# Small-read poll of download + provision progress
$ErrorActionPreference = 'Continue'
Write-Output "=== dl.log tail ==="
if (Test-Path C:\ai\models\qwen38\dl.log) { Get-Content C:\ai\models\qwen38\dl.log -Tail 6 }
Write-Output "=== hf dir size ==="
if (Test-Path C:\ai\models\qwen38\hf) {
  $m = Get-ChildItem C:\ai\models\qwen38\hf -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum
  Write-Output ("files=" + $m.Count + " bytes=" + $m.Sum + " GB=" + [math]::Round($m.Sum/1GB,2))
}
Write-Output "=== provision.log tail ==="
if (Test-Path C:\ai\qwen38\provision.log) { Get-Content C:\ai\qwen38\provision.log -Tail 6 }
Write-Output "=== C:\ai\qwen38 ==="
if (Test-Path C:\ai\qwen38) { Get-ChildItem C:\ai\qwen38 | Format-Table Name,Length -AutoSize | Out-String }
Write-Output "=== disk free ==="
(Get-PSDrive C).Free / 1GB
