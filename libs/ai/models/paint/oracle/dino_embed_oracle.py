"""Dump DINOv2-giant embedding tensor shapes (header only, no ViT forward).

Re-execs C:\\ai\\venv_paint when staged as system python.
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


def find_dino_dir() -> str:
    env = os.environ.get("MAKEPAD_DINO_PATH")
    if env and os.path.isdir(env):
        return env
    hub = os.path.join(os.path.expanduser("~"), ".cache", "huggingface", "hub")
    prefix = os.path.join(hub, "models--facebook--dinov2-giant")
    snaps = os.path.join(prefix, "snapshots")
    if os.path.isdir(snaps):
        for name in os.listdir(snaps):
            cand = os.path.join(snaps, name)
            if os.path.isfile(os.path.join(cand, "model.safetensors")):
                return cand
    return "facebook/dinov2-giant"


KEYS = (
    "embeddings.patch_embeddings.projection.weight",
    "embeddings.patch_embeddings.projection.bias",
    "embeddings.cls_token",
    "embeddings.position_embeddings",
    "embeddings.mask_token",
    "encoder.layer.0.norm1.weight",
    "encoder.layer.0.layer_scale1.lambda1",
    "encoder.layer.0.mlp.w12.weight",
    "encoder.layer.0.mlp.w3.weight",
)


def main() -> int:
    path = find_dino_dir()
    st = os.path.join(path, "model.safetensors")
    from safetensors import safe_open

    shapes = {}
    with safe_open(st, framework="pt", device="cpu") as f:
        keys = list(f.keys())
        for k in KEYS:
            if k in keys:
                shapes[k] = list(f.get_tensor(k).shape)
        layer_count = sum(1 for k in keys if k.startswith("encoder.layer.") and k.endswith(".norm1.weight"))
        pos = f.get_tensor("embeddings.position_embeddings")
        report = {
            "path": path,
            "n_tensors": len(keys),
            "layer_count": layer_count,
            "shapes": shapes,
            "pos_tokens": int(pos.shape[1]),
            "pos_hidden": int(pos.shape[-1]),
        }
    print("DINO_EMBED_ORACLE " + json.dumps(report, sort_keys=True))
    print("DINO_EMBED_ORACLE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
