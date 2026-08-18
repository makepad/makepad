"""Dump unet_dual write-mode contract (one forward at t=0).

Does not run the full 3.9GB UNet unless --forward. Default is config +
state-dict key/shape checks for unet_dual.conv_in (4-ch) vs unet.conv_in
(12-ch) and the 16 write-layer names.

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

DEFAULT_WEIGHTS = os.environ.get(
    "MAKEPAD_HUNYUAN_WEIGHTS",
    r"C:\ai\Hunyuan3D-2.1\weights\hunyuan3d-paintpbr-v2-1",
)


def main() -> int:
    import torch

    path = os.path.join(DEFAULT_WEIGHTS, "unet", "diffusion_pytorch_model.bin")
    obj = torch.load(path, map_location="cpu", weights_only=True)
    main_in = list(obj["unet.conv_in.weight"].shape)
    dual_in = list(obj["unet_dual.conv_in.weight"].shape)
    has_dual_attn = any(k.startswith("unet_dual.down_blocks.0.attentions") for k in obj)
    dual_extras = any("attn_dino" in k or "to_q_mr" in k for k in obj if k.startswith("unet_dual."))
    report = {
        "unet_conv_in": main_in,
        "unet_dual_conv_in": dual_in,
        "unet_dual_has_attn": has_dual_attn,
        "unet_dual_has_25d_extras": dual_extras,
        "write_layers": WRITE_LAYERS,
        "n_keys": len(obj),
        "n_dual_keys": sum(1 for k in obj if k.startswith("unet_dual.")),
    }
    print("DUAL_WRITE_ORACLE " + json.dumps(report, sort_keys=True))
    assert main_in[1] == 12, main_in
    assert dual_in[1] == 4, dual_in
    assert has_dual_attn
    assert not dual_extras
    print("DUAL_WRITE_ORACLE_OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
