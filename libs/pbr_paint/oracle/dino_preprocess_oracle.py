"""Dump the official DINOv2-giant AutoImageProcessor contract.

Hunyuan `Dino_v2` is just AutoImageProcessor + AutoModel; last_hidden_state
is what `image_proj_model_dino` consumes. This script does not load the ViT.

Re-execs C:\\ai\\venv_paint when staged through the tunnel as system python.
"""
from __future__ import annotations

import json
import os
import sys

VENV = r"C:\ai\venv_paint\Scripts\python.exe"
if os.path.isfile(VENV) and os.path.normcase(os.path.abspath(sys.executable)) != os.path.normcase(
    os.path.abspath(VENV)
):
    os.execv(VENV, [VENV, *sys.argv])

import numpy as np

DEFAULT_DINO = os.environ.get(
    "MAKEPAD_DINO_PATH",
    os.path.join(os.path.expanduser("~"), ".cache", "huggingface", "hub"),
)


def find_dino_dir() -> str:
    env = os.environ.get("MAKEPAD_DINO_PATH")
    if env and os.path.isdir(env):
        return env
    # Typical HF snapshot layout on the paint box.
    hub = os.path.join(os.path.expanduser("~"), ".cache", "huggingface", "hub")
    prefix = os.path.join(hub, "models--facebook--dinov2-giant")
    if os.path.isdir(prefix):
        snaps = os.path.join(prefix, "snapshots")
        if os.path.isdir(snaps):
            for name in os.listdir(snaps):
                cand = os.path.join(snaps, name)
                if os.path.isfile(os.path.join(cand, "preprocessor_config.json")):
                    return cand
    return "facebook/dinov2-giant"


def main() -> int:
    from transformers import AutoImageProcessor
    from PIL import Image

    path = find_dino_dir()
    proc = AutoImageProcessor.from_pretrained(path, local_files_only=os.path.isdir(path))
    cfg = {k: getattr(proc, k, None) for k in (
        "do_resize",
        "do_center_crop",
        "do_rescale",
        "do_normalize",
        "resample",
        "rescale_factor",
        "image_mean",
        "image_std",
        "crop_size",
        "size",
    )}
    # size/crop may be dict-like
    for key in ("crop_size", "size"):
        val = cfg[key]
        if hasattr(val, "height"):
            cfg[key] = {"height": val.height, "width": getattr(val, "width", None)}
        elif hasattr(val, "items"):
            cfg[key] = dict(val)

    img = Image.fromarray(np.zeros((512, 512, 3), dtype=np.uint8))
    out = proc(images=img, return_tensors="np")
    pv = out["pixel_values"]
    report = {
        "processor_type": type(proc).__name__,
        "path": path,
        "config": cfg,
        "pixel_values_shape": list(pv.shape),
        "pixel_values_dtype": str(pv.dtype),
        "pixel_min": float(pv.min()),
        "pixel_max": float(pv.max()),
        "pixel_mean": float(pv.mean()),
    }
    print("DINO_PREPROCESS_ORACLE " + json.dumps(report, sort_keys=True, default=str))
    print("DINO_PREPROCESS_ORACLE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
