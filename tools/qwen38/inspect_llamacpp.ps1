# Inspect provisioned llama.cpp b10430: converter support, binaries, official convert.log recipe
$ErrorActionPreference = 'Continue'
Write-Output "=== provision.log ==="
Get-Content C:\ai\qwen38\provision.log -Tail 15
Write-Output "=== source checkout ==="
git -C C:\ai\qwen38\llamacpp-src log -1 --format="%H %cI %s"
Write-Output "=== converter: Qwen3_5 registration ==="
Select-String -Path C:\ai\qwen38\llamacpp-src\convert_hf_to_gguf.py -Pattern 'Qwen3_5|qwen3_5' | Select-Object -First 12 | ForEach-Object { $_.LineNumber.ToString() + ": " + $_.Line.Trim() }
Write-Output "=== converter: mmproj + mtp flags ==="
Select-String -Path C:\ai\qwen38\llamacpp-src\convert_hf_to_gguf.py -Pattern '"--mmproj"|"--mtp"|mtp_' | Select-Object -First 12 | ForEach-Object { $_.LineNumber.ToString() + ": " + $_.Line.Trim() }
Write-Output "=== bin dir ==="
Get-ChildItem C:\ai\qwen38\bin -Filter *.exe | Select-Object -First 25 Name | Out-String
Get-ChildItem C:\ai\qwen38\bin -Filter *.dll | Measure-Object | ForEach-Object { "dll_count=" + $_.Count }
Write-Output "=== llama-server version ==="
& C:\ai\qwen38\bin\llama-server.exe --version 2>&1 | Select-Object -First 4
Write-Output "=== relevant server flags present ==="
$help = & C:\ai\qwen38\bin\llama-server.exe --help 2>&1 | Out-String
foreach ($f in @('--api-key','--cache-type-k','--jinja','--reasoning-format','--mmproj','--chat-template-kwargs','--model-draft','--spec-type','--swa-full','--flash-attn')) {
  if ($help -match [regex]::Escape($f)) { Write-Output ("HAS " + $f) } else { Write-Output ("MISSING " + $f) }
}
Write-Output "=== ggml-org official convert.log head (box-side fetch) ==="
C:\Windows\System32\curl.exe -sL -r 0-4000 "https://huggingface.co/ggml-org/Qwen3.8-27B-GGUF/resolve/main/convert.log" -o C:\ai\qwen38\ggmlorg_convert_head.log
Get-Content C:\ai\qwen38\ggmlorg_convert_head.log | Select-Object -First 40
