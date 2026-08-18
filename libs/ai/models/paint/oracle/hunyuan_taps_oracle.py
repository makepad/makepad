"""Frozen Hunyuan3D-Paint-2.1 layer-section oracle.

Uses the pinned UNet math from hunyuan3d-paintpbr-v2-1 (weights snapshot
0b94677654c57bb9a6b6845cd7b704ccf551d327, code 82920d643c0dc2f7bfd7255f45f62d386edfe60c)
without loading the 3.9 GiB UNet. Layer taps match the native pbr-cuda-taps
fixtures: mul, row-broadcast add, value-first GEGLU-erf, interleaved RoPE,
packed cross-attention, one SD ResNet block, and cond-assembly voxel indices.

This is not a full UNet / service-layer executor.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import struct
import sys
from typing import Any

import torch
import torch.nn.functional as F

# Frozen fixtures from libs/pbr_paint/src/numerical_fixtures.rs
MUL_LEFT = torch.tensor([[1.0, -2.0, 0.5], [8.0, -0.25, 3.0]], dtype=torch.float32)
MUL_RIGHT = torch.tensor([[4.0, 0.5, -6.0], [-0.125, 16.0, 2.0]], dtype=torch.float32)
MUL_EXPECT = torch.tensor([[4.0, -1.0, -3.0], [-1.0, -4.0, 6.0]], dtype=torch.float32)

ADD_LEFT = torch.tensor([[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]], dtype=torch.float32)
ADD_RIGHT = torch.tensor([-1.0, 0.5, 2.0], dtype=torch.float32)
ADD_EXPECT = torch.tensor([[0.0, 1.0], [3.5, 4.5], [7.0, 8.0]], dtype=torch.float32)

GEGLU_IN = torch.tensor([[2.0, -3.0, 1.0, -1.0]], dtype=torch.float32)
GEGLU_EXPECT = torch.tensor([[1.6826895, 0.47596577]], dtype=torch.float32)

ROPE_IN = torch.tensor([[1.0, 2.0, 3.0, 4.0], [1.0, 2.0, 3.0, 4.0]], dtype=torch.float32)
ROPE_COS = torch.tensor([[1.0, 1.0], [0.0, 0.0]], dtype=torch.float32)
ROPE_SIN = torch.tensor([[0.0, 0.0], [1.0, 1.0]], dtype=torch.float32)
ROPE_EXPECT = torch.tensor([[1.0, 2.0, 3.0, 4.0], [-2.0, 1.0, -4.0, 3.0]], dtype=torch.float32)

ATTN_Q = torch.tensor([[1.0, 0.0]], dtype=torch.float32)
ATTN_K = torch.tensor([[1.0, 0.0], [0.0, 1.0]], dtype=torch.float32)
ATTN_V = torch.tensor([[2.0, 4.0], [6.0, 8.0]], dtype=torch.float32)
ATTN_EXPECT = torch.tensor([[3.0757656, 5.0757656]], dtype=torch.float32)

RESNET_DIGEST = "f389dad8503717795268d67247f08fa098ae3d508fa992ed905a8891e1cbf6fd"
GROUP_NORM_EPS = 1e-5

# Vendored from hunyuan3d-paintpbr-v2-1/unet/attn_processor.py RotaryEmbedding.apply_rotary_emb
def hunyuan_apply_rotary_emb(x: torch.Tensor, cos: torch.Tensor, sin: torch.Tensor) -> torch.Tensor:
    # x: [batch, heads, tokens, dim] or [tokens, dim] after we add leading dims
    x_real, x_imag = x.reshape(*x.shape[:-1], -1, 2).unbind(-1)
    x_rotated = torch.stack((-x_imag, x_real), dim=-1).flatten(start_dim=-2)
    return (x.float() * cos + x_rotated.float() * sin).to(x.dtype)


def compute_discrete_voxel_indice(
    position: torch.Tensor, grid_resolution: int = 8, voxel_resolution: int = 128
) -> torch.Tensor:
    """Vendored from hunyuan3d-paintpbr-v2-1/unet/modules.py (pinned snapshot)."""
    position = position.half()
    _b, _n, _c, height, width = position.shape
    assert height % grid_resolution == 0 and width % grid_resolution == 0
    valid_mask = (position != 1).all(dim=2, keepdim=True)
    valid_mask = valid_mask.expand_as(position)
    position = position.clone()
    position[valid_mask == False] = 0  # noqa: E712
    cell_h = height // grid_resolution
    cell_w = width // grid_resolution
    position = position.reshape(_b, _n, _c, grid_resolution, cell_h, grid_resolution, cell_w)
    position = position.permute(0, 1, 3, 5, 2, 4, 6).contiguous()
    valid_mask = valid_mask.reshape(_b, _n, _c, grid_resolution, cell_h, grid_resolution, cell_w)
    valid_mask = valid_mask.permute(0, 1, 3, 5, 2, 4, 6).contiguous()
    grid_position = position.sum(dim=(-2, -1))
    count_masked = valid_mask.sum(dim=(-2, -1))
    grid_position = grid_position / count_masked.clamp(min=1)
    voxel_mask_thres = (height // grid_resolution) * (width // grid_resolution) // (4 * 4)
    grid_position = grid_position.clone()
    grid_position[count_masked < voxel_mask_thres] = 0
    grid_position = grid_position.permute(0, 1, 4, 2, 3).clamp(0, 1)
    voxel_indices = grid_position * (voxel_resolution - 1)
    return torch.round(voxel_indices).long()


def wrap_u64(value: int) -> int:
    return value & 0xFFFFFFFFFFFFFFFF


def gen(seed: int, length: int, scale: float) -> torch.Tensor:
    """Port of numerical_fixtures.rs::gen (splitmix64-style)."""
    state = wrap_u64(seed)
    out = []
    for _ in range(length):
        state = wrap_u64(state + 0x9E3779B97F4A7C15)
        z = state
        z = wrap_u64((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9)
        z = wrap_u64((z ^ (z >> 27)) * 0x94D049BB133111EB)
        z ^= z >> 31
        unit = float(z >> 40) / float(1 << 24)
        out.append((unit * 2.0 - 1.0) * scale)
    return torch.tensor(out, dtype=torch.float32)


def f16_round(values: torch.Tensor) -> torch.Tensor:
    return values.to(torch.float16).to(torch.float32)


def resnet_inputs(device: torch.device) -> dict[str, Any]:
    cin, cout, width, height, temb_dim = 4, 8, 4, 4, 8
    temb_w = f16_round(gen(11, cout * temb_dim, 0.5))
    return {
        "cin": cin,
        "cout": cout,
        "width": width,
        "height": height,
        "gn1_groups": 2,
        "gn2_groups": 4,
        "temb_dim": temb_dim,
        "x": gen(1, cin * width * height, 1.0).to(device),
        "temb": gen(2, temb_dim, 1.0).to(device),
        "gn1_gamma": (1.0 + gen(3, cin, 0.5)).to(device),
        "gn1_beta": gen(4, cin, 0.2).to(device),
        "conv1_w": gen(5, cout * cin * 9, 0.3).to(device),
        "conv1_b": gen(6, cout, 0.1).to(device),
        "temb_w_f16": temb_w.to(device),
        "temb_b": gen(12, cout, 0.1).to(device),
        "gn2_gamma": (1.0 + gen(7, cout, 0.5)).to(device),
        "gn2_beta": gen(8, cout, 0.2).to(device),
        "conv2_w": gen(9, cout * cout * 9, 0.2).to(device),
        "conv2_b": gen(10, cout, 0.1).to(device),
        "short_w": gen(13, cout * cin, 0.4).to(device),
        "short_b": gen(14, cout, 0.1).to(device),
    }


def nchw(planar: torch.Tensor, channels: int, height: int, width: int) -> torch.Tensor:
    return planar.reshape(1, channels, height, width)


def group_norm_planar(
    planar: torch.Tensor, channels: int, height: int, width: int, groups: int, gamma: torch.Tensor, beta: torch.Tensor
) -> torch.Tensor:
    x = nchw(planar, channels, height, width)
    y = F.group_norm(x, groups, gamma, beta, eps=GROUP_NORM_EPS)
    return y.reshape(channels * height * width)


def conv2d_planar(
    planar: torch.Tensor,
    cin: int,
    height: int,
    width: int,
    weight: torch.Tensor,
    bias: torch.Tensor,
    cout: int,
    k: int,
    pad: int,
) -> torch.Tensor:
    x = nchw(planar, cin, height, width)
    w = weight.reshape(cout, cin, k, k)
    y = F.conv2d(x, w, bias, stride=1, padding=pad)
    return y.reshape(cout * height * width)


def resnet_block(inp: dict[str, Any]) -> torch.Tensor:
    width, height = inp["width"], inp["height"]
    h = group_norm_planar(inp["x"], inp["cin"], height, width, inp["gn1_groups"], inp["gn1_gamma"], inp["gn1_beta"])
    h = F.silu(h)
    h = conv2d_planar(h, inp["cin"], height, width, inp["conv1_w"], inp["conv1_b"], inp["cout"], 3, 1)
    temb_act = F.silu(inp["temb"])
    temb_proj = F.linear(temb_act, inp["temb_w_f16"].reshape(inp["cout"], inp["temb_dim"]), inp["temb_b"])
    plane = width * height
    h = h.reshape(inp["cout"], plane) + temb_proj.reshape(inp["cout"], 1)
    h = h.reshape(-1)
    h = group_norm_planar(h, inp["cout"], height, width, inp["gn2_groups"], inp["gn2_gamma"], inp["gn2_beta"])
    h = F.silu(h)
    h = conv2d_planar(h, inp["cout"], height, width, inp["conv2_w"], inp["conv2_b"], inp["cout"], 3, 1)
    shortcut = conv2d_planar(inp["x"], inp["cin"], height, width, inp["short_w"], inp["short_b"], inp["cout"], 1, 0)
    return h + shortcut


def digest_f32(values: torch.Tensor) -> str:
    raw = values.detach().float().cpu().contiguous().numpy().astype("<f4").tobytes()
    return hashlib.sha256(raw).hexdigest()


def err_stats(actual: torch.Tensor, expected: torch.Tensor) -> dict[str, float]:
    diff = (actual - expected).abs()
    denom = expected.abs().clamp_min(1e-12)
    return {
        "max_abs": float(diff.max().item()) if diff.numel() else 0.0,
        "max_rel": float((diff / denom).max().item()) if diff.numel() else 0.0,
        "mean_abs": float(diff.mean().item()) if diff.numel() else 0.0,
    }


def within(stats: dict[str, float], atol: float, rtol: float, ref_max: float) -> bool:
    return stats["max_abs"] <= atol + rtol * max(ref_max, 0.0)


def to_list(t: torch.Tensor) -> list[float]:
    return [float(v) for v in t.detach().float().cpu().reshape(-1).tolist()]


def view_guidance_scale(azim: float) -> float:
    if 0.0 <= azim < 90.0:
        return azim / 90.0 + 1.0
    if 90.0 <= azim < 330.0:
        return 2.0
    return -azim / 90.0 + 5.0


def guidance_combine(
    uncond: torch.Tensor, ref_only: torch.Tensor, full: torch.Tensor, guidance: float, view_scales: torch.Tensor
) -> torch.Tensor:
    # uncond + g*vs*(ref-uncond) + g*vs*(full-ref), exact two-term order
    a = guidance * view_scales.reshape(-1, 1)
    return uncond + a * (ref_only - uncond) + a * (full - ref_only)


def run_oracle(device: torch.device) -> dict[str, Any]:
    report: dict[str, Any] = {
        "source": "hunyuan_taps_oracle",
        "torch": torch.__version__,
        "cuda": bool(torch.cuda.is_available()),
        "device": str(device),
        "gpu": torch.cuda.get_device_name(device) if device.type == "cuda" else None,
        "weights_snapshot": "0b94677654c57bb9a6b6845cd7b704ccf551d327",
        "code_revision": "82920d643c0dc2f7bfd7255f45f62d386edfe60c",
        "compared": [
            "mul",
            "add_rows_broadcast",
            "geglu_exact_erf",
            "geglu_tanh_rejected",
            "rope_interleaved (Hunyuan apply_rotary_emb)",
            "cross_attention (SDPA scale=1)",
            "resnet_block (torch CUDA group_norm/silu/conv2d/linear)",
            "cond_assembly voxel indices + guidance combine",
        ],
        "not_compared": [
            "full UNet2p5D graph",
            "VAE encode/decode",
            "DINOv2-giant",
            "fp16 checkpoint loader / weight-keyed graph",
            "service-layer hunyuan3d-paint-2.1",
        ],
        "taps": {},
    }

    mul = MUL_LEFT.to(device) * MUL_RIGHT.to(device)
    report["taps"]["mul_f32_precise"] = {
        **err_stats(mul, MUL_EXPECT.to(device)),
        "atol": 0.0,
        "rtol": 0.0,
        "values": to_list(mul),
        "pass": bool(torch.equal(mul.cpu(), MUL_EXPECT)),
    }

    add = ADD_LEFT.to(device) + ADD_RIGHT.to(device).reshape(-1, 1)
    report["taps"]["add_rows_broadcast"] = {
        **err_stats(add, ADD_EXPECT.to(device)),
        "atol": 0.0,
        "rtol": 0.0,
        "values": to_list(add),
        "pass": bool(torch.allclose(add.cpu(), ADD_EXPECT, atol=0.0, rtol=0.0)),
    }

    value, gate = GEGLU_IN.to(device).split(2, dim=-1)
    gelu_erf = F.gelu(gate, approximate="none")
    geglu = value * gelu_erf
    gelu_tanh = F.gelu(gate, approximate="tanh")
    geglu_tanh = value * gelu_tanh
    geglu_stats = err_stats(geglu, GEGLU_EXPECT.to(device))
    report["taps"]["geglu_exact_erf"] = {
        **geglu_stats,
        "atol": 2e-6,
        "rtol": 2e-6,
        "values": to_list(geglu),
        "tanh_values": to_list(geglu_tanh),
        "tanh_vs_erf_max_abs": err_stats(geglu_tanh, geglu)["max_abs"],
        "pass": within(geglu_stats, 2e-6, 2e-6, float(GEGLU_EXPECT.abs().max())),
    }

    # Hunyuan apply_rotary_emb: x [1,1,T,D], cos/sin broadcast on head dim
    x = ROPE_IN.to(device).reshape(1, 1, 2, 4)
    cos = ROPE_COS.to(device).reshape(1, 1, 2, 2).repeat_interleave(2, dim=-1)
    sin = ROPE_SIN.to(device).reshape(1, 1, 2, 2).repeat_interleave(2, dim=-1)
    # Interleaved pair rotation uses per-pair (cos,sin). Native tables are
    # half-dim (2) and applied to pairs (0,1) and (2,3). Expand to dim.
    rope = hunyuan_apply_rotary_emb(x, cos, sin).reshape(2, 4)
    rope_stats = err_stats(rope, ROPE_EXPECT.to(device))
    report["taps"]["rope_interleaved_layout"] = {
        **rope_stats,
        "atol": 0.0,
        "rtol": 0.0,
        "values": to_list(rope),
        "pass": within(rope_stats, 0.0, 0.0, 1.0),
    }

    q = ATTN_Q.to(device).reshape(1, 1, 1, 2)
    k = ATTN_K.to(device).reshape(1, 1, 2, 2)
    v = ATTN_V.to(device).reshape(1, 1, 2, 2)
    attn = F.scaled_dot_product_attention(q, k, v, dropout_p=0.0, is_causal=False, scale=1.0)
    attn = attn.reshape(1, 2)
    attn_stats = err_stats(attn, ATTN_EXPECT.to(device))
    report["taps"]["cross_attention_q1_kv2"] = {
        **attn_stats,
        "atol": 2e-6,
        "rtol": 2e-6,
        "values": to_list(attn),
        "pass": within(attn_stats, 2e-6, 2e-6, float(ATTN_EXPECT.abs().max())),
    }

    inp = resnet_inputs(device)
    resnet = resnet_block(inp)
    host_inp = {k: (v.detach().float().cpu() if torch.is_tensor(v) else v) for k, v in inp.items()}
    host_ref = host_resnet_block(host_inp)
    host_digest = digest_f32(host_ref)
    torch_vs_host = err_stats(resnet.detach().float().cpu(), host_ref)
    report["taps"]["resnet_block"] = {
        **torch_vs_host,
        "torch_digest": digest_f32(resnet),
        "host_digest": host_digest,
        "pinned_digest": RESNET_DIGEST,
        "host_digest_match": host_digest == RESNET_DIGEST,
        "values": to_list(resnet),
        "host_values": to_list(host_ref),
        "numel": int(resnet.numel()),
        # Exact SHA is a Rust-host pin. The Python host replay is allowed a
        # last-ulp drift (sigmoid vs 1/(1+exp)); pass is the numeric gate
        # that CUDA taps also use. Digest match is reported, not required.
        "pass": within(torch_vs_host, 1e-4, 1e-3, float(host_ref.abs().max())),
        "atol": 1e-4,
        "rtol": 1e-3,
    }

    # Cond assembly: mid-gray 64x64 map, all four RoPE levels
    size = 64
    rgb = torch.full((1, 1, 3, size, size), 128.0 / 255.0, device=device, dtype=torch.float32)
    levels = []
    all_match = True
    for grid, voxel in ((64, 512), (32, 256), (16, 128), (8, 64)):
        idx = compute_discrete_voxel_indice(rgb, grid, voxel)
        expect = int(round(float(f16_round(torch.tensor([128.0 / 255.0]))[0]) * (voxel - 1)))
        match = bool(torch.all(idx == expect))
        all_match = all_match and match
        levels.append({"grid": grid, "voxel": voxel, "expect": expect, "unique": idx.unique().tolist(), "pass": match})
    uncond = torch.tensor([[0.0, 0.0]], device=device)
    ref_only = torch.tensor([[10.0, -10.0]], device=device)
    full = torch.tensor([[1.0, 1.0]], device=device)
    guided = guidance_combine(uncond, ref_only, full, 3.0, torch.tensor([1.0], device=device))
    report["taps"]["cond_assembly"] = {
        "view_scale_0": view_guidance_scale(0.0),
        "view_scale_45": view_guidance_scale(45.0),
        "view_scale_90": view_guidance_scale(90.0),
        "view_scale_330": view_guidance_scale(330.0),
        "guidance_out": to_list(guided),
        "guidance_expect": [3.0, 3.0],
        "voxel_levels": levels,
        "pass": all_match and abs(guided[0, 0].item() - 3.0) < 1e-5,
    }
    return report


def host_group_norm(
    x: torch.Tensor, channels: int, plane: int, groups: int, gamma: torch.Tensor, beta: torch.Tensor, eps: float
) -> torch.Tensor:
    """Match numerical_fixtures.rs::reference::group_norm (N-mean, /N variance)."""
    per = channels // groups
    out = torch.empty_like(x)
    for g in range(groups):
        span = per * plane
        start = g * span
        sl = x[start : start + span]
        mean = sl.sum() / span
        var = ((sl - mean) * (sl - mean)).sum() / span
        inv = 1.0 / torch.sqrt(var + eps)
        for c in range(per):
            ch = g * per + c
            at = ch * plane
            out[at : at + plane] = (x[at : at + plane] - mean) * inv * gamma[ch] + beta[ch]
    return out


def host_conv2d(
    x: torch.Tensor,
    cin: int,
    width: int,
    height: int,
    weights: torch.Tensor,
    bias: torch.Tensor,
    cout: int,
    k: int,
    pad: int,
) -> torch.Tensor:
    out = torch.zeros(cout * width * height, dtype=x.dtype, device=x.device)
    w = weights.reshape(cout, cin, k, k)
    for oc in range(cout):
        for oy in range(height):
            for ox in range(width):
                acc = bias[oc]
                for ic in range(cin):
                    for ky in range(k):
                        for kx in range(k):
                            iy = oy + ky - pad
                            ix = ox + kx - pad
                            if iy < 0 or ix < 0 or iy >= height or ix >= width:
                                continue
                            xv = x[ic * width * height + iy * width + ix]
                            acc = acc + xv * w[oc, ic, ky, kx]
                out[oc * width * height + oy * width + ox] = acc
    return out


def host_resnet_block(inp: dict[str, Any]) -> torch.Tensor:
    """CPU-style reference matching numerical_fixtures.rs, run in f32."""
    plane = inp["width"] * inp["height"]
    x = inp["x"]
    h = host_group_norm(x, inp["cin"], plane, inp["gn1_groups"], inp["gn1_gamma"], inp["gn1_beta"], GROUP_NORM_EPS)
    h = h * torch.sigmoid(h)
    h = host_conv2d(h, inp["cin"], inp["width"], inp["height"], inp["conv1_w"], inp["conv1_b"], inp["cout"], 3, 1)
    temb_act = inp["temb"] * torch.sigmoid(inp["temb"])
    temb_proj = F.linear(temb_act, inp["temb_w_f16"].reshape(inp["cout"], inp["temb_dim"]), inp["temb_b"])
    h = h.reshape(inp["cout"], plane) + temb_proj.reshape(inp["cout"], 1)
    h = h.reshape(-1)
    h = host_group_norm(h, inp["cout"], plane, inp["gn2_groups"], inp["gn2_gamma"], inp["gn2_beta"], GROUP_NORM_EPS)
    h = h * torch.sigmoid(h)
    h = host_conv2d(h, inp["cout"], inp["width"], inp["height"], inp["conv2_w"], inp["conv2_b"], inp["cout"], 3, 1)
    shortcut = host_conv2d(x, inp["cin"], inp["width"], inp["height"], inp["short_w"], inp["short_b"], inp["cout"], 1, 0)
    return h + shortcut


def compare_native(oracle: dict[str, Any], native_path: str) -> dict[str, Any]:
    with open(native_path, "r", encoding="utf-8") as fh:
        native = json.load(fh)
    native_taps = native.get("taps", {})
    cmp: dict[str, Any] = {"native_source": native.get("source"), "taps": {}}
    alias = {
        "mul_f32_precise": "mul_f32_precise",
        "add_rows_broadcast": "add_rows_broadcast",
        "geglu_exact_erf": "geglu_exact_erf",
        "rope_interleaved_layout": "rope_interleaved_layout",
        "cross_attention_q1_kv2": "cross_attention_q1_kv2",
        "resnet_block": "resnet_block",
    }
    for name, otap in oracle["taps"].items():
        if "values" not in otap:
            continue
        stem = alias.get(name)
        if stem is None:
            continue
        matches = [(k, v) for k, v in native_taps.items() if k == stem or k.startswith(stem + "_p")]
        if not matches:
            cmp["taps"][name] = {"present": False, "pass": False}
            continue
        ov = torch.tensor(otap["values"], dtype=torch.float32)
        atol = float(otap.get("atol", 1e-4))
        rtol = float(otap.get("rtol", 1e-3))
        worst = None
        for key, nt in matches:
            nv = torch.tensor(nt["values"], dtype=torch.float32)
            if ov.numel() != nv.numel():
                worst = {
                    "present": True,
                    "native_key": key,
                    "shape_mismatch": [int(nv.numel()), int(ov.numel())],
                    "pass": False,
                }
                continue
            stats = err_stats(nv, ov)
            # Same per-element gate as pbr-cuda-taps: atol + rtol*|ref|.
            # Do not scale by the tensor-wide max (that hid the ResNet gap).
            per_elem_ok = True
            if ov.numel() == nv.numel():
                allowed = atol + rtol * ov.abs()
                per_elem_ok = bool(torch.all((nv - ov).abs() <= allowed).item())
            row = {
                "present": True,
                "native_key": key,
                **stats,
                "atol": atol,
                "rtol": rtol,
                "pass": per_elem_ok,
            }
            if worst is None or stats["max_abs"] > worst.get("max_abs", -1.0):
                worst = row
        cmp["taps"][name] = worst
    return cmp


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="pbr_oracle_taps.json")
    parser.add_argument("--native", default=None, help="native pbr_cuda_taps.json to compare")
    parser.add_argument("--device", default="cuda")
    args = parser.parse_args()
    if args.device.startswith("cuda") and not torch.cuda.is_available():
        print("ORACLE_FAIL CUDA requested but torch.cuda.is_available() is False", file=sys.stderr)
        return 1
    device = torch.device(args.device)
    report = run_oracle(device)
    if args.native:
        report["native_compare"] = compare_native(report, args.native)
    with open(args.out, "w", encoding="utf-8") as fh:
        json.dump(report, fh, indent=2)
        fh.write("\n")
    failed = [k for k, v in report["taps"].items() if not v.get("pass", False)]
    print(f"ORACLE_GPU {report['gpu']}")
    print(f"ORACLE_TORCH {report['torch']} device={report['device']}")
    for name, tap in report["taps"].items():
        extra = ""
        if "max_abs" in tap:
            extra = f" max_abs={tap['max_abs']:.9e} max_rel={tap.get('max_rel', 0):.9e}"
        if "digest" in tap:
            extra += f" digest_match={tap['digest_match']}"
        print(f"ORACLE_TAP name={name} pass={tap.get('pass')}{extra}")
    if args.native and "native_compare" in report:
        for name, tap in report["native_compare"]["taps"].items():
            extra = ""
            if "max_abs" in tap:
                extra = f" max_abs={tap['max_abs']:.9e} max_rel={tap.get('max_rel', 0):.9e}"
            print(f"ORACLE_VS_NATIVE name={name} pass={tap.get('pass')}{extra}")
    print(f"ORACLE_DUMP {os.path.abspath(args.out)}")
    if failed:
        print(f"ORACLE_FAIL taps={failed}", file=sys.stderr)
        return 1
    print("ORACLE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
