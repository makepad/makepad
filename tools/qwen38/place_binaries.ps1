# Place b10430 binaries at the firewall-allowed llama-server path (backup existing dir first)
$ErrorActionPreference = 'Stop'
$rel = 'C:\Users\playe\llama.cpp\build\bin\release'
$bak = 'C:\Users\playe\llama.cpp\build\bin\release_pre_qwen38_20260814'
if (Test-Path $rel) {
  if (-not (Test-Path $bak)) {
    Rename-Item -Path $rel -NewName 'release_pre_qwen38_20260814'
    Write-Output "BACKED_UP existing release -> release_pre_qwen38_20260814"
  } else {
    Write-Output "BACKUP ALREADY EXISTS; leaving both"
  }
}
New-Item -ItemType Directory -Force -Path $rel | Out-Null
Copy-Item C:\ai\qwen38\bin\* $rel -Recurse -Force
Write-Output ("PLACED files=" + (Get-ChildItem $rel -File | Measure-Object).Count)
& (Join-Path $rel 'llama-server.exe') --version 2>&1 | Select-Object -First 3
