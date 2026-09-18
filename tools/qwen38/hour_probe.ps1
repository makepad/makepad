$ErrorActionPreference = "Continue"
Write-Output "=== cuda_exec ==="
Get-ChildItem "C:/ai/qwen38cuda/build/src/libs/llama/src/cuda_exec" | ForEach-Object { $_.Name }
Write-Output "=== fattn ==="
if (Test-Path "C:/ai/qwen38cuda/build/src/libs/llama/src/cuda_exec/fattn") {
    Get-ChildItem "C:/ai/qwen38cuda/build/src/libs/llama/src/cuda_exec/fattn" | ForEach-Object { $_.Name }
} else {
    Write-Output "missing fattn dir"
}
Write-Output "=== C:/ai ==="
Get-ChildItem "C:/ai" -ErrorAction SilentlyContinue | ForEach-Object { $_.Name }
Write-Output "=== C:/ai/qwen38 ==="
Get-ChildItem "C:/ai/qwen38" -ErrorAction SilentlyContinue | ForEach-Object { $_.Name }
Write-Output "=== C:/ai/qwen38cuda ==="
Get-ChildItem "C:/ai/qwen38cuda" -ErrorAction SilentlyContinue | ForEach-Object { $_.Name }
Write-Output "=== find llama-bench ==="
Get-ChildItem -Path "C:/ai" -Recurse -Filter "llama-bench.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 10 |
    ForEach-Object { $_.FullName }
Write-Output "=== find llamacpp-src mmq ==="
@(
    "C:/ai/qwen38/llamacpp-src",
    "C:/ai/llamacpp-src",
    "C:/ai/qwen38cuda/llamacpp-src",
    "C:/ai/llama.cpp",
    "C:/src/llama.cpp"
) | ForEach-Object { "$_ exists=$(Test-Path $_)" }
Write-Output "=== listeners 8000-8099 ==="
Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
    Where-Object { $_.LocalPort -ge 8000 -and $_.LocalPort -lt 8100 } |
    ForEach-Object { "$($_.LocalPort) pid=$($_.OwningProcess)" }
