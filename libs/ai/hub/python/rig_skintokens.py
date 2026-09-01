#!/usr/bin/env python
"""rig_skintokens.py <in.glb> <out.glb> — the rig domain's box script.

Wraps the SkinTokens one-pass auto-rigger (C:\\ai\\SkinTokens, venv_st): mesh
GLB in, skinned GLB out, --use_transfer keeps the input texture. Runs under
the SAME interpreter the service template names (venv_st python), calling
SkinTokens' demo.py as a subprocess with its repo dir as cwd (it spawns its
own bpy_server).

Protocol (subproc_img.rs): progress lines "@P <frac 0..1> <stage>" on
stdout; anything else passes through to the service log. Non-zero exit or a
missing/invalid output file fails the job. An optional params sidecar sits
at <in.glb>.json ({"seed": N}).  Upstream enables stochastic sampling and
beam search but exposes no seed flag, so this wrapper seeds Python, NumPy and
Torch before executing demo.py.  Upstream's SkinVAE condition encoder also
constructs ``np.random.default_rng(None)``; that generator ignores the legacy
global NumPy seed and otherwise draws fresh OS entropy on every request.  The
bootstrap maps only the ``None`` case to the request seed while preserving
explicit seeds (the Michelangelo encoder deliberately uses seed 0).  That
makes a request reproducible and gives the native port a fixed oracle instead
of a moving target.  The released demo also forgets to put the complete
TokenRig module in eval mode (only its VAE is switched), leaving
Michelangelo's FPS random start enabled.  The bootstrap wraps the imported
model factory and applies ``eval()`` before demo inference.

Env knobs:
  SKINTOKENS_DIR   repo dir (default C:\\ai\\SkinTokens)
  SKINTOKENS_ARGS  extra demo.py args, whitespace-split
"""
import json
import os
import subprocess
import sys


def progress(frac, stage):
    print("@P %.3f %s" % (frac, stage), flush=True)


def main():
    if len(sys.argv) != 3:
        print("usage: rig_skintokens.py <in.glb> <out.glb>", flush=True)
        return 2
    in_glb = os.path.abspath(sys.argv[1])
    out_glb = os.path.abspath(sys.argv[2])
    repo = os.environ.get("SKINTOKENS_DIR", r"C:\ai\SkinTokens")

    params = {}
    try:
        with open(in_glb + ".json", "r", encoding="utf-8") as f:
            params = json.load(f)
    except OSError:
        pass
    print("rig: params %r" % (params,), flush=True)

    progress(0.02, "skintokens: starting")
    seed = int(params.get("seed", 0)) & 0xFFFFFFFF
    bootstrap = (
        "import random,runpy;"
        f"seed={seed};random.seed(seed);"
        "import numpy as np;np.random.seed(seed);"
        "_make_rng=np.random.default_rng;"
        "np.random.default_rng=lambda value=None:_make_rng(seed if value is None else value);"
        "import torch;torch.manual_seed(seed);torch.cuda.manual_seed_all(seed);"
        "import src.server.spec as _spec;_get_model=_spec.get_model;"
        "_spec.get_model=lambda *args,**kwargs:_get_model(*args,**kwargs).eval();"
        "runpy.run_path('demo.py',run_name='__main__')"
    )
    cmd = [
        sys.executable,
        "-c",
        bootstrap,
        "--input", in_glb,
        "--output", out_glb,
        "--use_transfer",
    ]
    extra = os.environ.get("SKINTOKENS_ARGS", "").split()
    cmd.extend(extra)
    print("rig: run %r (cwd %s)" % (cmd, repo), flush=True)
    progress(0.10, "skintokens: model load + autoregressive rig")

    child = subprocess.Popen(
        cmd,
        cwd=repo,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    # Stream child output into the service log; nudge the fraction along on
    # recognizable phase lines (SkinTokens prints per-stage banners).
    frac = 0.10
    for line in child.stdout:
        line = line.rstrip("\r\n")
        low = line.lower()
        for needle, at in (
            ("load", 0.20),
            ("skeleton", 0.40),
            ("skin", 0.60),
            ("transfer", 0.75),
            ("export", 0.85),
        ):
            if needle in low and at > frac:
                frac = at
                progress(frac, "skintokens: " + needle)
                break
        print("st| " + line, flush=True)
    code = child.wait()
    if code != 0:
        print("rig: demo.py exit %d" % code, flush=True)
        return 1

    # Output contract: a GLB that actually carries a skin.
    try:
        with open(out_glb, "rb") as f:
            head = f.read(64 * 1024 * 1024)
    except OSError as e:
        print("rig: output missing: %s" % e, flush=True)
        return 1
    if head[:4] != b"glTF" or b'"skins"' not in head:
        print("rig: output is not a rigged GLB", flush=True)
        return 1
    progress(0.98, "skintokens: done")
    return 0


if __name__ == "__main__":
    sys.exit(main())
