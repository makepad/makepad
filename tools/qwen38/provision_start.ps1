# Start detached provisioning: llama.cpp b10430 win-cuda-13.3 binaries + cudart + source clone at tag
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path C:\ai\qwen38 | Out-Null
$curl = 'C:\Windows\System32\curl.exe'
$u1 = 'https://github.com/ggml-org/llama.cpp/releases/download/b10430/llama-b10430-bin-win-cuda-13.3-x64.zip'
$u2 = 'https://github.com/ggml-org/llama.cpp/releases/download/b10430/cudart-llama-bin-win-cuda-13.3-x64.zip'
$inner = "cd /d C:\ai\qwen38 && $curl -sL --retry 3 --retry-delay 2 -o llama-cuda.zip $u1 && $curl -sL --retry 3 --retry-delay 2 -o cudart.zip $u2 && (if not exist bin mkdir bin) && tar -xf llama-cuda.zip -C bin && tar -xf cudart.zip -C bin && git clone --depth 1 --branch b10430 https://github.com/ggml-org/llama.cpp llamacpp-src && echo PROVISION_DONE"
$cmd = 'cmd /c "' + $inner + '" >> C:\ai\qwen38\provision.log 2>&1'
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
Write-Output ("PROVISION_LAUNCH pid=" + $r.ProcessId + " rc=" + $r.ReturnValue)
