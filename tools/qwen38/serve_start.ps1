# Launch llama-server detached at the firewall-allowed path with VRAM sampler + cold-load timing.
# args: [quant-file-basename] [mmproj-basename|none] [port] [extra args joined]
param(
  [string]$Model = 'Qwen3.8-27B-Q4_K_M.gguf',
  [string]$Mmproj = 'mmproj-Qwen3.8-27B-BF16.gguf',
  [int]$Port = 8090,
  [string]$Draft = 'none'
)
$ErrorActionPreference = 'Stop'
$exe = 'C:\Users\playe\llama.cpp\build\bin\release\llama-server.exe'
$out = 'C:\ai\models\qwen38'
$keyfile = 'C:\ai\qwen38\api_key.txt'
if (-not (Test-Path $keyfile)) {
  $k = -join ((1..48) | ForEach-Object { '{0:x}' -f (Get-Random -Maximum 16) })
  Set-Content -Path $keyfile -Value $k -NoNewline -Encoding ascii
}
$key = (Get-Content $keyfile -Raw).Trim()
# stop any previous llama-server we launched
Get-Process llama-server -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
# VRAM sampler: 1s cadence, 3h cap, own log
$samp = @'
$log = "C:\ai\qwen38\vram.log"
for ($i = 0; $i -lt 10800; $i++) {
  $u = (nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits).Trim()
  Add-Content -Path $log -Value ((Get-Date -Format "HH:mm:ss.f") + " " + $u)
  Start-Sleep -Seconds 1
}
'@
Set-Content -Path C:\ai\qwen38\vram_sampler.ps1 -Value $samp -Encoding UTF8
Remove-Item C:\ai\qwen38\vram.log -ErrorAction SilentlyContinue
$r0 = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = 'cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\ai\qwen38\vram_sampler.ps1"' }
Write-Output ("VRAM_SAMPLER pid=" + $r0.ProcessId)
$baseline = (nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits).Trim()
Write-Output ("VRAM_BASELINE_MB " + $baseline)
# server launch
$args8 = "-m $out\$Model --host 0.0.0.0 --port $Port --api-key $key -ngl 99 -c 32768 -ctk q8_0 -ctv q8_0 -fa on --jinja --reasoning-format auto -np 1 --alias qwen3.8-27b"
if ($Mmproj -ne 'none') { $args8 += " --mmproj $out\$Mmproj" }
if ($Draft -ne 'none') { $args8 += " --model-draft $out\$Draft" }
Remove-Item C:\ai\qwen38\server.log -ErrorAction SilentlyContinue
$cmd = 'cmd /c ""' + $exe + '" ' + $args8 + ' >> C:\ai\qwen38\server.log 2>&1"'
$t0 = Get-Date
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
Write-Output ("SERVER_LAUNCH pid=" + $r.ProcessId + " rc=" + $r.ReturnValue + " port=" + $Port + " model=" + $Model + " mmproj=" + $Mmproj + " draft=" + $Draft)
# cold-load: poll /health until 200
$ready = $false
for ($i = 0; $i -lt 600; $i++) {
  Start-Sleep -Milliseconds 500
  try {
    $resp = Invoke-WebRequest -Uri ("http://127.0.0.1:" + $Port + "/health") -UseBasicParsing -TimeoutSec 2
    if ($resp.StatusCode -eq 200) { $ready = $true; break }
  } catch { }
}
$dt = ((Get-Date) - $t0).TotalSeconds
if ($ready) { Write-Output ("COLD_LOAD_SECONDS " + [math]::Round($dt,1)) } else { Write-Output "SERVER_NOT_READY_AFTER_300S"; Get-Content C:\ai\qwen38\server.log -Tail 25 }
$used = (nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits).Trim()
Write-Output ("VRAM_AFTER_LOAD_MB " + $used)
Get-Content C:\ai\qwen38\server.log -Tail 8
