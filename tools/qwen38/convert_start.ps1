# Launch detached conversion pipeline (official ggml-org recipe, staged for disk headroom)
# Stage A: text BF16 -> Q4_K_M(recipe) + Q5_K_M(recipe) -> delete text BF16
# Stage B: mmproj BF16 + mmproj Q8_0
# Stage C: mtp BF16 -> mtp Q4_0 (--pure) -> delete mtp BF16
# All progress into C:\ai\models\qwen38\convert_ours.log
$ErrorActionPreference = 'Stop'
$work = @'
$ErrorActionPreference = 'Stop'
function Log($m) { Write-Output ("[" + (Get-Date -Format HH:mm:ss) + "] " + $m) }
function FreeGB { [math]::Round((Get-PSDrive C).Free/1GB,1) }
$src = 'C:\ai\qwen38\llamacpp-src'
$hf  = 'C:\ai\models\qwen38\hf'
$out = 'C:\ai\models\qwen38'
$q   = 'C:\ai\qwen38\bin\llama-quantize.exe'
Log ("start freeGB=" + (FreeGB))
Log "text bf16 convert"
python $src\convert_hf_to_gguf.py $hf --outtype bf16 --outfile $out\Qwen3.8-27B-BF16.gguf --no-mtp --model-name Qwen3.8-27B
if ($LASTEXITCODE -ne 0) { throw "text convert failed" }
Log ("text bf16 done freeGB=" + (FreeGB))
Log "quantize Q4_K_M (official recipe)"
& $q --pure --tensor-type output.weight=q6_k --tensor-type shexp=q8_0 --tensor-type latent=q8_0 --tensor-type attn_=q8_0 --tensor-type ssm_=q8_0 $out\Qwen3.8-27B-BF16.gguf $out\Qwen3.8-27B-Q4_K_M.gguf Q4_K_M
if ($LASTEXITCODE -ne 0) { throw "q4km failed" }
Log ("q4km done freeGB=" + (FreeGB))
Log "quantize Q5_K_M (same recipe, higher base)"
& $q --pure --tensor-type output.weight=q6_k --tensor-type shexp=q8_0 --tensor-type latent=q8_0 --tensor-type attn_=q8_0 --tensor-type ssm_=q8_0 $out\Qwen3.8-27B-BF16.gguf $out\Qwen3.8-27B-Q5_K_M.gguf Q5_K_M
if ($LASTEXITCODE -ne 0) { throw "q5km failed" }
Log ("q5km done freeGB=" + (FreeGB))
Log "smoke-load q4km header via llama-gguf-split --help skipped; deleting text BF16"
Remove-Item $out\Qwen3.8-27B-BF16.gguf
Log ("bf16 deleted freeGB=" + (FreeGB))
Log "mmproj bf16"
python $src\convert_hf_to_gguf.py $hf --outtype bf16 --outfile $out\mmproj-Qwen3.8-27B-BF16.gguf --mmproj --model-name Qwen3.8-27B
if ($LASTEXITCODE -ne 0) { throw "mmproj bf16 failed" }
Log "mmproj q8_0"
python $src\convert_hf_to_gguf.py $hf --outtype q8_0 --outfile $out\mmproj-Qwen3.8-27B-Q8_0.gguf --mmproj --model-name Qwen3.8-27B
if ($LASTEXITCODE -ne 0) { throw "mmproj q8 failed" }
Log "mtp bf16"
python $src\convert_hf_to_gguf.py $hf --outtype bf16 --outfile $out\mtp-Qwen3.8-27B-BF16.gguf --mtp --model-name Qwen3.8-27B
if ($LASTEXITCODE -ne 0) { throw "mtp bf16 failed" }
Log "mtp q4_0"
& $q --pure $out\mtp-Qwen3.8-27B-BF16.gguf $out\mtp-Qwen3.8-27B-Q4_0.gguf Q4_0
if ($LASTEXITCODE -ne 0) { throw "mtp q4_0 failed" }
Remove-Item $out\mtp-Qwen3.8-27B-BF16.gguf
Log ("mtp done freeGB=" + (FreeGB))
Log "sha256 artifacts"
foreach ($f in @("Qwen3.8-27B-Q4_K_M.gguf","Qwen3.8-27B-Q5_K_M.gguf","mmproj-Qwen3.8-27B-BF16.gguf","mmproj-Qwen3.8-27B-Q8_0.gguf","mtp-Qwen3.8-27B-Q4_0.gguf")) {
  $h = (Get-FileHash -Algorithm SHA256 (Join-Path $out $f)).Hash.ToLower()
  $len = (Get-Item (Join-Path $out $f)).Length
  Write-Output ("SHA256 " + $f + " " + $h + " bytes=" + $len)
}
Log "CONVERT_ALL_DONE"
'@
Set-Content -Path C:\ai\models\qwen38\convert_job.ps1 -Value $work -Encoding UTF8
$cmd = 'cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\ai\models\qwen38\convert_job.ps1 >> C:\ai\models\qwen38\convert_ours.log 2>&1"'
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
Write-Output ("CONVERT_LAUNCH pid=" + $r.ProcessId + " rc=" + $r.ReturnValue)
