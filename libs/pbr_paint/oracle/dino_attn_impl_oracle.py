"""Compare AutoModel last_hidden_state under default vs eager attention."""
from __future__ import annotations

import hashlib
import json
import os
import sys

VENV = r"C:\ai\venv_paint\Scripts\python.exe"
if os.path.isfile(VENV) and os.path.normcase(os.path.abspath(sys.executable)) != os.path.normcase(
    os.path.abspath(VENV)
):
    os.execv(VENV, [VENV, *sys.argv])

import numpy as np
import torch

DINO = r"C:\Users\playe\.cache\huggingface\hub\models--facebook--dinov2-giant\snapshots\611a9d42f2335e0f921f1e313ad3c1b7178d206d"
DUMP = r"C:\Users\playe\makepad\local\pbrpaint\dino\black_last_hidden.f32"


def sha(t):
    arr = t.detach().float().cpu().contiguous().numpy().astype("<f4", copy=False)
    return hashlib.sha256(arr.tobytes()).hexdigest()


def max_abs(a, b):
    d = (a.float() - b.float()).abs()
    return float(d.max()), float(d.mean())


@torch.no_grad()
def main() -> int:
    from transformers import AutoImageProcessor, AutoModel
    from PIL import Image

    device = torch.device("cuda")
    proc = AutoImageProcessor.from_pretrained(DINO, local_files_only=True)
    img = Image.fromarray(np.zeros((512, 512, 3), dtype=np.uint8))
    pixels = proc(images=img, return_tensors="pt")["pixel_values"].to(device=device, dtype=torch.float32)
    dumped = torch.from_numpy(np.fromfile(DUMP, dtype="<f4")).view(1, 257, 1536)

    report = {}
    for impl in (None, "eager", "sdpa"):
        kwargs = {"local_files_only": True, "torch_dtype": torch.float32}
        if impl is not None:
            kwargs["attn_implementation"] = impl
        model = AutoModel.from_pretrained(DINO, **kwargs).to(device).eval()
        used = getattr(model.config, "_attn_implementation", None)
        last = model(pixels).last_hidden_state
        mx, mn = max_abs(last.cpu(), dumped)
        report[str(impl)] = {
            "config_impl": used,
            "vs_dump_max": mx,
            "vs_dump_mean": mn,
            "digest": sha(last),
            "head": last.reshape(-1)[:8].float().cpu().tolist(),
        }
        print(f"IMPL {impl} used={used} vs_dump max={mx:.6e} mean={mn:.6e}", flush=True)
        del model
        torch.cuda.empty_cache()
    print("DINO_ATTN_IMPL " + json.dumps(report, sort_keys=True))
    print("DINO_ATTN_IMPL_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
