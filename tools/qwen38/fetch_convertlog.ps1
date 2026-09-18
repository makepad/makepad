# Pull interesting parts of ggml-org convert.log (box-side; grep for invocations + warnings)
$ErrorActionPreference = 'Continue'
C:\Windows\System32\curl.exe -sL "https://huggingface.co/ggml-org/Qwen3.8-27B-GGUF/resolve/main/convert.log" -o C:\ai\qwen38\ggmlorg_convert.log
$sz = (Get-Item C:\ai\qwen38\ggmlorg_convert.log).Length
Write-Output ("LOG_SIZE " + $sz)
Write-Output "=== invocation lines (+, python3, quantize) ==="
Select-String -Path C:\ai\qwen38\ggmlorg_convert.log -Pattern '^\+ ' | ForEach-Object { $_.Line } | Select-Object -First 30
Write-Output "=== tokenizer / pre / chkhsh lines ==="
Select-String -Path C:\ai\qwen38\ggmlorg_convert.log -Pattern 'chkhsh|pre-tokenizer|tokenizer.ggml.pre|WARNING' | ForEach-Object { $_.Line } | Select-Object -First 15
Write-Output "=== arch + hparam summary lines ==="
Select-String -Path C:\ai\qwen38\ggmlorg_convert.log -Pattern 'Model architecture|expert|rope|context_length|embedding_length|block_count|Set model|special tokens|chat template' | ForEach-Object { $_.Line } | Select-Object -First 25
