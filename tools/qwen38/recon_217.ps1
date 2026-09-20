# qwen38 bring-up: read-only recon of the .217 RTX 5090 box
$ErrorActionPreference = 'Continue'
Write-Output "=== WHO/WHERE ==="
whoami
(Get-Location).Path
Write-Output "=== ADMIN CHECK ==="
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$p = New-Object Security.Principal.WindowsPrincipal($id)
Write-Output ("IsAdminRole: " + $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))
Write-Output "=== GPU ==="
nvidia-smi --query-gpu=name,memory.total,memory.used,memory.free,driver_version --format=csv
Write-Output "=== GPU PROCESSES ==="
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv
Write-Output "=== DISKS ==="
Get-PSDrive -PSProvider FileSystem | Format-Table Name,@{n='UsedGB';e={[math]::Round($_.Used/1GB,1)}},@{n='FreeGB';e={[math]::Round($_.Free/1GB,1)}} -AutoSize | Out-String
Write-Output "=== TOOLING ==="
foreach ($c in @('python','git','cmake','nvcc','curl','tar')) {
  $w = (Get-Command $c -ErrorAction SilentlyContinue)
  if ($w) { Write-Output ("$c -> " + $w.Source) } else { Write-Output ("$c -> MISSING") }
}
python --version 2>&1
nvcc --version 2>&1 | Select-String release
Write-Output "=== C:\ai LAYOUT ==="
if (Test-Path C:\ai) { Get-ChildItem C:\ai | Format-Table Name,Length,LastWriteTime -AutoSize | Out-String }
Write-Output "=== LLAMA BINARIES ANYWHERE OBVIOUS ==="
foreach ($d in @('C:\ai\llama.cpp','C:\ai\llama','C:\llama.cpp','C:\ai\bin')) {
  if (Test-Path $d) { Write-Output "EXISTS: $d"; Get-ChildItem $d -ErrorAction SilentlyContinue | Select-Object -First 15 Name | Out-String }
}
Get-Command llama-server,llama-cli -ErrorAction SilentlyContinue | Format-Table Name,Source | Out-String
Write-Output "=== LISTENING PORTS (8080,8767,8384,plus llama-ish) ==="
$conns = Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalPort -in 8080,8081,8082,8090,8384,8767,8768,9000,9090 }
foreach ($cn in $conns) {
  $proc = Get-Process -Id $cn.OwningProcess -ErrorAction SilentlyContinue
  Write-Output ("port " + $cn.LocalPort + " addr " + $cn.LocalAddress + " pid " + $cn.OwningProcess + " exe " + $proc.Path)
}
Write-Output "=== FIREWALL: custom-looking inbound allows (skip stock groups) ==="
$rules = netsh advfirewall firewall show rule name=all dir=in
$block = @(); $keep = $false
foreach ($line in $rules) {
  if ($line -match '^Rule Name:') {
    if ($keep -and $block.Count -gt 0) { $block | ForEach-Object { Write-Output $_ }; Write-Output "--" }
    $block = @($line); $keep = $true
  } elseif ($line -match '^Grouping:\s+\S') { $keep = $false; $block += $line }
  elseif ($line -match '^(Enabled|LocalPort|Program|Action|Profiles):') { $block += $line }
}
if ($keep -and $block.Count -gt 0) { $block | ForEach-Object { Write-Output $_ } }
