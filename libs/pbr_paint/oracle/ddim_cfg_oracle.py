"""CPU DDIM + CFG oracle for Hunyuan3D-Paint-2.1.

Pins the 15-step trailing ZSNR v-pred schedule and the 3-branch guidance
combine independently of the UNet graph. No weights, no CUDA. Matches
`libs/pbr_paint/src/schedule.rs` and `denoise.rs`.

Optional: if `diffusers` is importable, compare the schedule against
`DDIMScheduler` from the pinned Hunyuan scheduler_config.
"""
from __future__ import annotations

import json
import math
import sys

TRAIN_TIMESTEPS = 1000
BETA_START = 0.00085
BETA_END = 0.012
STEPS = 15
GUIDANCE = 3.0


def round_half_even(v: float) -> float:
    floor = math.floor(v)
    diff = v - floor
    if diff > 0.5:
        return floor + 1.0
    if diff < 0.5:
        return floor
    if int(floor) % 2 == 0:
        return floor
    return floor + 1.0


def scaled_linear_betas():
    s0 = math.sqrt(BETA_START)
    s1 = math.sqrt(BETA_END)
    n = TRAIN_TIMESTEPS
    return [ (s0 + (s1 - s0) * i / (n - 1)) ** 2 for i in range(n) ]


def alphas_cumprod_zsnr():
    acc = 1.0
    sqrt_cum = []
    for beta in scaled_linear_betas():
        acc *= 1.0 - beta
        sqrt_cum.append(math.sqrt(acc))
    s0 = sqrt_cum[0]
    st = sqrt_cum[-1]
    scale = s0 / (s0 - st)
    return [ ((s - st) * scale) ** 2 for s in sqrt_cum ]


def timesteps_trailing(steps: int):
    ratio = TRAIN_TIMESTEPS / steps
    return [int(round_half_even(TRAIN_TIMESTEPS - i * ratio) - 1) for i in range(steps)]


def prev_timestep(t: int, steps: int):
    prev = t - (TRAIN_TIMESTEPS // steps)
    return prev if prev >= 0 else None


def alpha_sigma(ac, t):
    a = math.sqrt(ac[t])
    return a, math.sqrt(1.0 - ac[t])


def ddim_step(x, v, t, steps, ac):
    a, s = alpha_sigma(ac, t)
    prev = prev_timestep(t, steps)
    if prev is None:
        ap, sp = 1.0, 0.0
    else:
        ap, sp = alpha_sigma(ac, prev)
    out = []
    for xv, vv in zip(x, v):
        x0 = a * xv - s * vv
        eps = s * xv + a * vv
        out.append(ap * x0 + sp * eps)
    return out


def view_guidance_scale(azim: float) -> float:
    if 0.0 <= azim < 90.0:
        return azim / 90.0 + 1.0
    if 90.0 <= azim < 330.0:
        return 2.0
    return -azim / 90.0 + 5.0


def guidance_combine(uncond, ref_only, full, guidance, view_scales, row_len):
    out = []
    for row, vs in enumerate(view_scales):
        a = guidance * vs
        base = row * row_len
        for k in range(row_len):
            u = uncond[base + k]
            r = ref_only[base + k]
            f = full[base + k]
            out.append(u + a * (r - u) + a * (f - r))
    return out


def main() -> int:
    ac = alphas_cumprod_zsnr()
    ts = timesteps_trailing(STEPS)
    report = {
        "steps": STEPS,
        "guidance": GUIDANCE,
        "timesteps": ts,
        "prev_999": prev_timestep(999, STEPS),
        "alpha0": ac[0],
        "alpha999": ac[999],
        "view_scale_0": view_guidance_scale(0.0),
        "view_scale_90": view_guidance_scale(90.0),
        "view_scale_330": view_guidance_scale(330.0),
    }

    x0 = [0.25, -1.5, 0.75, 2.0]
    eps = [1.0, 0.5, -0.25, -1.0]
    t = 666
    a, s = alpha_sigma(ac, t)
    xt = [a * x + s * e for x, e in zip(x0, eps)]
    v = [a * e - s * x for x, e in zip(x0, eps)]
    got = ddim_step(xt, v, t, STEPS, ac)
    prev = prev_timestep(t, STEPS)
    ap, sp = alpha_sigma(ac, prev)
    expect = [ap * x + sp * e for x, e in zip(x0, eps)]
    report["ddim_t"] = t
    report["ddim_prev"] = prev
    report["ddim_got"] = got
    report["ddim_expect"] = expect
    report["ddim_max_abs"] = max(abs(g - e) for g, e in zip(got, expect))

    guided = guidance_combine([0.0, 0.0], [10.0, -10.0], [1.0, 1.0], 3.0, [1.0], 2)
    report["guidance_out"] = guided

    print("DDIM_CFG_ORACLE " + json.dumps(report, sort_keys=True))
    assert ts == [999, 932, 866, 799, 732, 666, 599, 532, 466, 399, 332, 266, 199, 132, 66]
    assert prev_timestep(999, STEPS) == 933
    assert ac[999] == 0.0
    assert abs(ac[0] - (1.0 - BETA_START)) < 1e-12
    assert report["ddim_max_abs"] < 1e-12
    assert abs(guided[0] - 3.0) < 1e-12
    print("DDIM_CFG_ORACLE_OK")

    try:
        from diffusers import DDIMScheduler
    except Exception as exc:
        print(f"DDIM_CFG_ORACLE_DIFFUSERS_SKIP {exc}")
        return 0

    sched = DDIMScheduler(
        num_train_timesteps=TRAIN_TIMESTEPS,
        beta_start=BETA_START,
        beta_end=BETA_END,
        beta_schedule="scaled_linear",
        prediction_type="v_prediction",
        clip_sample=False,
        set_alpha_to_one=True,
        steps_offset=0,
        interpolation_type="linear",
        timestep_spacing="trailing",
        rescale_betas_zero_snr=True,
    )
    sched.set_timesteps(STEPS)
    dts = [int(t) for t in sched.timesteps.tolist()]
    print("DDIM_CFG_ORACLE_DIFFUSERS " + json.dumps({"timesteps": dts}))
    if dts != ts:
        print("DDIM_CFG_ORACLE_DIFFUSERS_MISMATCH", file=sys.stderr)
        return 1
    print("DDIM_CFG_ORACLE_DIFFUSERS_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
