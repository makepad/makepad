# Reclaim disk: purge the hf xet chunk cache (safe when no download in flight; it's a re-download cache)
$ErrorActionPreference = 'Continue'
$xet = 'C:\Users\playe\.cache\huggingface\xet'
Write-Output ("free_before_GB " + [math]::Round((Get-PSDrive C).Free/1GB,1))
if (Test-Path $xet) {
  $m = Get-ChildItem $xet -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum
  Write-Output ("xet_cache_GB " + [math]::Round($m.Sum/1GB,1))
  Remove-Item $xet -Recurse -Force -ErrorAction SilentlyContinue
  Write-Output "xet_purged"
} else { Write-Output "no_xet_dir" }
Write-Output ("free_after_GB " + [math]::Round((Get-PSDrive C).Free/1GB,1))
