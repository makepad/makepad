# qwen38 bring-up recon round 2: firewall program paths, service cmdlines, python, caches
$ErrorActionPreference = 'Continue'
Write-Output "=== FIREWALL PROGRAM PATHS (llama-server / python / makepad / WSL rules) ==="
foreach ($rn in @('llama-server.exe','python.exe','makepad-asset-ai.exe','makepad-remote.exe','WSL 8080','WSL GGUF 9000','comfyui.exe')) {
  $rules = Get-NetFirewallRule -DisplayName $rn -ErrorAction SilentlyContinue
  foreach ($r in $rules) {
    $app = ($r | Get-NetFirewallApplicationFilter -ErrorAction SilentlyContinue).Program
    $port = ($r | Get-NetFirewallPortFilter -ErrorAction SilentlyContinue)
    Write-Output ("RULE '" + $rn + "' dir=" + $r.Direction + " act=" + $r.Action + " en=" + $r.Enabled + " prog=" + $app + " proto=" + $port.Protocol + " lport=" + $port.LocalPort)
  }
}
Write-Output "=== AI-CONTENT SERVICE CMDLINE (pid on 8767) ==="
$cn = Get-NetTCPConnection -State Listen -LocalPort 8767 -ErrorAction SilentlyContinue | Select-Object -First 1
if ($cn) {
  $wp = Get-CimInstance Win32_Process -Filter ("ProcessId=" + $cn.OwningProcess)
  Write-Output ("pid=" + $cn.OwningProcess)
  Write-Output ("exe=" + $wp.ExecutablePath)
  Write-Output ("cmdline=" + $wp.CommandLine)
  Write-Output ("cwd_hint_parent=" + (Get-CimInstance Win32_Process -Filter ("ProcessId=" + $wp.ParentProcessId)).CommandLine)
}
Write-Output "=== PID 4408 (8080/9000 owner) ==="
Get-Process -Id 4408 -ErrorAction SilentlyContinue | Format-List Name,Path | Out-String
(Get-CimInstance Win32_Process -Filter "ProcessId=4408" -ErrorAction SilentlyContinue).CommandLine
Write-Output "=== WSL STATE ==="
wsl -l -v 2>&1 | Out-String
Write-Output "=== PYTHON/PIP ==="
python -m pip --version 2>&1
python -c "import torch; print('torch', torch.__version__)" 2>&1
python -c "import safetensors, transformers; print('safetensors+transformers ok')" 2>&1
Write-Output "=== VENVS under C:\ai (pyvenv.cfg depth<=3) ==="
Get-ChildItem C:\ai -Recurse -Depth 3 -Filter pyvenv.cfg -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName }
Write-Output "=== C:\ai\models CONTENT ==="
Get-ChildItem C:\ai\models -ErrorAction SilentlyContinue | Format-Table Name,Length,LastWriteTime -AutoSize | Out-String
Write-Output "=== EXISTING llama-server ON DISK ==="
where.exe llama-server 2>&1
foreach ($pth in @('C:\ai\llama','C:\Users\playe\llama.cpp','C:\Users\playe\llama','C:\ai\llamacpp')) { if (Test-Path $pth) { Write-Output "EXISTS: $pth" } }
Write-Output "=== CURL CANDIDATES ==="
foreach ($cc in @('C:\Windows\System32\curl.exe','C:\Program Files\Git\mingw64\bin\curl.exe')) { if (Test-Path $cc) { & $cc --version | Select-Object -First 1 | ForEach-Object { Write-Output ("$cc -> " + $_) } } else { Write-Output "$cc MISSING" } }
Write-Output "=== HF CACHE ==="
foreach ($hc in @('C:\Users\playe\.cache\huggingface')) { if (Test-Path $hc) { Get-ChildItem $hc -ErrorAction SilentlyContinue | Format-Table Name,LastWriteTime -AutoSize | Out-String } else { Write-Output "$hc MISSING" } }
Write-Output "=== GPU IDLE BASELINE NOW ==="
nvidia-smi --query-gpu=memory.used,memory.free --format=csv
