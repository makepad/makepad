"""Official DINOv2-giant AutoModel last_hidden_state taps.

Hunyuan `Dino_v2` is AutoImageProcessor + AutoModel; `[0]` / last_hidden_state
is `[B, 257, 1536]` after the 224-crop (16x16 patches + CLS). This dumps
embeddings, first-block, and full last_hidden_state for a black 512 image
and a deterministic ramp so the native ViT can match fp32.

Re-execs C:\\ai\\venv_paint when staged as system python.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
import time

VENV = r"C:\ai\venv_paint\Scripts\python.exe"
if os.path.isfile(VENV) and os.path.normcase(os.path.abspath(sys.executable)) != os.path.normcase(
    os.path.abspath(VENV)
):
    os.execv(VENV, [VENV, *sys.argv])

import numpy as np
import torch


DINO_SNAP = os.environ.get(
    "MAKEPAD_DINO_PATH",
    r"C:\Users\playe\.cache\huggingface\hub\models--facebook--dinov2-giant\snapshots\611a9d42f2335e0f921f1e313ad3c1b7178d206d",
)
OUT_DIR = os.environ.get(
    "MAKEPAD_DINO_ORACLE_DIR",
    r"C:\Users\playe\makepad\local\pbrpaint\dino",
)
UNET_BIN = os.environ.get(
    "MAKEPAD_HUNYUAN_UNET",
    r"C:\ai\Hunyuan3D-2.1\weights\hunyuan3d-paintpbr-v2-1\unet\diffusion_pytorch_model.bin",
)


def sha256_f32(t: torch.Tensor) -> str:
    arr = t.detach().float().cpu().contiguous().numpy().astype("<f4", copy=False)
    return hashlib.sha256(arr.tobytes()).hexdigest()


def tap_meta(name: str, t: torch.Tensor) -> dict:
    flat = t.detach().float().cpu().contiguous().reshape(-1)
    head = flat[:16].tolist()
    return {
        "name": name,
        "shape": list(t.shape),
        "dtype": str(t.dtype).replace("torch.", ""),
        "n": int(flat.numel()),
        "min": float(flat.min()) if flat.numel() else 0.0,
        "max": float(flat.max()) if flat.numel() else 0.0,
        "mean": float(flat.mean()) if flat.numel() else 0.0,
        "digest": sha256_f32(t),
        "head": head,
    }


def write_f32(path: str, t: torch.Tensor) -> None:
    arr = t.detach().float().cpu().contiguous().numpy().astype("<f4", copy=False)
    with open(path, "wb") as f:
        f.write(arr.tobytes())


def ramp_rgb_u8(size: int = 512) -> np.ndarray:
    """Deterministic HWC uint8 ramp shared with the native canary."""
    ys = np.linspace(0.0, 1.0, size, dtype=np.float64).reshape(size, 1)
    xs = np.linspace(0.0, 1.0, size, dtype=np.float64).reshape(1, size)
    r = np.clip(np.rint((xs + ys) * 0.5 * 255.0), 0, 255).astype(np.uint8)
    g = np.clip(np.rint(xs * 255.0), 0, 255).astype(np.uint8)
    b = np.clip(np.rint(ys * 255.0), 0, 255).astype(np.uint8)
    r = np.broadcast_to(r, (size, size))
    g = np.broadcast_to(g, (size, size))
    b = np.broadcast_to(b, (size, size))
    return np.stack([r, g, b], axis=-1).copy()


def find_dino_dir() -> str:
    env = os.environ.get("MAKEPAD_DINO_PATH")
    if env and os.path.isdir(env):
        return env
    if os.path.isdir(DINO_SNAP) and os.path.isfile(os.path.join(DINO_SNAP, "model.safetensors")):
        return DINO_SNAP
    hub = os.path.join(os.path.expanduser("~"), ".cache", "huggingface", "hub")
    prefix = os.path.join(hub, "models--facebook--dinov2-giant")
    snaps = os.path.join(prefix, "snapshots")
    if os.path.isdir(snaps):
        for name in os.listdir(snaps):
            cand = os.path.join(snaps, name)
            if os.path.isfile(os.path.join(cand, "model.safetensors")):
                return cand
    return "facebook/dinov2-giant"


def dump_keys(path: str) -> dict:
    from safetensors import safe_open

    st = os.path.join(path, "model.safetensors")
    with safe_open(st, framework="pt", device="cpu") as f:
        keys = list(f.keys())
        layer0 = sorted(k for k in keys if k.startswith("encoder.layer.0."))
        shapes = {k: list(f.get_tensor(k).shape) for k in layer0}
        for extra in (
            "embeddings.patch_embeddings.projection.weight",
            "embeddings.patch_embeddings.projection.bias",
            "embeddings.cls_token",
            "embeddings.position_embeddings",
            "embeddings.mask_token",
            "layernorm.weight",
            "layernorm.bias",
            "norm.weight",
            "norm.bias",
        ):
            if extra in keys:
                shapes[extra] = list(f.get_tensor(extra).shape)
        mlp_names = sorted({k.split("encoder.layer.0.")[-1] for k in layer0 if ".mlp." in k})
        attn_names = sorted({k.split("encoder.layer.0.")[-1] for k in layer0 if "attn" in k or "attention" in k})
        layer_count = sum(1 for k in keys if k.startswith("encoder.layer.") and k.endswith(".norm1.weight"))
        if layer_count == 0:
            layer_count = sum(1 for k in keys if k.startswith("blocks.") and k.endswith(".norm1.weight"))
        return {
            "n_tensors": len(keys),
            "layer_count": layer_count,
            "layer0_keys": layer0,
            "mlp_names": mlp_names,
            "attn_names": attn_names,
            "shapes": shapes,
            "sample_keys": keys[:40],
        }


def maybe_project(hidden: torch.Tensor, report: dict, tag: str) -> None:
    if not os.path.isfile(UNET_BIN):
        report[f"{tag}_proj"] = "skip_no_unet"
        return
    try:
        from safetensors.torch import load_file
    except Exception:
        load_file = None
    # Projector lives in the Hunyuan UNet torch bin, not safetensors.
    try:
        obj = torch.load(UNET_BIN, map_location="cpu", weights_only=True)
    except TypeError:
        obj = torch.load(UNET_BIN, map_location="cpu")
    if isinstance(obj, dict) and "state_dict" in obj:
        obj = obj["state_dict"]
    w = obj.get("unet.image_proj_model_dino.proj.weight")
    b = obj.get("unet.image_proj_model_dino.proj.bias")
    nw = obj.get("unet.image_proj_model_dino.norm.weight")
    nb = obj.get("unet.image_proj_model_dino.norm.bias")
    if w is None or b is None or nw is None or nb is None:
        report[f"{tag}_proj"] = "skip_missing_keys"
        return
    x = hidden.detach().float().cpu().reshape(-1, hidden.shape[-1])
    y = torch.nn.functional.linear(x, w.float().cpu(), b.float().cpu())
    y = y.reshape(x.shape[0], 4, 1024)
    mean = y.mean(dim=-1, keepdim=True)
    var = y.var(dim=-1, unbiased=False, keepdim=True)
    y = (y - mean) / torch.sqrt(var + 1e-5)
    y = y * nw.float().view(1, 1, 1024) + nb.float().view(1, 1, 1024)
    report[f"{tag}_proj"] = tap_meta(f"{tag}_proj", y)
    write_f32(os.path.join(OUT_DIR, f"{tag}_proj.f32"), y)


@torch.no_grad()
def run_case(model, proc, name: str, rgb: np.ndarray, device: torch.device, dump_proj: bool) -> dict:
    from PIL import Image

    img = Image.fromarray(rgb)
    pixels = proc(images=img, return_tensors="pt")["pixel_values"].to(device=device, dtype=torch.float32)
    t0 = time.perf_counter()
    embeddings = model.embeddings(pixels)
    t_emb = time.perf_counter() - t0
    t0 = time.perf_counter()
    block0 = model.encoder.layer[0](embeddings)[0]
    t_b0 = time.perf_counter() - t0
    t0 = time.perf_counter()
    last = model(pixels).last_hidden_state
    t_full = time.perf_counter() - t0

    # Isolated first-block intermediates (pre-norm, attn, mlp) when available.
    extras = {}
    layer0 = model.encoder.layer[0]
    n1 = layer0.norm1(embeddings)
    extras["norm1"] = n1
    attn_out = layer0.attention(n1)
    if isinstance(attn_out, tuple):
        attn_out = attn_out[0]
    extras["attn"] = attn_out
    if hasattr(layer0, "layer_scale1"):
        extras["ls1"] = layer0.layer_scale1(attn_out)
    n2 = layer0.norm2(embeddings + extras.get("ls1", attn_out))
    extras["norm2"] = n2
    mlp_out = layer0.mlp(n2)
    extras["mlp"] = mlp_out

    case = {
        "pixels": tap_meta(f"{name}_pixels", pixels),
        "embeddings": tap_meta(f"{name}_embeddings", embeddings),
        "block0": tap_meta(f"{name}_block0", block0),
        "last_hidden": tap_meta(f"{name}_last_hidden", last),
        "seconds": {"embeddings": t_emb, "block0": t_b0, "full": t_full},
    }
    for k, v in extras.items():
        case[k] = tap_meta(f"{name}_{k}", v)
        write_f32(os.path.join(OUT_DIR, f"{name}_{k}.f32"), v)
    write_f32(os.path.join(OUT_DIR, f"{name}_pixels.f32"), pixels)
    write_f32(os.path.join(OUT_DIR, f"{name}_embeddings.f32"), embeddings)
    write_f32(os.path.join(OUT_DIR, f"{name}_block0.f32"), block0)
    write_f32(os.path.join(OUT_DIR, f"{name}_last_hidden.f32"), last)
    print(
        f"DINO_CASE {name} pixels={tuple(pixels.shape)} emb={tuple(embeddings.shape)} "
        f"block0={tuple(block0.shape)} last={tuple(last.shape)} "
        f"digest={case['last_hidden']['digest'][:16]} full_s={t_full:.3f}",
        flush=True,
    )
    if dump_proj:
        maybe_project(last, case, name)
    return case


def main() -> int:
    from transformers import AutoImageProcessor, AutoModel

    path = find_dino_dir()
    os.makedirs(OUT_DIR, exist_ok=True)
    keys = dump_keys(path)
    print("DINO_KEYS " + json.dumps({k: keys[k] for k in ("n_tensors", "layer_count", "mlp_names", "attn_names")}, sort_keys=True))
    print("DINO_LAYER0 " + json.dumps(keys["layer0_keys"]))
    print("DINO_SHAPES " + json.dumps(keys["shapes"], sort_keys=True))

    cfg_path = os.path.join(path, "config.json")
    cfg = {}
    if os.path.isfile(cfg_path):
        with open(cfg_path, "r", encoding="utf-8") as f:
            cfg = json.load(f)
        keep = {
            "hidden_size",
            "num_hidden_layers",
            "num_attention_heads",
            "patch_size",
            "image_size",
            "layer_norm_eps",
            "hidden_act",
            "mlp_ratio",
            "use_swiglu_ffn",
            "qkv_bias",
            "layerscale_value",
            "model_type",
            "architectures",
        }
        print("DINO_CONFIG " + json.dumps({k: cfg.get(k) for k in keep}, sort_keys=True))

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    t0 = time.perf_counter()
    proc = AutoImageProcessor.from_pretrained(path, local_files_only=os.path.isdir(path))
    model = AutoModel.from_pretrained(
        path,
        local_files_only=os.path.isdir(path),
        torch_dtype=torch.float32,
    )
    model = model.to(device).eval()
    print(f"DINO_LOAD_S {time.perf_counter() - t0:.3f} device={device} class={type(model).__name__}")

    black = np.zeros((512, 512, 3), dtype=np.uint8)
    ramp = ramp_rgb_u8(512)
    report = {
        "path": path,
        "device": str(device),
        "model_class": type(model).__name__,
        "keys": keys,
        "config": {k: cfg.get(k) for k in (
            "hidden_size", "num_hidden_layers", "num_attention_heads",
            "patch_size", "image_size", "layer_norm_eps", "use_swiglu_ffn",
            "mlp_ratio", "qkv_bias", "layerscale_value",
        )},
        "out_dir": OUT_DIR,
        "black": run_case(model, proc, "black", black, device, dump_proj=True),
        "ramp": run_case(model, proc, "ramp", ramp, device, dump_proj=True),
    }
    out_json = os.path.join(OUT_DIR, "dino_forward_oracle.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2, sort_keys=True)
    print("DINO_FORWARD_ORACLE " + json.dumps({
        "out": out_json,
        "black_last": report["black"]["last_hidden"]["shape"],
        "ramp_last": report["ramp"]["last_hidden"]["shape"],
        "black_digest": report["black"]["last_hidden"]["digest"],
        "ramp_digest": report["ramp"]["last_hidden"]["digest"],
        "mlp_names": keys["mlp_names"],
    }, sort_keys=True))
    print("DINO_FORWARD_ORACLE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
