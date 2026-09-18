# Start detached download of Qwen/Qwen3.8-27B @ pinned revision to C:\ai\models\qwen38\hf
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path C:\ai\models\qwen38 | Out-Null
$py = @'
import os, time
os.environ["HF_HUB_DISABLE_SYMLINKS"] = "1"
from huggingface_hub import snapshot_download
t0 = time.time()
p = snapshot_download(
    repo_id="Qwen/Qwen3.8-27B",
    revision="1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0",
    local_dir=r"C:\ai\models\qwen38\hf",
    max_workers=8,
)
print("SNAPSHOT_DONE", p, "secs", round(time.time() - t0, 1), flush=True)
'@
Set-Content -Path C:\ai\models\qwen38\dl.py -Value $py -Encoding UTF8
# WMI-detached so it survives tunnel teardown
$cmd = 'cmd /c "python C:\ai\models\qwen38\dl.py >> C:\ai\models\qwen38\dl.log 2>&1"'
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
Write-Output ("DL_LAUNCH pid=" + $r.ProcessId + " rc=" + $r.ReturnValue)
