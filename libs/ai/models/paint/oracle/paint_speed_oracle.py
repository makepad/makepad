"""Official Hunyuan3D-Paint-2.1 denoise-loop speed oracle.

Times the same 15-step 3-branch CFG UNet loop native cares about
(view-select / remesh / ESRGAN / bake are not included). Official demo
loads fp16 and swaps the checkpoint DDIM scheduler for UniPC; native
stays on checkpoint DDIM 15 v-pred ZSNR trailing. This script times both.

Weights: C:\\ai\\Hunyuan3D-2.1\\weights\\hunyuan3d-paintpbr-v2-1
Code:    C:\\ai\\Hunyuan3D-2.1 @ 82920d643c0dc2f7bfd7255f45f62d386edfe60c
Python:  C:\\ai\\venv_paint\\Scripts\\python.exe (re-exec if needed)

Prints:
  ORACLE_SPEED size= views= sched= warm_s= denoise_s= peak_mib=
"""
from __future__ import annotations

import os
import sys
import time
from pathlib import Path

VENV_PY = Path(os.environ.get("MAKEPAD_PAINT_PYTHON", r"C:\ai\venv_paint\Scripts\python.exe"))


def _reexec_venv() -> None:
    if os.environ.get("MAKEPAD_PAINT_ORACLE_REEXEC") == "1":
        return
    if not VENV_PY.is_file():
        return
    here = Path(sys.executable).resolve()
    want = VENV_PY.resolve()
    if here == want:
        return
    os.environ["MAKEPAD_PAINT_ORACLE_REEXEC"] = "1"
    os.execv(str(want), [str(want), str(Path(__file__).resolve()), *sys.argv[1:]])


_reexec_venv()

import argparse
import json

import numpy as np
import torch

DEFAULT_ROOT = Path(os.environ.get("MAKEPAD_HUNYUAN_ROOT", r"C:\ai\Hunyuan3D-2.1"))
DEFAULT_WEIGHTS = DEFAULT_ROOT / "weights" / "hunyuan3d-paintpbr-v2-1"
PAINT_SRC = DEFAULT_ROOT / "hy3dpaint"
STEPS = 15
GUIDANCE = 3.0
N_PBR = 2
N_REF = 1


def _peak_mib() -> float:
    if not torch.cuda.is_available():
        return 0.0
    return float(torch.cuda.max_memory_allocated()) / (1024.0 * 1024.0)


def _sync() -> None:
    if torch.cuda.is_available():
        torch.cuda.synchronize()


def _cam_mapping(azim: float) -> float:
    if 0.0 <= azim < 90.0:
        return azim / 90.0 + 1.0
    if 90.0 <= azim < 330.0:
        return 2.0
    return -azim / 90.0 + 5.0


def _view_scales(n_views: int, device: torch.device, dtype: torch.dtype) -> torch.Tensor:
    # Native pipeline default 6-view ring. Front-ish, sides, back.
    azims = [0.0, 60.0, 120.0, 180.0, 240.0, 300.0][:n_views]
    if n_views > 6:
        azims = [i * (360.0 / n_views) for i in range(n_views)]
    scales = [_cam_mapping(a) for a in azims]
    return (
        torch.tensor(scales, device=device, dtype=dtype)
        .unsqueeze(0)
        .repeat(N_PBR, 1)
        .reshape(-1)[:, None, None, None]
    )


def _make_scheduler(weights: Path, kind: str, device: torch.device):
    from diffusers import DDIMScheduler, UniPCMultistepScheduler

    base = DDIMScheduler.from_pretrained(str(weights / "scheduler"))
    if kind == "ddim":
        sched = base
    elif kind == "unipc":
        sched = UniPCMultistepScheduler.from_config(base.config, timestep_spacing="trailing")
    else:
        raise ValueError(f"unknown scheduler {kind}")
    sched.set_timesteps(STEPS, device=device)
    return sched


@torch.no_grad()
def _build_cond(unet, size: int, n_views: int, device: torch.device, dtype: torch.dtype):
    lat = size // 8
    gen = torch.Generator(device="cpu").manual_seed(0)
    sample = torch.randn(1, N_PBR, n_views, 4, lat, lat, generator=gen).to(device=device, dtype=dtype)
    embeds_normal = torch.randn(1, n_views, 4, lat, lat, generator=gen).to(device=device, dtype=dtype)
    embeds_position = torch.randn(1, n_views, 4, lat, lat, generator=gen).to(device=device, dtype=dtype)
    ref_latents = torch.randn(1, N_REF, 4, lat, lat, generator=gen).to(device=device, dtype=dtype)
    # Official voxel pyramid is computed from image-res position maps.
    position_maps = torch.rand(1, n_views, 3, size, size, generator=gen).to(device=device, dtype=dtype)
    dino = torch.randn(1, 257, 1536, generator=gen).to(device=device, dtype=dtype)

    enc_alb = unet.unet.learned_text_clip_albedo.unsqueeze(0).to(device=device, dtype=dtype)
    enc_mr = unet.unet.learned_text_clip_mr.unsqueeze(0).to(device=device, dtype=dtype)
    prompt = torch.stack([enc_alb, enc_mr], dim=1)
    # Official CFG: [neg, pos, pos] then dino [0, 0, dino], ref_scale [0, 1, 1].
    prompt_embeds = torch.cat([prompt, prompt, prompt], dim=0)
    prompt_embeds[0].zero_()

    cond = {
        "num_in_batch": n_views,
        "embeds_normal": embeds_normal.repeat(3, *([1] * (embeds_normal.dim() - 1))),
        "embeds_position": embeds_position.repeat(3, *([1] * (embeds_position.dim() - 1))),
        "position_maps": position_maps.repeat(3, *([1] * (position_maps.dim() - 1))),
        "ref_latents": ref_latents.repeat(3, *([1] * (ref_latents.dim() - 1))),
        "ref_scale": torch.as_tensor([0.0, 1.0, 1.0], device=device, dtype=dtype),
        "mva_scale": 1.0,
        "dino_hidden_states": torch.cat([torch.zeros_like(dino), torch.zeros_like(dino), dino], dim=0),
        "cache": {},
    }
    return sample, prompt_embeds, cond


@torch.no_grad()
def _unet_step(unet, latents, t, prompt_embeds, cond, n_views: int):
    # Official denoise(): rearrange -> repeat 3 -> scale_model_input -> unet.
    lat_5d = latents.reshape(1, N_PBR, n_views, *latents.shape[1:])
    model_in = lat_5d.repeat(3, 1, 1, 1, 1, 1)
    flat = model_in.reshape(-1, *model_in.shape[3:])
    return unet(
        model_in,
        t,
        encoder_hidden_states=prompt_embeds,
        return_dict=False,
        **cond,
    )[0], lat_5d, flat


@torch.no_grad()
def time_loop(unet, weights: Path, size: int, n_views: int, kind: str, device: torch.device, dtype: torch.dtype):
    if torch.cuda.is_available():
        torch.cuda.reset_peak_memory_stats()
        torch.cuda.empty_cache()

    t_warm = time.perf_counter()
    sample, prompt_embeds, cond = _build_cond(unet, size, n_views, device, dtype)
    sched = _make_scheduler(weights, kind, device)
    view_scale = _view_scales(n_views, device, dtype)
    # Dual-stream write-cache + one warmup UNet (official caches this).
    latents = sample.reshape(-1, *sample.shape[3:]).contiguous()
    t0 = sched.timesteps[0]
    pred, _, _ = _unet_step(unet, latents, t0, prompt_embeds, cond, n_views)
    _ = pred.float().mean().item()
    _sync()
    warm_s = time.perf_counter() - t_warm

    # Fresh noise; reuse the write-cache / dino proj sitting in cond["cache"].
    gen = torch.Generator(device="cpu").manual_seed(1)
    lat = size // 8
    latents = torch.randn(N_PBR * n_views, 4, lat, lat, generator=gen).to(device=device, dtype=dtype)
    _sync()
    t_den = time.perf_counter()
    for t in sched.timesteps:
        noise_pred, _, _ = _unet_step(unet, latents, t, prompt_embeds, cond, n_views)
        noise_u, noise_r, noise_f = noise_pred.chunk(3)
        guided = noise_u + GUIDANCE * view_scale * (noise_r - noise_u)
        guided = guided + GUIDANCE * view_scale * (noise_f - noise_r)
        latents = sched.step(guided, t, latents, return_dict=False)[0]
    _sync()
    denoise_s = time.perf_counter() - t_den
    peak = _peak_mib()
    finite = bool(torch.isfinite(latents).all().item())
    print(
        f"ORACLE_SPEED size={size} views={n_views} sched={kind} "
        f"warm_s={warm_s:.3f} denoise_s={denoise_s:.3f} peak_mib={peak:.1f}"
    )
    print(
        f"ORACLE_SPEED_META size={size} views={n_views} sched={kind} "
        f"dtype=float16 steps={STEPS} cfg_batch=3 finite={int(finite)} "
        f"latent={lat}x{lat} timesteps={ [int(x) for x in sched.timesteps.detach().cpu().tolist()] }"
    )
    if not finite:
        raise RuntimeError(f"non-finite latents after {kind} {size}x{n_views}")
    return {
        "size": size,
        "views": n_views,
        "sched": kind,
        "warm_s": warm_s,
        "denoise_s": denoise_s,
        "peak_mib": peak,
        "dtype": "float16",
        "finite": finite,
    }


def load_unet(weights: Path, device: torch.device, dtype: torch.dtype):
    sys.path.insert(0, str(PAINT_SRC))
    from hunyuanpaintpbr.unet.modules import UNet2p5DConditionModel

    t0 = time.perf_counter()
    unet = UNet2p5DConditionModel.from_pretrained(str(weights / "unet"), torch_dtype=dtype)
    unet = unet.to(device).eval()
    load_s = time.perf_counter() - t0
    print(f"ORACLE_UNET_LOAD_S {load_s:.3f} dtype=float16 device={device}")
    return unet, load_s


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--weights", type=Path, default=DEFAULT_WEIGHTS)
    parser.add_argument("--sizes", default="128,256")
    parser.add_argument("--views", type=int, default=6)
    parser.add_argument("--scheds", default="ddim,unipc")
    parser.add_argument("--include-512", action="store_true")
    args = parser.parse_args()

    print(f"ORACLE_PYTHON {sys.executable}")
    print(f"ORACLE_TORCH {torch.__version__} cuda={torch.cuda.is_available()}")
    print(f"ORACLE_WEIGHTS {args.weights}")
    print(f"ORACLE_DTYPE float16  # official DiffusionPipeline torch_dtype=float16")
    if not torch.cuda.is_available():
        print("ORACLE_SPEED_FAIL no CUDA")
        return 1
    print(f"ORACLE_GPU {torch.cuda.get_device_name(0)}")

    device = torch.device("cuda")
    dtype = torch.float16
    sizes = [int(s) for s in args.sizes.split(",") if s.strip()]
    if args.include_512 and 512 not in sizes:
        sizes.append(512)
    scheds = [s.strip() for s in args.scheds.split(",") if s.strip()]

    unet, load_s = load_unet(args.weights, device, dtype)
    rows = []
    for size in sizes:
        for kind in scheds:
            print(f"ORACLE_BEGIN size={size} views={args.views} sched={kind}")
            row = time_loop(unet, args.weights, size, args.views, kind, device, dtype)
            row["load_s"] = load_s
            rows.append(row)
            if torch.cuda.is_available():
                torch.cuda.empty_cache()

    print("ORACLE_SPEED_TABLE " + json.dumps(rows, sort_keys=True))
    print("ORACLE_SPEED_OK")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"ORACLE_SPEED_FAIL {type(exc).__name__}: {exc}")
        raise
