#!/usr/bin/env python3
"""Build the IQ-dequant oracle fixture.

Oracle = llama.cpp's gguf-py numpy dequantizers (an implementation independent
of both ggml-quants.c and our Rust port). Cases are:
  * real blocks lifted out of the unsloth UD- GGUFs on this machine, for the
    types those files actually use (IQ4_XS / IQ4_NL / IQ3_S / Q3_K), and
  * deterministic pseudo-random blocks for every other type we add kernels for.

Fixture format (little-endian):
  magic "IQFX" u32 | n_cases u32
  per case: ggml_type u32 | n_elems u32 | n_bytes u32 | raw bytes | f32[n_elems]
"""
import struct, sys, os, hashlib
sys.path.insert(0, "/Users/admin/llama.cpp/gguf-py")

import numpy as np
from gguf.constants import GGMLQuantizationType
from gguf import quants

OUT = "/Users/admin/makepad/makepad/libs/ai/cuda/tests/data/iq_dequant_oracle.bin"

TYPE_IDS = {
    "Q2_K": 10, "Q3_K": 11,
    "IQ2_XXS": 16, "IQ2_XS": 17, "IQ3_XXS": 18, "IQ1_S": 19,
    "IQ4_NL": 20, "IQ3_S": 21, "IQ2_S": 22, "IQ4_XS": 23, "IQ1_M": 29,
}
BLOCK_BYTES = {
    "Q2_K": 84, "Q3_K": 110, "IQ2_XXS": 66, "IQ2_XS": 74, "IQ3_XXS": 98,
    "IQ1_S": 50, "IQ4_NL": 18, "IQ3_S": 110, "IQ2_S": 82, "IQ4_XS": 136,
    "IQ1_M": 56,
}
BLOCK_ELEMS = {k: (32 if k == "IQ4_NL" else 256) for k in BLOCK_BYTES}

GGML_TYPE_NAMES = {v: k for k, v in TYPE_IDS.items()}


def read_gguf_tensor_bytes(path, wanted_types, per_type=1, nblocks=8):
    """Return {type_name: raw bytes of the first `nblocks` blocks of a tensor}."""
    out = {}
    with open(path, "rb") as f:
        def u32(): return struct.unpack("<I", f.read(4))[0]
        def u64(): return struct.unpack("<Q", f.read(8))[0]
        def s():
            n = u64(); return f.read(n).decode("utf-8", "replace")
        def read_val(t):
            if t == 0: return f.read(1)[0]
            if t == 1: return struct.unpack("<b", f.read(1))[0]
            if t == 2: return struct.unpack("<H", f.read(2))[0]
            if t == 3: return struct.unpack("<h", f.read(2))[0]
            if t == 4: return u32()
            if t == 5: return struct.unpack("<i", f.read(4))[0]
            if t == 6: return struct.unpack("<f", f.read(4))[0]
            if t == 7: return f.read(1)[0] != 0
            if t == 8: return s()
            if t == 9:
                et = u32(); n = u64()
                return [read_val(et) for _ in range(n)]
            if t == 10: return u64()
            if t == 11: return struct.unpack("<q", f.read(8))[0]
            if t == 12: return struct.unpack("<d", f.read(8))[0]
            raise ValueError(t)
        assert f.read(4) == b"GGUF"
        u32()  # version
        n_tensors = u64(); n_kv = u64()
        for _ in range(n_kv):
            s(); read_val(u32())
        tensors = []
        for _ in range(n_tensors):
            name = s(); nd = u32()
            dims = [u64() for _ in range(nd)]
            tt = u32(); off = u64()
            tensors.append((name, dims, tt, off))
        align = 32
        data_start = (f.tell() + align - 1) // align * align
        for name, dims, tt, off in tensors:
            tn = GGML_TYPE_NAMES.get(tt)
            if tn is None or tn not in wanted_types or tn in out:
                continue
            nb = BLOCK_BYTES[tn] * nblocks
            f.seek(data_start + off)
            out[tn] = (name, f.read(nb))
    return out


def dequant(tn, raw):
    n_blocks = len(raw) // BLOCK_BYTES[tn]
    arr = np.frombuffer(raw, dtype=np.uint8).reshape(n_blocks, BLOCK_BYTES[tn])
    qt = getattr(quants, tn)
    if hasattr(qt, "init_grid"):
        qt.init_grid()
    return qt.dequantize_blocks(arr).astype(np.float32).reshape(-1)


def main():
    cases = []
    model = "/Users/admin/makepad/makepad/local/models/Qwen3.8-27B-UD-Q4_K_M.gguf"
    real = read_gguf_tensor_bytes(model, {"IQ4_XS", "IQ4_NL", "IQ3_S", "Q3_K"})
    for tn, (name, raw) in sorted(real.items()):
        vals = dequant(tn, raw)
        cases.append((tn, raw, vals))
        print(f"real   {tn:8s} from {name}: {len(raw)} bytes -> {vals.size} values "
              f"[{vals.min():.5f}, {vals.max():.5f}]")

    rng = np.random.default_rng(7)
    for tn in sorted(BLOCK_BYTES):
        nblocks = 8
        raw = rng.integers(0, 256, size=BLOCK_BYTES[tn] * nblocks, dtype=np.uint8).tobytes()
        # Keep the f16 scale fields sane: random 16 bits can be inf/NaN, which
        # would make the comparison meaningless. Patch each block's `d` to 1.0
        # where the layout puts an f16 at a known offset.
        d_off = {"Q2_K": 80, "Q3_K": 108, "IQ2_XXS": 0, "IQ2_XS": 0, "IQ2_S": 0,
                 "IQ3_XXS": 0, "IQ3_S": 0, "IQ1_S": 0, "IQ4_NL": 0, "IQ4_XS": 0}
        buf = bytearray(raw)
        bb = BLOCK_BYTES[tn]
        if tn in d_off:
            for b in range(nblocks):
                off = b * bb + d_off[tn]
                buf[off:off + 2] = struct.pack("<e", np.float16(1.0))
            if tn == "Q2_K":  # dmin too
                for b in range(nblocks):
                    off = b * bb + 82
                    buf[off:off + 2] = struct.pack("<e", np.float16(0.5))
        if tn == "IQ1_M":
            # IQ1_M's f16 super-scale is scattered across the top nibble of the
            # four `scales` u16s; random bits there give inf/NaN. Pin those four
            # nibbles so the assembled half is 1.0 (0x3C00) and leave the low 12
            # bits (the per-sub-block 3-bit scales) random.
            top = [0x0, 0x0, 0xC, 0x3]
            for b in range(nblocks):
                base = b * bb + 48
                for i in range(4):
                    v = struct.unpack("<H", bytes(buf[base + 2 * i:base + 2 * i + 2]))[0]
                    v = (v & 0x0fff) | (top[i] << 12)
                    buf[base + 2 * i:base + 2 * i + 2] = struct.pack("<H", v)
        raw = bytes(buf)
        vals = dequant(tn, raw)
        assert np.all(np.isfinite(vals)), f"{tn} oracle produced non-finite values"
        cases.append((tn, raw, vals))
        print(f"random {tn:8s}: {len(raw)} bytes -> {vals.size} values "
              f"[{vals.min():.5f}, {vals.max():.5f}]")

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(b"IQFX")
        f.write(struct.pack("<I", len(cases)))
        for tn, raw, vals in cases:
            f.write(struct.pack("<III", TYPE_IDS[tn], vals.size, len(raw)))
            f.write(raw)
            f.write(vals.astype("<f4").tobytes())
    digest = hashlib.sha256(open(OUT, "rb").read()).hexdigest()
    print(f"wrote {OUT} ({os.path.getsize(OUT)} bytes) sha256={digest[:16]}")


main()
