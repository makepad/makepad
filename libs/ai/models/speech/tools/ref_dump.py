#!/usr/bin/env python3
"""Expose Kokoro's internal ONNX tensors so the Rust port can be diffed stage by stage.

Without this, a wrong sign somewhere in the prosody predictor shows up only as
"the audio sounds a bit off" — 2,463 nodes downstream. With it, each stage is
checked where it is written.

    # list candidate tensors
    /tmp/kref/bin/python libs/tts/tools/ref_dump.py --list lstm

    # dump matching tensors as .npy under refdump/
    /tmp/kref/bin/python libs/tts/tools/ref_dump.py --dump text_encoder "Hello there."
"""

import os
import subprocess
import sys

import numpy as np
import onnx
import onnxruntime as ort

MODEL = "kokoro_ref.onnx"
VOICE = "af_heart.mkvoice"
OUT_DIR = "refdump"


def rust_tokens(text):
    out = subprocess.run(
        ["cargo", "run", "--release", "--quiet",
         "--manifest-path", "libs/tts/Cargo.toml", "--bin", "g2p_test", "--", "--ids", text],
        capture_output=True, text=True, check=True,
    )
    return [int(x) for x in out.stdout.strip().split(",")]


def load_voice():
    sys.path.insert(0, os.path.dirname(__file__))
    from ref_infer import load_voice as load  # reuse the container reader

    return load(VOICE)


def candidates(model):
    """Every value produced by a node, in graph order."""
    return [out for node in model.graph.node for out in node.output if out]


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    mode, pattern = sys.argv[1], sys.argv[2]
    text = sys.argv[3] if len(sys.argv) > 3 else "Escape the Gummer."

    model = onnx.load(MODEL)
    names = [n for n in candidates(model) if pattern.lower() in n.lower()]

    if mode == "--list":
        print(f"{len(names)} tensors matching {pattern!r}:")
        for name in names[:60]:
            print(f"  {name}")
        if len(names) > 60:
            print(f"  ... {len(names)-60} more")
        return

    if mode != "--dump":
        raise SystemExit(__doc__)
    if not names:
        raise SystemExit(f"nothing matches {pattern!r}")

    # Promote the chosen tensors to graph outputs.
    existing = {o.name for o in model.graph.output}
    for name in names:
        if name not in existing:
            model.graph.output.extend([onnx.ValueInfoProto(name=name)])

    ids = rust_tokens(text)
    # `pack[len(ps) - 1]`, where len(ps) == len(ids) - 2 (the zero pads).
    style = load_voice()[len(ids) - 3].astype(np.float32).reshape(1, 256)

    session = ort.InferenceSession(
        model.SerializeToString(), providers=["CPUExecutionProvider"]
    )
    wanted = [o.name for o in session.get_outputs()]
    values = session.run(
        wanted,
        {
            "input_ids": np.array([ids], dtype=np.int64),
            "style": style,
            "speed": np.array([1.0], dtype=np.float32),
        },
    )

    os.makedirs(OUT_DIR, exist_ok=True)
    print(f"text: {text}\ntokens: {len(ids)}\n")
    for name, value in zip(wanted, values):
        if name not in names:
            continue
        safe = name.strip("/").replace("/", "_")
        np.save(f"{OUT_DIR}/{safe}.npy", value)
        array = np.asarray(value)
        print(f"  {name}")
        print(f"    shape={array.shape} dtype={array.dtype} "
              f"mean={array.mean():+.5f} std={array.std():.5f} "
              f"min={array.min():+.4f} max={array.max():+.4f}")


if __name__ == "__main__":
    main()
