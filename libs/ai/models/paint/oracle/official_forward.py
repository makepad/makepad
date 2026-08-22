"""Official Hunyuan3D-Paint-2.1 oracle on a provisioned box.

Loads the local venv_paint + C:/ai/Hunyuan3D-2.1 weights (no Hub download)
and dumps VAE encode/decode plus an optional UNet2p5D first-block tap so the
native Rust executor can compare on the same box.

This is the reference, not a service fallback.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import traceback
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F

DEFAULT_ROOT = Path(os.environ.get("MAKEPAD_HUNYUAN_ROOT", r"C:\ai\Hunyuan3D-2.1"))
DEFAULT_WEIGHTS = DEFAULT_ROOT / "weights" / "hunyuan3d-paintpbr-v2-1"
DEFAULT_DINO = os.environ.get("MAKEPAD_DINO_PATH", "facebook/dinov2-giant")
PAINT_SRC = DEFAULT_ROOT / "hy3dpaint"


def sha256_f32(t: torch.Tensor) -> str:
    arr = t.detach().float().cpu().contiguous().numpy().astype("<f4", copy=False)
    return hashlib.sha256(arr.tobytes()).hexdigest()


def err_stats(actual: torch.Tensor, expected: torch.Tensor) -> dict:
    diff = (actual.float() - expected.float()).abs()
    denom = expected.float().abs().clamp_min(1e-12)
    return {
        "max_abs": float(diff.max().item()) if diff.numel() else 0.0,
        "max_rel": float((diff / denom).max().item()) if diff.numel() else 0.0,
        "mean_abs": float(diff.mean().item()) if diff.numel() else 0.0,
    }


def ramp_nchw(n: int, c: int, h: int, w: int, device: torch.device) -> torch.Tensor:
    """Deterministic [0,1] ramp used by both oracle and native."""
    ys = torch.linspace(0.0, 1.0, h, device=device).view(1, 1, h, 1).expand(1, 1, h, w)
    xs = torch.linspace(0.0, 1.0, w, device=device).view(1, 1, 1, w).expand(1, 1, h, w)
    ch0 = (xs + ys) * 0.5
    ch1 = xs
    ch2 = ys
    img = torch.cat([ch0, ch1, ch2], dim=1)
    if c != 3:
        img = img.repeat(1, (c + 2) // 3, 1, 1)[:, :c]
    return img.repeat(n, 1, 1, 1).contiguous()


def load_vae(weights: Path, device: torch.device, dtype: torch.dtype):
    from diffusers import AutoencoderKL

    t0 = time.perf_counter()
    vae = AutoencoderKL.from_pretrained(
        str(weights / "vae"),
        torch_dtype=dtype,
        use_safetensors=False,
        local_files_only=True,
    )
    vae = vae.to(device).eval()
    print(f"ORACLE_VAE_LOAD_S {time.perf_counter() - t0:.3f}")
    return vae


@torch.no_grad()
def dump_vae(vae, device: torch.device, size: int) -> dict:
    scale = float(getattr(vae.config, "scaling_factor", 0.18215))
    rgb = ramp_nchw(1, 3, size, size, device).to(vae.dtype)
    x = rgb * 2.0 - 1.0
    t0 = time.perf_counter()
    posterior = vae.encode(x).latent_dist
    mean = posterior.mean
    logvar = posterior.logvar
    # Deterministic: use mean, not sample. Native must do the same.
    latent = mean * scale
    encode_s = time.perf_counter() - t0
    t0 = time.perf_counter()
    recon = vae.decode(latent / scale).sample
    decode_s = time.perf_counter() - t0
    recon01 = (recon * 0.5 + 0.5).clamp(0, 1)
    report = {
        "size": size,
        "scale": scale,
        "encode_s": encode_s,
        "decode_s": decode_s,
        "latent_shape": list(latent.shape),
        "latent_digest": sha256_f32(latent),
        "mean_digest": sha256_f32(mean),
        "recon_digest": sha256_f32(recon01),
        "latent_head": latent.float().cpu().reshape(-1)[:16].tolist(),
        "latent_values": latent.float().cpu().reshape(-1).tolist(),
        "recon_head": recon01.float().cpu().reshape(-1)[:16].tolist(),
    }
    print(
        f"ORACLE_VAE size={size} encode_s={encode_s:.3f} decode_s={decode_s:.3f} "
        f"latent={tuple(latent.shape)} digest={report['latent_digest'][:16]}"
    )
    return report


def _tap(t: torch.Tensor, n: int = 32) -> dict:
    flat = t.detach().float().cpu().contiguous().reshape(-1)
    return {
        "shape": list(t.shape),
        "digest": sha256_f32(t),
        "head": flat[:n].tolist(),
    }


def _set_plain_attn_processor(inner):
    """Disable MDA extras so attn1 is a standard 3-D token self-attn."""
    try:
        from diffusers.models.attention_processor import AttnProcessor2_0

        inner.attn1.set_processor(AttnProcessor2_0())
    except Exception:
        from diffusers.models.attention_processor import AttnProcessor

        inner.attn1.set_processor(AttnProcessor())


@torch.no_grad()
def _plain_transformer_wrap(attn, hidden, encoder):
    """Transformer2D + inner BasicTransformerBlock with 2.5D extras off."""
    wrap = attn.transformer_blocks[0]
    inner = wrap.transformer
    saved = {
        "proc": inner.attn1.processor,
        "mda": wrap.use_mda,
        "ma": wrap.use_ma,
        "ra": wrap.use_ra,
        "dino": wrap.use_dino,
    }
    _set_plain_attn_processor(inner)
    wrap.use_mda = False
    wrap.use_ma = False
    wrap.use_ra = False
    wrap.use_dino = False
    residual = hidden
    gn = attn.norm(residual)
    b, c, hh, ww = gn.shape
    tok = attn.proj_in(gn.permute(0, 2, 3, 1).reshape(b, hh * ww, c))
    tok = inner(tok, encoder_hidden_states=encoder)
    out = attn.proj_out(tok).reshape(b, hh, ww, c).permute(0, 3, 1, 2).contiguous() + residual
    inner.attn1.set_processor(saved["proc"])
    wrap.use_mda = saved["mda"]
    wrap.use_ma = saved["ma"]
    wrap.use_ra = saved["ra"]
    wrap.use_dino = saved["dino"]
    return out, {
        "heads": int(inner.attn1.heads),
        "head_dim": int(inner.attn1.inner_dim // inner.attn1.heads),
        "norm_eps": float(attn.norm.eps),
    }


@torch.no_grad()
def dump_unet_stages(weights: Path, device: torch.device, dtype: torch.dtype, size: int) -> dict:
    sys.path.insert(0, str(PAINT_SRC))
    from hunyuanpaintpbr.unet.modules import UNet2p5DConditionModel

    t0 = time.perf_counter()
    unet = UNet2p5DConditionModel.from_pretrained(str(weights / "unet"), torch_dtype=dtype)
    unet = unet.to(device).eval()
    print(f"ORACLE_UNET_LOAD_S {time.perf_counter() - t0:.3f}")

    down0 = unet.unet.down_blocks[0]
    attn0 = down0.attentions[0]
    inner0 = attn0.transformer_blocks[0].transformer
    ff0 = inner0.ff
    downsampler = down0.downsamplers[0]
    meta = {
        "down0_type": type(down0).__name__,
        "resnet0_in": int(down0.resnets[0].norm1.num_channels),
        "resnet1_in": int(down0.resnets[1].norm1.num_channels),
        "downsample_type": type(downsampler).__name__,
        "downsample_padding": int(getattr(downsampler, "padding", -1)),
        "attn0_type": type(attn0).__name__,
        "attn0_norm_eps": float(attn0.norm.eps),
        "attn0_norm_groups": int(attn0.norm.num_groups),
        "attn0_use_linear_projection": bool(getattr(attn0, "use_linear_projection", True)),
        "inner_type": type(inner0).__name__,
        "inner_norm_type": str(getattr(inner0, "norm_type", "")),
        "inner_norm1_eps": float(inner0.norm1.eps),
        "inner_heads": int(inner0.attn1.heads),
        "inner_head_dim": int(inner0.attn1.inner_dim // inner0.attn1.heads),
        "inner_only_cross": bool(getattr(inner0, "only_cross_attention", False)),
        "ff_type": type(ff0).__name__,
        "ff0_type": type(ff0.net[0]).__name__,
        "attn1_processor": type(inner0.attn1.processor).__name__,
        "attn2_processor": type(inner0.attn2.processor).__name__,
        "wrap_type": type(attn0.transformer_blocks[0]).__name__,
        "wrap_use_mda": bool(getattr(attn0.transformer_blocks[0], "use_mda", False)),
        "wrap_use_ma": bool(getattr(attn0.transformer_blocks[0], "use_ma", False)),
        "wrap_use_ra": bool(getattr(attn0.transformer_blocks[0], "use_ra", False)),
        "wrap_use_dino": bool(getattr(attn0.transformer_blocks[0], "use_dino", False)),
        "learned_clip_shape": list(unet.unet.learned_text_clip_albedo.shape),
    }
    print("ORACLE_UNET_META " + json.dumps(meta, sort_keys=True))

    # 12-ch conv_in: 4 noise + 4 normal latent + 4 position latent
    h = size // 8
    noise = torch.arange(4 * h * h, device=device, dtype=dtype).reshape(1, 4, h, h) / float(4 * h * h)
    normal = noise * 0.25 + 0.1
    position = noise * 0.5 - 0.2
    x12 = torch.cat([noise, normal, position], dim=1)
    t0 = time.perf_counter()
    conv = unet.unet.conv_in(x12)
    conv_s = time.perf_counter() - t0
    report = {
        "in_shape": list(x12.shape),
        "out_shape": list(conv.shape),
        "conv_in_s": conv_s,
        "digest": sha256_f32(conv),
        "head": conv.float().cpu().reshape(-1)[:16].tolist(),
        "meta": meta,
    }
    print(f"ORACLE_CONV_IN {tuple(conv.shape)} s={conv_s:.4f} digest={report['digest'][:16]}")

    t = torch.tensor([999], device=device)
    t_emb = unet.unet.time_proj(t)
    t_emb = unet.unet.time_embedding(t_emb.to(dtype))
    report["temb_digest"] = sha256_f32(t_emb)
    report["temb_head"] = t_emb.float().cpu().reshape(-1)[:16].tolist()

    t0 = time.perf_counter()
    res0 = down0.resnets[0](conv, t_emb)
    report["resnet0_s"] = time.perf_counter() - t0
    report["resnet0_digest"] = sha256_f32(res0)
    report["resnet0_head"] = res0.float().cpu().reshape(-1)[:16].tolist()
    print(f"ORACLE_RESNET0 s={report['resnet0_s']:.4f} digest={report['resnet0_digest'][:16]}")

    t0 = time.perf_counter()
    res1 = down0.resnets[1](res0, t_emb)
    report["resnet1_s"] = time.perf_counter() - t0
    report["resnet1_digest"] = sha256_f32(res1)
    report["resnet1_head"] = res1.float().cpu().reshape(-1)[:32].tolist()
    print(f"ORACLE_RESNET1 s={report['resnet1_s']:.4f} digest={report['resnet1_digest'][:16]}")

    t0 = time.perf_counter()
    down = downsampler(res1)
    report["down_s"] = time.perf_counter() - t0
    report["down_digest"] = sha256_f32(down)
    report["down_head"] = down.float().cpu().reshape(-1)[:32].tolist()
    report["down_shape"] = list(down.shape)
    print(f"ORACLE_DOWN {tuple(down.shape)} s={report['down_s']:.4f} digest={report['down_digest'][:16]}")

    enc = unet.unet.learned_text_clip_albedo.unsqueeze(0).to(device=device, dtype=dtype)
    report["enc_digest"] = sha256_f32(enc)
    report["enc_head"] = enc.float().cpu().reshape(-1)[:16].tolist()

    wrap = attn0.transformer_blocks[0]
    saved = {
        "proc": inner0.attn1.processor,
        "mda": wrap.use_mda,
        "ma": wrap.use_ma,
        "ra": wrap.use_ra,
        "dino": wrap.use_dino,
    }
    _set_plain_attn_processor(inner0)
    wrap.use_mda = False
    wrap.use_ma = False
    wrap.use_ra = False
    wrap.use_dino = False

    residual = res0
    gn = attn0.norm(residual)
    report["attn0_gn"] = _tap(gn)
    b, c, hh, ww = gn.shape
    tok = gn.permute(0, 2, 3, 1).reshape(b, hh * ww, c)
    tok_in = attn0.proj_in(tok)
    report["attn0_proj_in"] = _tap(tok_in)

    n1 = inner0.norm1(tok_in)
    q = inner0.attn1.to_q(n1)
    k = inner0.attn1.to_k(n1)
    v = inner0.attn1.to_v(n1)
    report["attn0_q"] = _tap(q)
    heads = inner0.attn1.heads
    head_dim = q.shape[-1] // heads
    qh = q.view(b, -1, heads, head_dim).transpose(1, 2)
    kh = k.view(b, -1, heads, head_dim).transpose(1, 2)
    vh = v.view(b, -1, heads, head_dim).transpose(1, 2)
    attn = torch.nn.functional.scaled_dot_product_attention(qh, kh, vh, dropout_p=0.0, is_causal=False)
    attn = attn.transpose(1, 2).reshape(b, -1, heads * head_dim)
    attn1 = inner0.attn1.to_out[0](attn)
    h_attn1 = tok_in + attn1
    report["attn0_attn1"] = _tap(h_attn1)

    n2 = inner0.norm2(h_attn1)
    q2 = inner0.attn2.to_q(n2)
    k2 = inner0.attn2.to_k(enc)
    v2 = inner0.attn2.to_v(enc)
    q2h = q2.view(b, -1, heads, head_dim).transpose(1, 2)
    k2h = k2.view(b, -1, heads, head_dim).transpose(1, 2)
    v2h = v2.view(b, -1, heads, head_dim).transpose(1, 2)
    cross = torch.nn.functional.scaled_dot_product_attention(q2h, k2h, v2h, dropout_p=0.0, is_causal=False)
    cross = cross.transpose(1, 2).reshape(b, -1, heads * head_dim)
    attn2 = inner0.attn2.to_out[0](cross)
    h_attn2 = h_attn1 + attn2
    report["attn0_attn2"] = _tap(h_attn2)

    n3 = inner0.norm3(h_attn2)
    ff = inner0.ff(n3)
    h_ff = h_attn2 + ff
    report["attn0_ff"] = _tap(h_ff)
    report["attn0_ff_act"] = type(inner0.ff.net[0]).__name__

    inner_out = inner0(tok_in, encoder_hidden_states=enc)
    report["attn0_inner"] = _tap(inner_out)
    report["attn0_inner_vs_manual"] = err_stats(inner_out, h_ff)

    proj = attn0.proj_out(inner_out)
    wrap_out = proj.reshape(b, hh, ww, c).permute(0, 3, 1, 2).contiguous() + residual
    report["attn0_wrap"] = _tap(wrap_out)
    report["attn0_head"] = wrap_out.float().cpu().reshape(-1)[:32].tolist()
    report["attn0_digest"] = sha256_f32(wrap_out)
    print(f"ORACLE_ATTN0_WRAP {tuple(wrap_out.shape)} digest={report['attn0_digest'][:16]}")

    t0 = time.perf_counter()
    try:
        attn_mod = attn0(
            res0,
            encoder_hidden_states=enc,
            cross_attention_kwargs={"mode": "", "num_in_batch": 1},
        )
        if isinstance(attn_mod, tuple):
            attn_mod = attn_mod[0]
        report["attn0_module_s"] = time.perf_counter() - t0
        report["attn0_module"] = _tap(attn_mod)
        report["attn0_module_vs_wrap"] = err_stats(attn_mod, wrap_out)
        print(
            f"ORACLE_ATTN0 {tuple(wrap_out.shape)} digest={report['attn0_digest'][:16]} "
            f"module_vs_wrap={report['attn0_module_vs_wrap']['max_abs']:.3e} "
            f"inner_vs_manual={report['attn0_inner_vs_manual']['max_abs']:.3e}"
        )
    except Exception as e:
        report["attn0_module_error"] = repr(e)
        print(f"ORACLE_ATTN0_MODULE_FAIL {e}")
        print(
            f"ORACLE_ATTN0 {tuple(wrap_out.shape)} digest={report['attn0_digest'][:16]} "
            f"inner_vs_manual={report['attn0_inner_vs_manual']['max_abs']:.3e}"
        )

    inner0.attn1.set_processor(saved["proc"])
    wrap.use_mda = saved["mda"]
    wrap.use_ma = saved["ma"]
    wrap.use_ra = saved["ra"]
    wrap.use_dino = saved["dino"]

    attn1_out, attn1_meta = _plain_transformer_wrap(down0.attentions[1], res1, enc)
    report["attn1_head"] = attn1_out.float().cpu().reshape(-1)[:32].tolist()
    report["attn1_digest"] = sha256_f32(attn1_out)
    report["attn1_meta"] = attn1_meta
    print(f"ORACLE_ATTN1 {tuple(attn1_out.shape)} digest={report['attn1_digest'][:16]} {attn1_meta}")

    down1 = unet.unet.down_blocks[1]
    t0 = time.perf_counter()
    d1_res0 = down1.resnets[0](down, t_emb)
    report["d1_res0_s"] = time.perf_counter() - t0
    report["d1_res0_digest"] = sha256_f32(d1_res0)
    report["d1_res0_head"] = d1_res0.float().cpu().reshape(-1)[:32].tolist()
    report["d1_res0_shape"] = list(d1_res0.shape)
    print(f"ORACLE_D1_RES0 {tuple(d1_res0.shape)} digest={report['d1_res0_digest'][:16]}")

    t0 = time.perf_counter()
    d1_res1 = down1.resnets[1](d1_res0, t_emb)
    report["d1_res1_digest"] = sha256_f32(d1_res1)
    report["d1_res1_head"] = d1_res1.float().cpu().reshape(-1)[:32].tolist()
    print(f"ORACLE_D1_RES1 {tuple(d1_res1.shape)} digest={report['d1_res1_digest'][:16]}")

    d1_attn0, d1_attn0_meta = _plain_transformer_wrap(down1.attentions[0], d1_res0, enc)
    report["d1_attn0_head"] = d1_attn0.float().cpu().reshape(-1)[:32].tolist()
    report["d1_attn0_digest"] = sha256_f32(d1_attn0)
    report["d1_attn0_meta"] = d1_attn0_meta
    print(f"ORACLE_D1_ATTN0 {tuple(d1_attn0.shape)} digest={report['d1_attn0_digest'][:16]} {d1_attn0_meta}")

    d1_attn1, d1_attn1_meta = _plain_transformer_wrap(down1.attentions[1], d1_res1, enc)
    report["d1_attn1_head"] = d1_attn1.float().cpu().reshape(-1)[:32].tolist()
    report["d1_attn1_digest"] = sha256_f32(d1_attn1)
    report["d1_attn1_meta"] = d1_attn1_meta
    print(f"ORACLE_D1_ATTN1 {tuple(d1_attn1.shape)} digest={report['d1_attn1_digest'][:16]} {d1_attn1_meta}")

    t0 = time.perf_counter()
    d1_down = down1.downsamplers[0](d1_res1)
    report["d1_down_s"] = time.perf_counter() - t0
    report["d1_down_digest"] = sha256_f32(d1_down)
    report["d1_down_head"] = d1_down.float().cpu().reshape(-1)[:32].tolist()
    report["d1_down_shape"] = list(d1_down.shape)
    print(f"ORACLE_D1_DOWN {tuple(d1_down.shape)} digest={report['d1_down_digest'][:16]}")

    down2 = unet.unet.down_blocks[2]
    d2_res0 = down2.resnets[0](d1_down, t_emb)
    report["d2_res0_digest"] = sha256_f32(d2_res0)
    report["d2_res0_head"] = d2_res0.float().cpu().reshape(-1)[:32].tolist()
    report["d2_res0_shape"] = list(d2_res0.shape)
    print(f"ORACLE_D2_RES0 {tuple(d2_res0.shape)} digest={report['d2_res0_digest'][:16]}")

    d2_res1 = down2.resnets[1](d2_res0, t_emb)
    report["d2_res1_digest"] = sha256_f32(d2_res1)
    report["d2_res1_head"] = d2_res1.float().cpu().reshape(-1)[:32].tolist()
    print(f"ORACLE_D2_RES1 {tuple(d2_res1.shape)} digest={report['d2_res1_digest'][:16]}")

    d2_attn0, d2_attn0_meta = _plain_transformer_wrap(down2.attentions[0], d2_res0, enc)
    report["d2_attn0_head"] = d2_attn0.float().cpu().reshape(-1)[:32].tolist()
    report["d2_attn0_digest"] = sha256_f32(d2_attn0)
    report["d2_attn0_meta"] = d2_attn0_meta
    print(f"ORACLE_D2_ATTN0 {tuple(d2_attn0.shape)} digest={report['d2_attn0_digest'][:16]} {d2_attn0_meta}")

    d2_down = down2.downsamplers[0](d2_res1)
    report["d2_down_digest"] = sha256_f32(d2_down)
    report["d2_down_head"] = d2_down.float().cpu().reshape(-1)[:32].tolist()
    report["d2_down_shape"] = list(d2_down.shape)
    print(f"ORACLE_D2_DOWN {tuple(d2_down.shape)} digest={report['d2_down_digest'][:16]}")

    down3 = unet.unet.down_blocks[3]
    d3_res0 = down3.resnets[0](d2_down, t_emb)
    report["d3_res0_digest"] = sha256_f32(d3_res0)
    report["d3_res0_head"] = d3_res0.float().cpu().reshape(-1)[:32].tolist()
    report["d3_res0_shape"] = list(d3_res0.shape)
    print(f"ORACLE_D3_RES0 {tuple(d3_res0.shape)} digest={report['d3_res0_digest'][:16]}")

    d3_res1 = down3.resnets[1](d3_res0, t_emb)
    report["d3_res1_digest"] = sha256_f32(d3_res1)
    report["d3_res1_head"] = d3_res1.float().cpu().reshape(-1)[:32].tolist()
    print(f"ORACLE_D3_RES1 {tuple(d3_res1.shape)} digest={report['d3_res1_digest'][:16]}")

    mid = unet.unet.mid_block
    mid_res0 = mid.resnets[0](d3_res1, t_emb)
    report["mid_res0_digest"] = sha256_f32(mid_res0)
    report["mid_res0_head"] = mid_res0.float().cpu().reshape(-1)[:32].tolist()
    print(f"ORACLE_MID_RES0 {tuple(mid_res0.shape)} digest={report['mid_res0_digest'][:16]}")

    mid_attn, mid_attn_meta = _plain_transformer_wrap(mid.attentions[0], mid_res0, enc)
    report["mid_attn_head"] = mid_attn.float().cpu().reshape(-1)[:32].tolist()
    report["mid_attn_digest"] = sha256_f32(mid_attn)
    report["mid_attn_meta"] = mid_attn_meta
    print(f"ORACLE_MID_ATTN {tuple(mid_attn.shape)} digest={report['mid_attn_digest'][:16]} {mid_attn_meta}")

    mid_res1 = mid.resnets[1](mid_attn, t_emb)
    report["mid_res1_digest"] = sha256_f32(mid_res1)
    report["mid_res1_head"] = mid_res1.float().cpu().reshape(-1)[:32].tolist()
    print(f"ORACLE_MID_RES1 {tuple(mid_res1.shape)} digest={report['mid_res1_digest'][:16]}")

    module_acts = dump_module_chain(unet, conv, t_emb, enc, report)

    extra_acts = dump_extras_and_up(
        unet,
        {
            "conv": conv,
            "res0": res0,
            "res1": res1,
            "down": down,
            "d1_res0": d1_res0,
            "d1_res1": d1_res1,
            "d1_down": d1_down,
            "d2_res0": d2_res0,
            "d2_res1": d2_res1,
            "d2_down": d2_down,
            "d3_res0": d3_res0,
            "d3_res1": d3_res1,
            "mid_res0": mid_res0,
            "mid_res1": mid_res1,
        },
        t_emb,
        device,
        dtype,
        report,
    )

    acts_path = Path(os.environ.get(
        "PBR_UNET_ACTS",
        r"C:\Users\playe\makepad\local\pbrpaint\pbr_official_unet_acts.txt",
    ))
    acts = {
        "conv": conv,
        "res0": res0,
        "res1": res1,
        "down": down,
        "d1_res0": d1_res0,
        "d1_res1": d1_res1,
        "d1_down": d1_down,
        "d2_res0": d2_res0,
        "d2_res1": d2_res1,
        "d2_down": d2_down,
        "d3_res0": d3_res0,
        "d3_res1": d3_res1,
        "mid_res0": mid_res0,
        "mid_attn": mid_attn,
        "mid_res1": mid_res1,
    }
    acts.update(module_acts)
    acts.update(extra_acts)
    acts.update(dump_dual_write(unet, device, dtype, size, report))
    acts.update(dump_ddim_loop(unet, weights, device, dtype, size, report))
    with acts_path.open("w", encoding="utf-8") as f:
        for name, tensor in acts.items():
            flat = tensor.float().cpu().contiguous().reshape(-1)
            f.write(f"{name} {flat.numel()}")
            for v in flat.tolist():
                f.write(f" {v:.8e}")
            f.write("\n")
    report["acts_path"] = str(acts_path)
    print(f"ORACLE_ACTS {acts_path}")
    return report


WRITE_LAYERS = [
    "down_0_0_0",
    "down_0_1_0",
    "down_1_0_0",
    "down_1_1_0",
    "down_2_0_0",
    "down_2_1_0",
    "mid_0_0",
    "up_1_0_0",
    "up_1_1_0",
    "up_1_2_0",
    "up_2_0_0",
    "up_2_1_0",
    "up_2_2_0",
    "up_3_0_0",
    "up_3_1_0",
    "up_3_2_0",
]


@torch.no_grad()
def dump_dual_write(unet, device, dtype, size, report):
    """Official unet_dual write at t=0, mode=w, 2-view 4-ch ref latents."""
    dual = getattr(unet, "unet_dual", None)
    if dual is None:
        print("ORACLE_DUAL_MISSING")
        return {}
    h = size // 8
    n4 = 4 * h * h
    v0 = torch.arange(n4, device=device, dtype=dtype).reshape(1, 4, h, h) / float(n4)
    v1 = v0 * 0.8 + 0.02
    ref = torch.cat([v0, v1], dim=0)
    enc = unet.unet.learned_text_clip_ref.unsqueeze(0).to(device=device, dtype=dtype).repeat(2, 1, 1)
    acts = {}

    def tap(name, tensor):
        report[f"{name}_head"] = tensor.float().cpu().reshape(-1)[:32].tolist()
        report[f"{name}_digest"] = sha256_f32(tensor)
        report[f"{name}_shape"] = list(tensor.shape)
        print(f"ORACLE_{name.upper()} {tuple(tensor.shape)} digest={report[f'{name}_digest'][:16]}")
        acts[name] = tensor

    conv = dual.conv_in(ref)
    tap("dual_conv", conv)
    t0 = dual.time_proj(torch.tensor([0], device=device)).to(dtype)
    temb = dual.time_embedding(t0)
    tap("dual_temb", temb)
    report["dual_conv_in_shape"] = list(dual.conv_in.weight.shape)
    print(f"ORACLE_DUAL_CONV_IN_W {report['dual_conv_in_shape']}")

    cond = {}
    try:
        dual(
            ref,
            0,
            encoder_hidden_states=enc,
            return_dict=False,
            cross_attention_kwargs={
                "mode": "w",
                "num_in_batch": 2,
                "condition_embed_dict": cond,
            },
        )
    except Exception as e:
        print(f"ORACLE_DUAL_FWD_FAIL {type(e).__name__}: {e}")
        traceback.print_exc()
        return acts

    report["dual_write_keys"] = sorted(str(k) for k in cond.keys())
    print(f"ORACLE_DUAL_WRITE_KEYS {report['dual_write_keys']}")
    for name in WRITE_LAYERS:
        if name not in cond:
            print(f"ORACLE_DUAL_MISSING_LAYER {name}")
            continue
        tap(f"dual_{name}", cond[name])
    return acts


def _ddim_rows(device, dtype, h):
    """Deterministic 2-view x 2-material 4-ch rows + per-view normal/position."""
    n4 = 4 * h * h
    base = torch.arange(n4, device=device, dtype=dtype).reshape(1, 4, h, h) / float(n4)
    alb0, alb1 = base, base * 0.8 + 0.02
    mr0 = base * 0.7 + 0.05
    mr1 = mr0 * 0.8 + 0.02
    n0 = base * 0.25 + 0.1
    n1 = n0 * 0.8 + 0.02
    p0 = base * 0.5 - 0.2
    p1 = p0 * 0.8 + 0.02
    noises = [alb0, alb1, mr0, mr1]
    normals = [n0, n1, n0, n1]
    positions = [p0, p1, p0, p1]
    x12s = [torch.cat([noises[i], normals[i], positions[i]], dim=1) for i in range(4)]
    return noises, normals, positions, x12s


def _guidance_combine_nchw(uncond, ref_only, full, guidance, view_scales):
    out = []
    for i, vs in enumerate(view_scales):
        a = guidance * vs
        u, r, f = uncond[i], ref_only[i], full[i]
        out.append(u + a * (r - u) + a * (f - r))
    return torch.stack(out, dim=0)


@torch.no_grad()
def dump_ddim_loop(unet, weights, device, dtype, size, report):
    """15-step DDIM / v-pred / ZSNR / trailing on the deterministic 8x8 pack."""
    from diffusers import DDIMScheduler

    dual = getattr(unet, "unet_dual", None)
    if dual is None:
        print("ORACLE_DDIM_SKIP no unet_dual")
        return {}
    h = size // 8
    noises, normals, positions, _x12s = _ddim_rows(device, dtype, h)
    sample = torch.cat(noises, dim=0)
    enc_alb = unet.unet.learned_text_clip_albedo.unsqueeze(0).to(device=device, dtype=dtype)
    enc_mr = unet.unet.learned_text_clip_mr.unsqueeze(0).to(device=device, dtype=dtype)
    enc4 = torch.cat([enc_alb, enc_alb, enc_mr, enc_mr], dim=0)
    enc0 = torch.zeros_like(enc4)
    dino_raw = torch.arange(1536, device=device, dtype=dtype).reshape(1, 1536) / 1536.0
    dino = unet.unet.image_proj_model_dino(dino_raw)
    dino0 = torch.zeros_like(dino)
    from hunyuanpaintpbr.unet.modules import calc_multires_voxel_idxs

    pos = torch.zeros(1, 2, 3, h, h, device=device, dtype=dtype)
    ys = torch.linspace(0.1, 0.9, h, device=device, dtype=dtype).view(1, 1, 1, h, 1)
    xs = torch.linspace(0.1, 0.9, h, device=device, dtype=dtype).view(1, 1, 1, 1, h)
    pos[0, 0, 0] = xs
    pos[0, 0, 1] = ys
    pos[0, 0, 2] = 0.35
    pos[0, 1, 0] = 1.0 - xs
    pos[0, 1, 1] = ys
    pos[0, 1, 2] = 0.65
    voxels = calc_multires_voxel_idxs(pos, grid_resolutions=[8, 4, 2, 1], voxel_resolutions=[64, 32, 16, 8])

    n4 = 4 * h * h
    ref = torch.cat(
        [
            torch.arange(n4, device=device, dtype=dtype).reshape(1, 4, h, h) / float(n4),
        ],
        dim=0,
    )
    ref = torch.cat([ref, ref * 0.8 + 0.02], dim=0)
    enc_ref = unet.unet.learned_text_clip_ref.unsqueeze(0).to(device=device, dtype=dtype).repeat(2, 1, 1)
    cond = {}
    dual(
        ref,
        0,
        encoder_hidden_states=enc_ref,
        return_dict=False,
        cross_attention_kwargs={"mode": "w", "num_in_batch": 2, "condition_embed_dict": cond},
    )

    states = _attn_states(unet)
    acts = {}

    def tap(name, tensor):
        report[f"{name}_head"] = tensor.float().cpu().reshape(-1)[:32].tolist()
        report[f"{name}_digest"] = sha256_f32(tensor)
        report[f"{name}_shape"] = list(tensor.shape)
        print(f"ORACLE_{name.upper()} {tuple(tensor.shape)} digest={report[f'{name}_digest'][:16]}")
        acts[name] = tensor

    tap("ddim_x0", sample)
    tap("ddim_dino", dino)

    def unet_v(x4, t, enc, dino_tok, ref_scale):
        x12 = torch.cat([torch.cat([x4[i : i + 1], normals[i], positions[i]], dim=1) for i in range(4)], dim=0)
        return unet.unet(
            x12,
            t,
            encoder_hidden_states=enc,
            return_dict=False,
            cross_attention_kwargs={
                "mode": "r",
                "num_in_batch": 2,
                "condition_embed_dict": cond,
                "dino_hidden_states": dino_tok,
                "position_voxel_indices": voxels,
                "ref_scale": ref_scale,
                "mva_scale": 1.0,
            },
        )[0]

    try:
        _apply_read_extras(states)
        t0 = 999
        v_full = unet_v(sample, t0, enc4, dino, 1.0)
        tap("ddim_v0", v_full)
        v_uncond = unet_v(sample, t0, enc0, dino0, 0.0)
        v_ref = unet_v(sample, t0, enc4, dino0, 1.0)
        tap("ddim_v0_uncond", v_uncond)
        tap("ddim_v0_ref", v_ref)
        view_scales = [1.0, 2.0, 1.0, 2.0]
        guided = _guidance_combine_nchw(v_uncond, v_ref, v_full, 3.0, view_scales)
        tap("ddim_v0_guided", guided)

        sched = DDIMScheduler.from_pretrained(str(Path(weights) / "scheduler"))
        sched.set_timesteps(15, device=device)
        report["ddim_timesteps"] = [int(t) for t in sched.timesteps.detach().cpu().tolist()]
        print(f"ORACLE_DDIM_TIMESTEPS {report['ddim_timesteps']}")

        xt = sample
        for i, t in enumerate(sched.timesteps):
            v_u = unet_v(xt, t, enc0, dino0, 0.0)
            v_r = unet_v(xt, t, enc4, dino0, 1.0)
            v_f = unet_v(xt, t, enc4, dino, 1.0)
            vg = _guidance_combine_nchw(v_u, v_r, v_f, 3.0, view_scales)
            xt = sched.step(vg, t, xt, return_dict=True).prev_sample
            if i in (0, 7, 14):
                tap(f"ddim_x{i+1}", xt)
                if i == 0:
                    tap("ddim_v0_step", vg)
    except Exception as e:
        print(f"ORACLE_DDIM_FAIL {type(e).__name__}: {e}")
        traceback.print_exc()
    finally:
        _extras_restore(states)
    return acts


def _unwrap_mod(out):
    if isinstance(out, tuple):
        out = out[0]
    return out.sample if hasattr(out, "sample") else out


def _iter_transformer_attns(unet):
    for block in list(unet.unet.down_blocks) + [unet.unet.mid_block] + list(unet.unet.up_blocks):
        for attn in getattr(block, "attentions", None) or []:
            yield attn


def _extras_off_all(unet):
    saved = []
    for attn in _iter_transformer_attns(unet):
        wrap = attn.transformer_blocks[0]
        inner = wrap.transformer
        saved.append((inner, wrap, inner.attn1.processor, wrap.use_mda, wrap.use_ma, wrap.use_ra, wrap.use_dino))
        _set_plain_attn_processor(inner)
        wrap.use_mda = wrap.use_ma = wrap.use_ra = wrap.use_dino = False
    return saved


def _extras_restore(saved):
    for inner, wrap, proc, mda, ma, ra, dino in saved:
        inner.attn1.set_processor(proc)
        wrap.use_mda = mda
        wrap.use_ma = ma
        wrap.use_ra = ra
        wrap.use_dino = dino


def _plain_kwargs():
    return {"mode": "", "num_in_batch": 1}


def _call_attn(attn, hidden, enc, kwargs):
    try:
        out = attn(
            hidden,
            encoder_hidden_states=enc,
            cross_attention_kwargs=kwargs,
            return_dict=False,
        )
    except TypeError:
        out = attn(hidden, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
    return _unwrap_mod(out)


@torch.no_grad()
def dump_module_chain(unet, conv, t_emb, enc, report):
    """Extras-off walk that matches `down_blocks[i](...)` / `up_blocks[i](...)`."""
    saved = _extras_off_all(unet)
    kwargs = _plain_kwargs()
    acts = {}

    def tap(name, tensor):
        report[f"{name}_head"] = tensor.float().cpu().reshape(-1)[:32].tolist()
        report[f"{name}_digest"] = sha256_f32(tensor)
        report[f"{name}_shape"] = list(tensor.shape)
        print(f"ORACLE_{name.upper()} {tuple(tensor.shape)} digest={report[f'{name}_digest'][:16]}")
        acts[name] = tensor

    def call_down(idx, hidden):
        block = unet.unet.down_blocks[idx]
        if getattr(block, "attentions", None):
            out, skips = block(
                hidden,
                t_emb,
                encoder_hidden_states=enc,
                cross_attention_kwargs=kwargs,
            )
        else:
            out, skips = block(hidden, t_emb)
        return out, skips

    try:
        h = conv
        # down0: resnet→attn pairs, then downsample
        h = unet.unet.down_blocks[0].resnets[0](h, t_emb)
        tap("mod_d0r0", h)
        h = _call_attn(unet.unet.down_blocks[0].attentions[0], h, enc, kwargs)
        tap("mod_d0a0", h)
        h = unet.unet.down_blocks[0].resnets[1](h, t_emb)
        tap("mod_d0r1", h)
        h = _call_attn(unet.unet.down_blocks[0].attentions[1], h, enc, kwargs)
        tap("mod_d0a1", h)
        h = unet.unet.down_blocks[0].downsamplers[0](h)
        tap("mod_d0out", h)
        try:
            blk, blk_skips = call_down(0, conv)
            report["mod_d0_vs_block"] = err_stats(h, blk)
            print(f"ORACLE_MOD_D0_VS_BLOCK max_abs={report['mod_d0_vs_block']['max_abs']:.3e} nskip={len(blk_skips)}")
        except Exception as e:
            print(f"ORACLE_MOD_D0_BLOCK_FAIL {type(e).__name__}: {e}")

        h = unet.unet.down_blocks[1].resnets[0](h, t_emb)
        tap("mod_d1r0", h)
        h = _call_attn(unet.unet.down_blocks[1].attentions[0], h, enc, kwargs)
        tap("mod_d1a0", h)
        h = unet.unet.down_blocks[1].resnets[1](h, t_emb)
        tap("mod_d1r1", h)
        h = _call_attn(unet.unet.down_blocks[1].attentions[1], h, enc, kwargs)
        tap("mod_d1a1", h)
        h = unet.unet.down_blocks[1].downsamplers[0](h)
        tap("mod_d1out", h)
        try:
            blk, blk_skips = call_down(1, acts["mod_d0out"])
            report["mod_d1_vs_block"] = err_stats(h, blk)
            print(f"ORACLE_MOD_D1_VS_BLOCK max_abs={report['mod_d1_vs_block']['max_abs']:.3e} nskip={len(blk_skips)}")
        except Exception as e:
            print(f"ORACLE_MOD_D1_BLOCK_FAIL {type(e).__name__}: {e}")

        h = unet.unet.down_blocks[2].resnets[0](h, t_emb)
        tap("mod_d2r0", h)
        h = _call_attn(unet.unet.down_blocks[2].attentions[0], h, enc, kwargs)
        tap("mod_d2a0", h)
        h = unet.unet.down_blocks[2].resnets[1](h, t_emb)
        tap("mod_d2r1", h)
        h = _call_attn(unet.unet.down_blocks[2].attentions[1], h, enc, kwargs)
        tap("mod_d2a1", h)
        h = unet.unet.down_blocks[2].downsamplers[0](h)
        tap("mod_d2out", h)
        try:
            blk, _ = call_down(2, acts["mod_d1out"])
            report["mod_d2_vs_block"] = err_stats(h, blk)
            print(f"ORACLE_MOD_D2_VS_BLOCK max_abs={report['mod_d2_vs_block']['max_abs']:.3e}")
        except Exception as e:
            print(f"ORACLE_MOD_D2_BLOCK_FAIL {type(e).__name__}: {e}")

        h = unet.unet.down_blocks[3].resnets[0](h, t_emb)
        tap("mod_d3r0", h)
        h = unet.unet.down_blocks[3].resnets[1](h, t_emb)
        tap("mod_d3r1", h)
        try:
            blk, _ = call_down(3, acts["mod_d2out"])
            report["mod_d3_vs_block"] = err_stats(h, blk)
            print(f"ORACLE_MOD_D3_VS_BLOCK max_abs={report['mod_d3_vs_block']['max_abs']:.3e}")
        except Exception as e:
            print(f"ORACLE_MOD_D3_BLOCK_FAIL {type(e).__name__}: {e}")

        mid = unet.unet.mid_block
        h = mid.resnets[0](h, t_emb)
        tap("mod_midr0", h)
        h = _call_attn(mid.attentions[0], h, enc, kwargs)
        tap("mod_mida", h)
        h = mid.resnets[1](h, t_emb)
        tap("mod_midr1", h)
        try:
            blk = mid(acts["mod_d3r1"], t_emb, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
            report["mod_mid_vs_block"] = err_stats(h, blk)
            print(f"ORACLE_MOD_MID_VS_BLOCK max_abs={report['mod_mid_vs_block']['max_abs']:.3e}")
        except Exception as e:
            print(f"ORACLE_MOD_MID_BLOCK_FAIL {type(e).__name__}: {e}")

        # Official UNet skip tuple: conv_in + each down residual (after attn, then downsample).
        skips = [
            conv,
            acts["mod_d0a0"],
            acts["mod_d0a1"],
            acts["mod_d0out"],
            acts["mod_d1a0"],
            acts["mod_d1a1"],
            acts["mod_d1out"],
            acts["mod_d2a0"],
            acts["mod_d2a1"],
            acts["mod_d2out"],
            acts["mod_d3r0"],
            acts["mod_d3r1"],
        ]

        def pop_n(n):
            chunk = tuple(skips[-n:])
            del skips[-n:]
            return chunk

        def zip_up(block, hidden, res_tuple, with_attn):
            res_list = list(res_tuple)
            for i, resnet in enumerate(block.resnets):
                skip = res_list.pop()
                hidden = resnet(torch.cat([hidden, skip], dim=1), t_emb)
                if with_attn:
                    hidden = _call_attn(block.attentions[i], hidden, enc, kwargs)
            if getattr(block, "upsamplers", None):
                hidden = block.upsamplers[0](hidden)
            return hidden

        up0 = unet.unet.up_blocks[0]
        res = pop_n(len(up0.resnets))
        try:
            h = up0(h, res, t_emb)
        except Exception as e:
            print(f"ORACLE_MOD_UP0_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up0, h, res, False)
        tap("mod_up0", h)

        up1 = unet.unet.up_blocks[1]
        res = pop_n(len(up1.resnets))
        try:
            blk = up1(h, res, t_emb, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
            h = blk
        except Exception as e:
            print(f"ORACLE_MOD_UP1_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up1, h, res, True)
        tap("mod_up1", h)

        up2 = unet.unet.up_blocks[2]
        res = pop_n(len(up2.resnets))
        try:
            blk = up2(h, res, t_emb, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
            h = blk
        except Exception as e:
            print(f"ORACLE_MOD_UP2_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up2, h, res, True)
        tap("mod_up2", h)

        up3 = unet.unet.up_blocks[3]
        res = pop_n(len(up3.resnets))
        # Intermediates for up3 (last, no upsample): after each resnet+attn pair.
        u = acts["mod_up2"]
        for i, (resnet, attn) in enumerate(zip(up3.resnets, up3.attentions)):
            skip = res[len(res) - 1 - i]
            u = resnet(torch.cat([u, skip], dim=1), t_emb)
            u = _call_attn(attn, u, enc, kwargs)
            tap(f"mod_u3a{i}", u)
        try:
            blk = up3(acts["mod_up2"], res, t_emb, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
            report["mod_u3_vs_block"] = err_stats(u, blk)
            print(f"ORACLE_MOD_U3_VS_BLOCK max_abs={report['mod_u3_vs_block']['max_abs']:.3e}")
            h = blk
        except Exception as e:
            print(f"ORACLE_MOD_U3_BLOCK_FAIL {type(e).__name__}: {e}")
            h = u
        tap("mod_u3out", h)

        head = unet.unet.conv_out(torch.nn.functional.silu(unet.unet.conv_norm_out(h)))
        tap("mod_head", head)
        if skips:
            print(f"ORACLE_MOD_SKIP_LEFTOVER {len(skips)}")
    finally:
        _extras_restore(saved)
    return acts



def _cat4(xs):
    return torch.cat(xs, dim=0)


def _split4(t4):
    return list(t4.chunk(4, dim=0))


def _affine4(x):
    """2-view x 2-material pack from one NCHW map (matches existing extras taps)."""
    v0 = x
    v1 = x * 0.8 + 0.02
    mr0 = x * 0.7 + 0.05
    mr1 = mr0 * 0.8 + 0.02
    return [v0, v1, mr0, mr1]


def _write2(x0, x1):
    return torch.cat([x0 * 0.5 + 0.1, x1 * 0.5 + 0.1], dim=0)


def _extras_attn(attn, h4, enc4, dino, voxels, write_h, write_enc):
    wrap = attn.transformer_blocks[0]
    inner = wrap.transformer
    saved = {
        "proc": inner.attn1.processor,
        "mda": wrap.use_mda,
        "ma": wrap.use_ma,
        "ra": wrap.use_ra,
        "dino": wrap.use_dino,
    }
    # Write stores RA cache only (2 albedo views). MA/DINO need the 4-wide
    # (n_pbr=2, n_views=2) read batch and would einops-fail here.
    wrap.use_ra = True
    wrap.use_ma = wrap.use_dino = wrap.use_mda = False
    _set_plain_attn_processor(inner)
    cond = {}
    _unwrap_mod(
        attn(
            write_h,
            encoder_hidden_states=write_enc,
            cross_attention_kwargs={
                "mode": "w",
                "num_in_batch": 2,
                "condition_embed_dict": cond,
            },
        )
    )
    wrap.use_mda = wrap.use_ma = wrap.use_ra = wrap.use_dino = True
    inner.attn1.set_processor(saved["proc"])
    out = _unwrap_mod(
        attn(
            h4,
            encoder_hidden_states=enc4,
            cross_attention_kwargs={
                "mode": "r",
                "num_in_batch": 2,
                "condition_embed_dict": cond,
                "dino_hidden_states": dino,
                "position_voxel_indices": voxels,
                "ref_scale": 1.0,
                "mva_scale": 1.0,
            },
        )
    )
    inner.attn1.set_processor(saved["proc"])
    wrap.use_mda = saved["mda"]
    wrap.use_ma = saved["ma"]
    wrap.use_ra = saved["ra"]
    wrap.use_dino = saved["dino"]
    return out


def _map_resnets(resnets, xs, t_emb, idx):
    return [resnets[idx](x, t_emb) for x in xs]


def _map_down(down, xs):
    return [down(x) for x in xs]


def _map_up(up, xs):
    return [up(x) for x in xs]


@torch.no_grad()
def dump_extras_and_up(unet, t, t_emb, device, dtype, report):
    res0 = t["res0"]
    mid_res1 = t["mid_res1"]
    d3_res1 = t["d3_res1"]
    d3_res0 = t["d3_res0"]
    d2_down = t["d2_down"]
    from hunyuanpaintpbr.unet.modules import calc_multires_voxel_idxs

    attn0 = unet.unet.down_blocks[0].attentions[0]
    wrap = attn0.transformer_blocks[0]
    inner = wrap.transformer
    saved = {
        "proc": inner.attn1.processor,
        "mda": wrap.use_mda,
        "ma": wrap.use_ma,
        "ra": wrap.use_ra,
        "dino": wrap.use_dino,
    }
    enc_alb = unet.unet.learned_text_clip_albedo.unsqueeze(0).to(device=device, dtype=dtype)
    enc_mr = unet.unet.learned_text_clip_mr.unsqueeze(0).to(device=device, dtype=dtype)
    enc2 = torch.cat([enc_alb, enc_mr], dim=0)
    mr_in = res0 * 0.7 + 0.05
    h2 = torch.cat([res0, mr_in], dim=0)
    dino_raw = torch.arange(1536, device=device, dtype=dtype).reshape(1, 1536) / 1536.0
    dino_tok = unet.unet.image_proj_model_dino(dino_raw)
    report["dino_proj_head"] = dino_tok.float().cpu().reshape(-1)[:32].tolist()
    report["dino_proj_digest"] = sha256_f32(dino_tok)
    report["enc_mr_digest"] = sha256_f32(enc_mr)
    print(f"ORACLE_DINO_PROJ {tuple(dino_tok.shape)} digest={report['dino_proj_digest'][:16]}")

    def run_attn(hidden, enc, kwargs, mda, dino, ra, ma):
        wrap.use_mda = mda
        wrap.use_dino = dino
        wrap.use_ra = ra
        wrap.use_ma = ma
        if mda:
            inner.attn1.set_processor(saved["proc"])
        else:
            _set_plain_attn_processor(inner)
        out = attn0(hidden, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
        return _unwrap_mod(out)

    mda = run_attn(h2, enc2, {"mode": "", "num_in_batch": 1}, True, False, False, False)
    report["mda_head"] = mda.float().cpu().reshape(-1)[:32].tolist()
    report["mda_digest"] = sha256_f32(mda)
    print(f"ORACLE_MDA {tuple(mda.shape)} digest={report['mda_digest'][:16]}")

    dino = run_attn(
        h2,
        enc2,
        {"mode": "", "num_in_batch": 1, "dino_hidden_states": dino_tok},
        False,
        True,
        False,
        False,
    )
    report["dino_head"] = dino.float().cpu().reshape(-1)[:32].tolist()
    report["dino_digest"] = sha256_f32(dino)
    print(f"ORACLE_DINO {tuple(dino.shape)} digest={report['dino_digest'][:16]}")

    mda_dino = run_attn(
        h2,
        enc2,
        {"mode": "", "num_in_batch": 1, "dino_hidden_states": dino_tok},
        True,
        True,
        False,
        False,
    )
    report["mda_dino_head"] = mda_dino.float().cpu().reshape(-1)[:32].tolist()
    report["mda_dino_digest"] = sha256_f32(mda_dino)
    print(f"ORACLE_MDA_DINO {tuple(mda_dino.shape)} digest={report['mda_dino_digest'][:16]}")

    cond = {}
    ref = res0 * 0.5 + 0.1
    run_attn(ref, enc_alb, {"mode": "w", "num_in_batch": 1, "condition_embed_dict": cond}, False, False, True, False)
    ref_out = run_attn(
        h2,
        enc2,
        {"mode": "r", "num_in_batch": 1, "condition_embed_dict": cond, "ref_scale": 1.0},
        False,
        False,
        True,
        False,
    )
    report["ref_head"] = ref_out.float().cpu().reshape(-1)[:32].tolist()
    report["ref_digest"] = sha256_f32(ref_out)
    print(f"ORACLE_REF {tuple(ref_out.shape)} digest={report['ref_digest'][:16]} cond_keys={list(cond)}")

    # 2 views x 2 materials = batch 4. Views: res0 and res0*0.8+0.02
    v1 = res0
    v2 = res0 * 0.8 + 0.02
    h4 = torch.cat([v1, v2, mr_in, mr_in * 0.8 + 0.02], dim=0)
    enc4 = torch.cat([enc_alb, enc_alb, enc_mr, enc_mr], dim=0)
    pos = torch.zeros(1, 2, 3, 8, 8, device=device, dtype=dtype)
    ys = torch.linspace(0.1, 0.9, 8, device=device, dtype=dtype).view(1, 1, 1, 8, 1)
    xs = torch.linspace(0.1, 0.9, 8, device=device, dtype=dtype).view(1, 1, 1, 1, 8)
    pos[0, 0, 0] = xs
    pos[0, 0, 1] = ys
    pos[0, 0, 2] = 0.35
    pos[0, 1, 0] = 1.0 - xs
    pos[0, 1, 1] = ys
    pos[0, 1, 2] = 0.65
    voxels = calc_multires_voxel_idxs(pos, grid_resolutions=[8, 4, 2, 1], voxel_resolutions=[64, 32, 16, 8])
    report["ma_voxel_keys"] = [int(k) for k in voxels.keys()]
    vox128 = voxels[128]
    report["ma_voxel_res"] = int(vox128["voxel_resolution"])
    vox_idx = vox128["voxel_indices"].float()
    ma = run_attn(
        h4,
        enc4,
        {"mode": "", "num_in_batch": 2, "position_voxel_indices": voxels, "mva_scale": 1.0},
        False,
        False,
        False,
        True,
    )
    report["ma_head"] = ma.float().cpu().reshape(-1)[:32].tolist()
    report["ma_digest"] = sha256_f32(ma)
    print(f"ORACLE_MA {tuple(ma.shape)} digest={report['ma_digest'][:16]} voxels={report['ma_voxel_keys']}")

    # Combined extras, 2 views x 2 materials, with dino + ref
    cond2 = {}
    run_attn(
        torch.cat([ref, ref * 0.9], dim=0),
        torch.cat([enc_alb, enc_alb], dim=0),
        {"mode": "w", "num_in_batch": 2, "condition_embed_dict": cond2},
        False,
        False,
        True,
        False,
    )
    full = run_attn(
        h4,
        enc4,
        {
            "mode": "r",
            "num_in_batch": 2,
            "condition_embed_dict": cond2,
            "ref_scale": 1.0,
            "mva_scale": 1.0,
            "position_voxel_indices": voxels,
            "dino_hidden_states": dino_tok,
        },
        True,
        True,
        True,
        True,
    )
    report["full25_head"] = full.float().cpu().reshape(-1)[:32].tolist()
    report["full25_digest"] = sha256_f32(full)
    print(f"ORACLE_FULL25 {tuple(full.shape)} digest={report['full25_digest'][:16]}")

    inner.attn1.set_processor(saved["proc"])
    wrap.use_mda = saved["mda"]
    wrap.use_ma = saved["ma"]
    wrap.use_ra = saved["ra"]
    wrap.use_dino = saved["dino"]

    up0 = unet.unet.up_blocks[0]
    cat0 = torch.cat([mid_res1, d3_res1], dim=1)
    up0_r0 = up0.resnets[0](cat0, t_emb)
    report["up0_res0_head"] = up0_r0.float().cpu().reshape(-1)[:32].tolist()
    report["up0_res0_digest"] = sha256_f32(up0_r0)
    report["up0_res0_shape"] = list(up0_r0.shape)
    print(f"ORACLE_UP0_RES0 {tuple(up0_r0.shape)} digest={report['up0_res0_digest'][:16]}")

    cat1 = torch.cat([up0_r0, d3_res0], dim=1)
    up0_r1 = up0.resnets[1](cat1, t_emb)
    report["up0_res1_head"] = up0_r1.float().cpu().reshape(-1)[:32].tolist()
    report["up0_res1_digest"] = sha256_f32(up0_r1)
    print(f"ORACLE_UP0_RES1 {tuple(up0_r1.shape)} digest={report['up0_res1_digest'][:16]}")

    cat2 = torch.cat([up0_r1, d2_down], dim=1)
    up0_r2 = up0.resnets[2](cat2, t_emb)
    report["up0_res2_head"] = up0_r2.float().cpu().reshape(-1)[:32].tolist()
    report["up0_res2_digest"] = sha256_f32(up0_r2)
    print(f"ORACLE_UP0_RES2 {tuple(up0_r2.shape)} digest={report['up0_res2_digest'][:16]}")

    up0_up = up0.upsamplers[0](up0_r2)
    report["up0_up_head"] = up0_up.float().cpu().reshape(-1)[:32].tolist()
    report["up0_up_digest"] = sha256_f32(up0_up)
    report["up0_up_shape"] = list(up0_up.shape)
    print(f"ORACLE_UP0_UP {tuple(up0_up.shape)} digest={report['up0_up_digest'][:16]}")

    enc = enc_alb
    up1 = unet.unet.up_blocks[1]
    up1_r0 = up1.resnets[0](torch.cat([up0_up, t["d2_res1"]], dim=1), t_emb)
    report["up1_res0_head"] = up1_r0.float().cpu().reshape(-1)[:32].tolist()
    report["up1_res0_digest"] = sha256_f32(up1_r0)
    print(f"ORACLE_UP1_RES0 {tuple(up1_r0.shape)} digest={report['up1_res0_digest'][:16]}")
    up1_a0, up1_a0_meta = _plain_transformer_wrap(up1.attentions[0], up1_r0, enc)
    report["up1_attn0_head"] = up1_a0.float().cpu().reshape(-1)[:32].tolist()
    report["up1_attn0_digest"] = sha256_f32(up1_a0)
    report["up1_attn0_meta"] = up1_a0_meta
    print(f"ORACLE_UP1_ATTN0 {tuple(up1_a0.shape)} digest={report['up1_attn0_digest'][:16]} {up1_a0_meta}")
    up1_r1 = up1.resnets[1](torch.cat([up1_a0, t["d2_res0"]], dim=1), t_emb)
    report["up1_res1_head"] = up1_r1.float().cpu().reshape(-1)[:32].tolist()
    report["up1_res1_digest"] = sha256_f32(up1_r1)
    print(f"ORACLE_UP1_RES1 {tuple(up1_r1.shape)} digest={report['up1_res1_digest'][:16]}")
    up1_a1, _ = _plain_transformer_wrap(up1.attentions[1], up1_r1, enc)
    report["up1_attn1_head"] = up1_a1.float().cpu().reshape(-1)[:32].tolist()
    report["up1_attn1_digest"] = sha256_f32(up1_a1)
    print(f"ORACLE_UP1_ATTN1 {tuple(up1_a1.shape)} digest={report['up1_attn1_digest'][:16]}")
    up1_r2 = up1.resnets[2](torch.cat([up1_a1, t["d1_down"]], dim=1), t_emb)
    report["up1_res2_head"] = up1_r2.float().cpu().reshape(-1)[:32].tolist()
    report["up1_res2_digest"] = sha256_f32(up1_r2)
    print(f"ORACLE_UP1_RES2 {tuple(up1_r2.shape)} digest={report['up1_res2_digest'][:16]}")
    up1_a2, _ = _plain_transformer_wrap(up1.attentions[2], up1_r2, enc)
    report["up1_attn2_head"] = up1_a2.float().cpu().reshape(-1)[:32].tolist()
    report["up1_attn2_digest"] = sha256_f32(up1_a2)
    print(f"ORACLE_UP1_ATTN2 {tuple(up1_a2.shape)} digest={report['up1_attn2_digest'][:16]}")
    up1_up = up1.upsamplers[0](up1_a2)
    report["up1_up_head"] = up1_up.float().cpu().reshape(-1)[:32].tolist()
    report["up1_up_digest"] = sha256_f32(up1_up)
    report["up1_up_shape"] = list(up1_up.shape)
    print(f"ORACLE_UP1_UP {tuple(up1_up.shape)} digest={report['up1_up_digest'][:16]}")

    up2 = unet.unet.up_blocks[2]
    up2_r0 = up2.resnets[0](torch.cat([up1_up, t["d1_res1"]], dim=1), t_emb)
    report["up2_res0_head"] = up2_r0.float().cpu().reshape(-1)[:32].tolist()
    report["up2_res0_digest"] = sha256_f32(up2_r0)
    print(f"ORACLE_UP2_RES0 {tuple(up2_r0.shape)} digest={report['up2_res0_digest'][:16]}")
    up2_a0, up2_a0_meta = _plain_transformer_wrap(up2.attentions[0], up2_r0, enc)
    report["up2_attn0_head"] = up2_a0.float().cpu().reshape(-1)[:32].tolist()
    report["up2_attn0_digest"] = sha256_f32(up2_a0)
    report["up2_attn0_meta"] = up2_a0_meta
    print(f"ORACLE_UP2_ATTN0 {tuple(up2_a0.shape)} digest={report['up2_attn0_digest'][:16]} {up2_a0_meta}")
    up2_r1 = up2.resnets[1](torch.cat([up2_a0, t["d1_res0"]], dim=1), t_emb)
    report["up2_res1_head"] = up2_r1.float().cpu().reshape(-1)[:32].tolist()
    report["up2_res1_digest"] = sha256_f32(up2_r1)
    print(f"ORACLE_UP2_RES1 {tuple(up2_r1.shape)} digest={report['up2_res1_digest'][:16]}")
    up2_a1, _ = _plain_transformer_wrap(up2.attentions[1], up2_r1, enc)
    report["up2_attn1_head"] = up2_a1.float().cpu().reshape(-1)[:32].tolist()
    report["up2_attn1_digest"] = sha256_f32(up2_a1)
    print(f"ORACLE_UP2_ATTN1 {tuple(up2_a1.shape)} digest={report['up2_attn1_digest'][:16]}")
    up2_r2 = up2.resnets[2](torch.cat([up2_a1, t["down"]], dim=1), t_emb)
    report["up2_res2_head"] = up2_r2.float().cpu().reshape(-1)[:32].tolist()
    report["up2_res2_digest"] = sha256_f32(up2_r2)
    print(f"ORACLE_UP2_RES2 {tuple(up2_r2.shape)} digest={report['up2_res2_digest'][:16]}")
    up2_a2, _ = _plain_transformer_wrap(up2.attentions[2], up2_r2, enc)
    report["up2_attn2_head"] = up2_a2.float().cpu().reshape(-1)[:32].tolist()
    report["up2_attn2_digest"] = sha256_f32(up2_a2)
    print(f"ORACLE_UP2_ATTN2 {tuple(up2_a2.shape)} digest={report['up2_attn2_digest'][:16]}")
    up2_up = up2.upsamplers[0](up2_a2)
    report["up2_up_head"] = up2_up.float().cpu().reshape(-1)[:32].tolist()
    report["up2_up_digest"] = sha256_f32(up2_up)
    report["up2_up_shape"] = list(up2_up.shape)
    print(f"ORACLE_UP2_UP {tuple(up2_up.shape)} digest={report['up2_up_digest'][:16]}")

    up3 = unet.unet.up_blocks[3]
    up3_r0 = up3.resnets[0](torch.cat([up2_up, t["res1"]], dim=1), t_emb)
    report["up3_res0_head"] = up3_r0.float().cpu().reshape(-1)[:32].tolist()
    report["up3_res0_digest"] = sha256_f32(up3_r0)
    print(f"ORACLE_UP3_RES0 {tuple(up3_r0.shape)} digest={report['up3_res0_digest'][:16]}")
    up3_a0, up3_a0_meta = _plain_transformer_wrap(up3.attentions[0], up3_r0, enc)
    report["up3_attn0_head"] = up3_a0.float().cpu().reshape(-1)[:32].tolist()
    report["up3_attn0_digest"] = sha256_f32(up3_a0)
    report["up3_attn0_meta"] = up3_a0_meta
    print(f"ORACLE_UP3_ATTN0 {tuple(up3_a0.shape)} digest={report['up3_attn0_digest'][:16]} {up3_a0_meta}")
    up3_r1 = up3.resnets[1](torch.cat([up3_a0, t["res0"]], dim=1), t_emb)
    report["up3_res1_head"] = up3_r1.float().cpu().reshape(-1)[:32].tolist()
    report["up3_res1_digest"] = sha256_f32(up3_r1)
    print(f"ORACLE_UP3_RES1 {tuple(up3_r1.shape)} digest={report['up3_res1_digest'][:16]}")
    up3_a1, _ = _plain_transformer_wrap(up3.attentions[1], up3_r1, enc)
    report["up3_attn1_head"] = up3_a1.float().cpu().reshape(-1)[:32].tolist()
    report["up3_attn1_digest"] = sha256_f32(up3_a1)
    print(f"ORACLE_UP3_ATTN1 {tuple(up3_a1.shape)} digest={report['up3_attn1_digest'][:16]}")
    up3_r2 = up3.resnets[2](torch.cat([up3_a1, t["conv"]], dim=1), t_emb)
    report["up3_res2_head"] = up3_r2.float().cpu().reshape(-1)[:32].tolist()
    report["up3_res2_digest"] = sha256_f32(up3_r2)
    print(f"ORACLE_UP3_RES2 {tuple(up3_r2.shape)} digest={report['up3_res2_digest'][:16]}")
    up3_a2, _ = _plain_transformer_wrap(up3.attentions[2], up3_r2, enc)
    report["up3_attn2_head"] = up3_a2.float().cpu().reshape(-1)[:32].tolist()
    report["up3_attn2_digest"] = sha256_f32(up3_a2)
    print(f"ORACLE_UP3_ATTN2 {tuple(up3_a2.shape)} digest={report['up3_attn2_digest'][:16]}")

    gn_out = unet.unet.conv_norm_out
    report["conv_norm_eps"] = float(gn_out.eps)
    report["conv_norm_groups"] = int(gn_out.num_groups)
    head_n = gn_out(up3_a2)
    head_n = torch.nn.functional.silu(head_n)
    report["conv_norm_head"] = head_n.float().cpu().reshape(-1)[:32].tolist()
    report["conv_norm_digest"] = sha256_f32(head_n)
    print(
        f"ORACLE_CONV_NORM {tuple(head_n.shape)} digest={report['conv_norm_digest'][:16]} "
        f"eps={report['conv_norm_eps']} groups={report['conv_norm_groups']}"
    )
    head = unet.unet.conv_out(head_n)
    report["conv_out_head"] = head.float().cpu().reshape(-1)[:32].tolist()
    report["conv_out_digest"] = sha256_f32(head)
    report["conv_out_shape"] = list(head.shape)
    print(f"ORACLE_CONV_OUT {tuple(head.shape)} digest={report['conv_out_digest'][:16]}")

    # Later-layer extras: MDA on up1.attentions.0 (1280 ch, 2x2, 20 heads)
    saved_up1 = {
        "proc": up1.attentions[0].transformer_blocks[0].transformer.attn1.processor,
        "mda": up1.attentions[0].transformer_blocks[0].use_mda,
        "ma": up1.attentions[0].transformer_blocks[0].use_ma,
        "ra": up1.attentions[0].transformer_blocks[0].use_ra,
        "dino": up1.attentions[0].transformer_blocks[0].use_dino,
    }
    wrap_u = up1.attentions[0].transformer_blocks[0]
    inner_u = wrap_u.transformer
    wrap_u.use_mda = True
    wrap_u.use_ma = wrap_u.use_ra = wrap_u.use_dino = False
    inner_u.attn1.set_processor(saved_up1["proc"])
    up1_mr = up1_r0 * 0.7 + 0.05
    up1_h2 = torch.cat([up1_r0, up1_mr], dim=0)
    up1_mda = _unwrap_mod(
        up1.attentions[0](
            up1_h2,
            encoder_hidden_states=enc2,
            cross_attention_kwargs={"mode": "", "num_in_batch": 1},
        )
    )
    report["up1_mda_head"] = up1_mda.float().cpu().reshape(-1)[:32].tolist()
    report["up1_mda_digest"] = sha256_f32(up1_mda)
    print(f"ORACLE_UP1_MDA {tuple(up1_mda.shape)} digest={report['up1_mda_digest'][:16]}")
    inner_u.attn1.set_processor(saved_up1["proc"])
    wrap_u.use_mda = saved_up1["mda"]
    wrap_u.use_ma = saved_up1["ma"]
    wrap_u.use_ra = saved_up1["ra"]
    wrap_u.use_dino = saved_up1["dino"]

    t["up1_res0"] = up1_r0
    extras_on_acts = dump_extras_on_graph(
        unet, t, t_emb, enc_alb, enc_mr, enc4, dino_tok, voxels, report
    )
    extras_on_mod = dump_extras_on_module(
        unet, t["conv"], t_emb, enc_alb, enc_mr, enc4, dino_tok, voxels, report
    )

    out = {
        "mda": mda,
        "dino": dino,
        "mda_dino": mda_dino,
        "ref": ref_out,
        "ma": ma,
        "full25": full,
        "up0_res0": up0_r0,
        "up0_res1": up0_r1,
        "up0_res2": up0_r2,
        "up0_up": up0_up,
        "up1_res0": up1_r0,
        "up1_attn0": up1_a0,
        "up1_res1": up1_r1,
        "up1_attn1": up1_a1,
        "up1_res2": up1_r2,
        "up1_attn2": up1_a2,
        "up1_up": up1_up,
        "up2_res0": up2_r0,
        "up2_attn0": up2_a0,
        "up2_res1": up2_r1,
        "up2_attn1": up2_a1,
        "up2_res2": up2_r2,
        "up2_attn2": up2_a2,
        "up2_up": up2_up,
        "up3_res0": up3_r0,
        "up3_attn0": up3_a0,
        "up3_res1": up3_r1,
        "up3_attn1": up3_a1,
        "up3_res2": up3_r2,
        "up3_attn2": up3_a2,
        "conv_norm": head_n,
        "conv_out": head,
        "up1_mda": up1_mda,
        "mr_in": mr_in,
        "ref_in": ref,
        "dino_tok": dino_tok,
        "ma_voxel": vox_idx,
        "h4": h4,
    }
    out.update(extras_on_acts)
    out.update(extras_on_mod)
    return out


def dump_extras_on_graph(unet, t, t_emb, enc_alb, enc_mr, enc4, dino_tok, voxels, report):
    """2-view x 2-material extras-on isolated later attns + chained graph."""
    write_enc = torch.cat([enc_alb, enc_alb], dim=0)
    acts = {}
    for key, tensor in voxels.items():
        xyz = tensor["voxel_indices"].float()
        acts[f"ma_voxel_{int(key)}"] = xyz
        report[f"ma_voxel_{int(key)}_res"] = int(tensor["voxel_resolution"])
        report[f"ma_voxel_{int(key)}_n"] = int(xyz.reshape(-1, 3).shape[0])

    def tap(name, tensor):
        report[f"{name}_head"] = tensor.float().cpu().reshape(-1)[:32].tolist()
        report[f"{name}_digest"] = sha256_f32(tensor)
        report[f"{name}_shape"] = list(tensor.shape)
        print(f"ORACLE_{name.upper()} {tuple(tensor.shape)} digest={report[f'{name}_digest'][:16]}")
        acts[name] = tensor

    def attn4(attn, xs):
        return _split4(
            _extras_attn(
                attn,
                _cat4(xs),
                enc4,
                dino_tok,
                voxels,
                _write2(xs[0], xs[1]),
                write_enc,
            )
        )

    # Isolated extras-on on extras-off official inputs (later layers).
    iso = [
        ("xod0a0", unet.unet.down_blocks[0].attentions[0], t["res0"]),
        ("xod1a0", unet.unet.down_blocks[1].attentions[0], t["d1_res0"]),
        ("xomid", unet.unet.mid_block.attentions[0], t["mid_res0"]),
    ]
    if "up1_res0" in t:
        iso.append(("xou1a0", unet.unet.up_blocks[1].attentions[0], t["up1_res0"]))
    for name, attn, src in iso:
        pack = _affine4(src)
        tap(name, _extras_attn(attn, _cat4(pack), enc4, dino_tok, voxels, _write2(pack[0], pack[1]), write_enc))

    # Chained extras-on from conv_in affine-4 pack. Save skips per sample.
    try:
        _dump_extras_on_chain(unet, t, t_emb, tap, attn4)
    except Exception as e:
        print(f"ORACLE_XON_CHAIN_FAIL {type(e).__name__}: {e}")
        traceback.print_exc()
    return acts


def _dump_extras_on_chain(unet, t, t_emb, tap, attn4):
    xs = _affine4(t["conv"])
    tap("xon_conv", _cat4(xs))
    skips = [list(xs)]  # conv_in skips, 4 samples

    down0 = unet.unet.down_blocks[0]
    xs = _map_resnets(down0.resnets, xs, t_emb, 0)
    xs = attn4(down0.attentions[0], xs)
    tap("xon_d0a0", _cat4(xs))
    skips.append(list(xs))
    xs = _map_resnets(down0.resnets, xs, t_emb, 1)
    xs = attn4(down0.attentions[1], xs)
    tap("xon_d0a1", _cat4(xs))
    skips.append(list(xs))
    xs = _map_down(down0.downsamplers[0], xs)
    tap("xon_d0down", _cat4(xs))
    skips.append(list(xs))

    down1 = unet.unet.down_blocks[1]
    xs = _map_resnets(down1.resnets, xs, t_emb, 0)
    xs = attn4(down1.attentions[0], xs)
    tap("xon_d1a0", _cat4(xs))
    skips.append(list(xs))
    xs = _map_resnets(down1.resnets, xs, t_emb, 1)
    xs = attn4(down1.attentions[1], xs)
    skips.append(list(xs))
    xs = _map_down(down1.downsamplers[0], xs)
    tap("xon_d1down", _cat4(xs))
    skips.append(list(xs))

    down2 = unet.unet.down_blocks[2]
    xs = _map_resnets(down2.resnets, xs, t_emb, 0)
    xs = attn4(down2.attentions[0], xs)
    tap("xon_d2a0", _cat4(xs))
    skips.append(list(xs))
    xs = _map_resnets(down2.resnets, xs, t_emb, 1)
    xs = attn4(down2.attentions[1], xs)
    skips.append(list(xs))
    xs = _map_down(down2.downsamplers[0], xs)
    tap("xon_d2down", _cat4(xs))
    skips.append(list(xs))

    down3 = unet.unet.down_blocks[3]
    xs = _map_resnets(down3.resnets, xs, t_emb, 0)
    skips.append(list(xs))
    xs = _map_resnets(down3.resnets, xs, t_emb, 1)
    tap("xon_d3r1", _cat4(xs))
    skips.append(list(xs))

    mid = unet.unet.mid_block
    xs = _map_resnets(mid.resnets, xs, t_emb, 0)
    xs = attn4(mid.attentions[0], xs)
    tap("xon_mid", _cat4(xs))
    xs = _map_resnets(mid.resnets, xs, t_emb, 1)
    tap("xon_midr1", _cat4(xs))

    def pop_skip():
        return skips.pop()

    def cat_skip(hidden, skip):
        return [torch.cat([h, s], dim=1) for h, s in zip(hidden, skip)]

    up0 = unet.unet.up_blocks[0]
    xs = _map_resnets(up0.resnets, cat_skip(xs, pop_skip()), t_emb, 0)
    xs = _map_resnets(up0.resnets, cat_skip(xs, pop_skip()), t_emb, 1)
    xs = _map_resnets(up0.resnets, cat_skip(xs, pop_skip()), t_emb, 2)
    xs = _map_up(up0.upsamplers[0], xs)
    tap("xon_up0", _cat4(xs))

    up1 = unet.unet.up_blocks[1]
    xs = _map_resnets(up1.resnets, cat_skip(xs, pop_skip()), t_emb, 0)
    xs = attn4(up1.attentions[0], xs)
    tap("xon_u1a0", _cat4(xs))
    xs = _map_resnets(up1.resnets, cat_skip(xs, pop_skip()), t_emb, 1)
    xs = attn4(up1.attentions[1], xs)
    xs = _map_resnets(up1.resnets, cat_skip(xs, pop_skip()), t_emb, 2)
    xs = attn4(up1.attentions[2], xs)
    xs = _map_up(up1.upsamplers[0], xs)
    tap("xon_up1", _cat4(xs))

    up2 = unet.unet.up_blocks[2]
    xs = _map_resnets(up2.resnets, cat_skip(xs, pop_skip()), t_emb, 0)
    xs = attn4(up2.attentions[0], xs)
    tap("xon_u2a0", _cat4(xs))
    xs = _map_resnets(up2.resnets, cat_skip(xs, pop_skip()), t_emb, 1)
    xs = attn4(up2.attentions[1], xs)
    xs = _map_resnets(up2.resnets, cat_skip(xs, pop_skip()), t_emb, 2)
    xs = attn4(up2.attentions[2], xs)
    xs = _map_up(up2.upsamplers[0], xs)
    tap("xon_up2", _cat4(xs))

    up3 = unet.unet.up_blocks[3]
    xs = _map_resnets(up3.resnets, cat_skip(xs, pop_skip()), t_emb, 0)
    xs = attn4(up3.attentions[0], xs)
    tap("xon_u3a0", _cat4(xs))
    xs = _map_resnets(up3.resnets, cat_skip(xs, pop_skip()), t_emb, 1)
    xs = attn4(up3.attentions[1], xs)
    xs = _map_resnets(up3.resnets, cat_skip(xs, pop_skip()), t_emb, 2)
    xs = attn4(up3.attentions[2], xs)
    tap("xon_u3a2", _cat4(xs))

    heads = [unet.unet.conv_out(torch.nn.functional.silu(unet.unet.conv_norm_out(x))) for x in xs]
    tap("xon_head", _cat4(heads))
    if skips:
        print(f"ORACLE_XON_SKIP_LEFTOVER {len(skips)}")


def _attn_states(unet):
    states = []
    for attn in _iter_transformer_attns(unet):
        wrap = attn.transformer_blocks[0]
        inner = wrap.transformer
        states.append((inner, wrap, inner.attn1.processor, wrap.use_mda, wrap.use_ma, wrap.use_ra, wrap.use_dino))
    return states


def _apply_write_extras(states):
    for inner, wrap, *_ in states:
        _set_plain_attn_processor(inner)
        wrap.use_mda = wrap.use_ma = wrap.use_dino = False
        wrap.use_ra = True


def _apply_read_extras(states):
    for inner, wrap, proc, *_ in states:
        inner.attn1.set_processor(proc)
        wrap.use_mda = wrap.use_ma = wrap.use_ra = wrap.use_dino = True


@torch.no_grad()
def dump_extras_on_module(unet, conv, t_emb, enc_alb, enc_mr, enc4, dino, voxels, report):
    """2-view x 2-material extras-on `down_blocks[i](...)` / up module-chain."""
    states = _attn_states(unet)
    write_enc = torch.cat([enc_alb, enc_alb], dim=0)
    xs = _affine4(conv)
    h4 = _cat4(xs)
    w2 = _write2(xs[0], xs[1])
    cond = {}
    acts = {}

    def tap(name, tensor):
        report[f"{name}_head"] = tensor.float().cpu().reshape(-1)[:32].tolist()
        report[f"{name}_digest"] = sha256_f32(tensor)
        report[f"{name}_shape"] = list(tensor.shape)
        print(f"ORACLE_{name.upper()} {tuple(tensor.shape)} digest={report[f'{name}_digest'][:16]}")
        acts[name] = tensor

    wkw = {"mode": "w", "num_in_batch": 2, "condition_embed_dict": cond}
    rkw = {
        "mode": "r",
        "num_in_batch": 2,
        "condition_embed_dict": cond,
        "dino_hidden_states": dino,
        "position_voxel_indices": voxels,
        "ref_scale": 1.0,
        "mva_scale": 1.0,
    }

    def call_down(idx, hidden, enc, kwargs):
        block = unet.unet.down_blocks[idx]
        if getattr(block, "attentions", None):
            return block(hidden, t_emb, encoder_hidden_states=enc, cross_attention_kwargs=kwargs)
        return block(hidden, t_emb)

    def zip_down(idx, hidden, enc, kwargs):
        block = unet.unet.down_blocks[idx]
        skips = []
        if getattr(block, "attentions", None):
            for resnet, attn in zip(block.resnets, block.attentions):
                hidden = resnet(hidden, t_emb)
                hidden = _call_attn(attn, hidden, enc, kwargs)
                skips.append(hidden)
        else:
            for resnet in block.resnets:
                hidden = resnet(hidden, t_emb)
                skips.append(hidden)
        if getattr(block, "downsamplers", None):
            hidden = block.downsamplers[0](hidden)
            skips.append(hidden)
        return hidden, skips

    def zip_up(block, hidden, res_tuple, enc, kwargs, with_attn):
        res_list = list(res_tuple)
        for i, resnet in enumerate(block.resnets):
            skip = res_list.pop()
            hidden = resnet(torch.cat([hidden, skip], dim=1), t_emb)
            if with_attn:
                hidden = _call_attn(block.attentions[i], hidden, enc, kwargs)
        if getattr(block, "upsamplers", None):
            hidden = block.upsamplers[0](hidden)
        return hidden

    try:
        _apply_write_extras(states)
        w = w2
        w_skips = [w]
        for i in range(4):
            w, sk = call_down(i, w, write_enc, wkw)
            w_skips.extend(sk)
        w = unet.unet.mid_block(w, t_emb, encoder_hidden_states=write_enc, cross_attention_kwargs=wkw)
        for up in unet.unet.up_blocks:
            n = len(up.resnets)
            res = tuple(w_skips[-n:])
            del w_skips[-n:]
            if getattr(up, "attentions", None):
                w = up(w, res, t_emb, encoder_hidden_states=write_enc, cross_attention_kwargs=wkw)
            else:
                w = up(w, res, t_emb)
        report["xom_write_keys"] = sorted(str(k) for k in cond.keys())
        print(f"ORACLE_XOM_WRITE_KEYS {report['xom_write_keys']}")

        _apply_read_extras(states)
        h = h4
        r_skips = [h]

        # down0 zip + module
        hz, zsk = zip_down(0, h, enc4, rkw)
        tap("xom_d0a0", zsk[0])
        tap("xom_d0a1", zsk[1])
        tap("xom_d0out", hz)
        try:
            blk, bsk = call_down(0, h, enc4, rkw)
            report["xom_d0_vs_block"] = err_stats(hz, blk)
            print(f"ORACLE_XOM_D0_VS_BLOCK max_abs={report['xom_d0_vs_block']['max_abs']:.3e} nskip={len(bsk)}")
            h, sk = blk, bsk
        except Exception as e:
            print(f"ORACLE_XOM_D0_BLOCK_FAIL {type(e).__name__}: {e}")
            traceback.print_exc()
            h, sk = hz, zsk
        r_skips.extend(sk)

        hz, zsk = zip_down(1, h, enc4, rkw)
        tap("xom_d1a0", zsk[0])
        tap("xom_d1a1", zsk[1])
        tap("xom_d1out", hz)
        try:
            blk, bsk = call_down(1, h, enc4, rkw)
            report["xom_d1_vs_block"] = err_stats(hz, blk)
            print(f"ORACLE_XOM_D1_VS_BLOCK max_abs={report['xom_d1_vs_block']['max_abs']:.3e}")
            h, sk = blk, bsk
        except Exception as e:
            print(f"ORACLE_XOM_D1_BLOCK_FAIL {type(e).__name__}: {e}")
            h, sk = hz, zsk
        r_skips.extend(sk)

        hz, zsk = zip_down(2, h, enc4, rkw)
        tap("xom_d2a0", zsk[0])
        tap("xom_d2out", hz)
        try:
            blk, bsk = call_down(2, h, enc4, rkw)
            report["xom_d2_vs_block"] = err_stats(hz, blk)
            print(f"ORACLE_XOM_D2_VS_BLOCK max_abs={report['xom_d2_vs_block']['max_abs']:.3e}")
            h, sk = blk, bsk
        except Exception as e:
            print(f"ORACLE_XOM_D2_BLOCK_FAIL {type(e).__name__}: {e}")
            h, sk = hz, zsk
        r_skips.extend(sk)

        hz, zsk = zip_down(3, h, enc4, rkw)
        tap("xom_d3r1", zsk[-1])
        try:
            blk, bsk = call_down(3, h, enc4, rkw)
            report["xom_d3_vs_block"] = err_stats(hz, blk)
            print(f"ORACLE_XOM_D3_VS_BLOCK max_abs={report['xom_d3_vs_block']['max_abs']:.3e}")
            h, sk = blk, bsk
        except Exception as e:
            print(f"ORACLE_XOM_D3_BLOCK_FAIL {type(e).__name__}: {e}")
            h, sk = hz, zsk
        r_skips.extend(sk)

        mid = unet.unet.mid_block
        hm = mid.resnets[0](h, t_emb)
        hm = _call_attn(mid.attentions[0], hm, enc4, rkw)
        tap("xom_mida", hm)
        hm = mid.resnets[1](hm, t_emb)
        tap("xom_midr1", hm)
        try:
            blk = mid(h, t_emb, encoder_hidden_states=enc4, cross_attention_kwargs=rkw)
            report["xom_mid_vs_block"] = err_stats(hm, blk)
            print(f"ORACLE_XOM_MID_VS_BLOCK max_abs={report['xom_mid_vs_block']['max_abs']:.3e}")
            h = blk
        except Exception as e:
            print(f"ORACLE_XOM_MID_BLOCK_FAIL {type(e).__name__}: {e}")
            h = hm

        def pop_n(n):
            chunk = tuple(r_skips[-n:])
            del r_skips[-n:]
            return chunk

        up0 = unet.unet.up_blocks[0]
        res = pop_n(len(up0.resnets))
        try:
            h = up0(h, res, t_emb)
        except Exception as e:
            print(f"ORACLE_XOM_UP0_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up0, h, res, enc4, rkw, False)
        tap("xom_up0", h)

        up1 = unet.unet.up_blocks[1]
        res = pop_n(len(up1.resnets))
        try:
            h = up1(h, res, t_emb, encoder_hidden_states=enc4, cross_attention_kwargs=rkw)
        except Exception as e:
            print(f"ORACLE_XOM_UP1_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up1, h, res, enc4, rkw, True)
        tap("xom_up1", h)

        up2 = unet.unet.up_blocks[2]
        res = pop_n(len(up2.resnets))
        try:
            h = up2(h, res, t_emb, encoder_hidden_states=enc4, cross_attention_kwargs=rkw)
        except Exception as e:
            print(f"ORACLE_XOM_UP2_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up2, h, res, enc4, rkw, True)
        tap("xom_up2", h)

        up3 = unet.unet.up_blocks[3]
        res = pop_n(len(up3.resnets))
        try:
            h = up3(h, res, t_emb, encoder_hidden_states=enc4, cross_attention_kwargs=rkw)
        except Exception as e:
            print(f"ORACLE_XOM_UP3_BLOCK_FAIL {type(e).__name__}: {e}")
            h = zip_up(up3, h, res, enc4, rkw, True)
        tap("xom_u3out", h)

        heads = unet.unet.conv_out(torch.nn.functional.silu(unet.unet.conv_norm_out(h)))
        tap("xom_head", heads)
        if r_skips:
            print(f"ORACLE_XOM_SKIP_LEFTOVER {len(r_skips)}")
    except Exception as e:
        print(f"ORACLE_XOM_FAIL {type(e).__name__}: {e}")
        traceback.print_exc()
    finally:
        _extras_restore(states)
    return acts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--weights", default=str(DEFAULT_WEIGHTS))
    parser.add_argument("--out", default="pbr_official_oracle.json")
    parser.add_argument("--size", type=int, default=64, help="RGB view size; latent is size/8")
    parser.add_argument("--unet", action="store_true", help="also load UNet and dump conv_in/resnet/down/attn")
    parser.add_argument("--skip-vae", action="store_true", help="skip VAE encode/decode dump")
    parser.add_argument("--dtype", default="fp16", choices=["fp16", "fp32"])
    args = parser.parse_args()
    if not torch.cuda.is_available():
        print("ORACLE_FAIL no CUDA", file=sys.stderr)
        return 1
    device = torch.device("cuda")
    dtype = torch.float16 if args.dtype == "fp16" else torch.float32
    weights = Path(args.weights)
    report = {
        "source": "official_hunyuan_paint",
        "weights": str(weights),
        "gpu": torch.cuda.get_device_name(device),
        "torch": torch.__version__,
        "dtype": args.dtype,
        "size": args.size,
    }
    print(f"ORACLE_GPU {report['gpu']}")
    if not args.skip_vae:
        vae = load_vae(weights, device, dtype)
        report["vae"] = dump_vae(vae, device, args.size)
        del vae
        torch.cuda.empty_cache()
    if args.unet:
        report["unet"] = dump_unet_stages(weights, device, dtype, args.size)
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"ORACLE_DUMP {os.path.abspath(args.out)}")
    print("ORACLE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
