#!/usr/bin/env python3
"""Convert Kokoro's `.pth` weights into the flat `.mktts` file the Rust loader reads.

Standard library only — a `.pth` is a zip around a pickle, so torch and numpy are
not needed. Tensors are copied verbatim; PyTorch's weight-norm pairs
(`weight_g` / `weight_v`) are left intact and reconstructed on the Rust side,
where `W = g * v / ||v||` is cheap.

    python3 convert_kokoro.py kokoro-v1_0.pth kokoro-v1_0.mktts
    python3 convert_kokoro.py --voice kokoro_voices/af_heart.pt af_heart.mkvoice

Format (little endian):

    magic   b"MKTTS\\0\\0\\0"      8 bytes
    version u32                    = 1
    count   u32                    number of tensors
    index   count * {
        name_len u32, name utf8,
        dtype    u8   0 = f32
        ndim     u8
        shape    ndim * u32
        offset   u64  from start of file
        nbytes   u64
    }
    blob    tensor data, each 32-byte aligned
"""

import struct
import sys
import zipfile

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from inspect_pth import DTYPES, Storage, Tensor, load, walk  # noqa: E402

MAGIC = b"MKTTS\0\0\0"
VERSION = 1
ALIGN = 32


def storage_bytes(archive, prefix, storage, cache):
    """Raw bytes of one storage record, cached — tensors can share storage."""
    if storage.key not in cache:
        with archive.open(f"{prefix}data/{storage.key}") as handle:
            cache[storage.key] = handle.read()
    return cache[storage.key]


def tensor_bytes(archive, prefix, tensor, cache):
    dtype, itemsize = DTYPES[tensor.storage.dtype]
    if dtype != "f32":
        raise SystemExit(f"unsupported dtype {dtype}; expected all-f32 weights")
    raw = storage_bytes(archive, prefix, tensor.storage, cache)
    start = tensor.offset * itemsize
    end = start + tensor.numel * itemsize
    if end > len(raw):
        raise SystemExit(f"tensor overruns its storage ({end} > {len(raw)})")
    return raw[start:end]


def collect(path):
    root = load(path)
    if isinstance(root, Tensor):
        return [("style", root)]  # a voice pack is a bare tensor
    return list(walk(root))


def convert(src, dst):
    tensors = collect(src)
    if not tensors:
        raise SystemExit(f"no tensors found in {src}")

    with zipfile.ZipFile(src) as archive:
        pkl = next(n for n in archive.namelist() if n.endswith("data.pkl"))
        prefix = pkl[: -len("data.pkl")]
        cache = {}
        payloads = [tensor_bytes(archive, prefix, t, cache) for _, t in tensors]

    # Lay out the index first so offsets are known before anything is written.
    index_size = 4 + 4  # version + count  (magic written separately)
    for (name, tensor), _ in zip(tensors, payloads):
        index_size += 4 + len(name.encode()) + 1 + 1 + 4 * len(tensor.shape) + 8 + 8

    offset = len(MAGIC) + index_size
    offsets = []
    for payload in payloads:
        offset = (offset + ALIGN - 1) // ALIGN * ALIGN
        offsets.append(offset)
        offset += len(payload)

    with open(dst, "wb") as out:
        out.write(MAGIC)
        out.write(struct.pack("<II", VERSION, len(tensors)))
        for ((name, tensor), payload, at) in zip(tensors, payloads, offsets):
            encoded = name.encode()
            out.write(struct.pack("<I", len(encoded)))
            out.write(encoded)
            out.write(struct.pack("<BB", 0, len(tensor.shape)))
            for dim in tensor.shape:
                out.write(struct.pack("<I", dim))
            out.write(struct.pack("<QQ", at, len(payload)))
        for payload, at in zip(payloads, offsets):
            out.write(b"\0" * (at - out.tell()))
            out.write(payload)

    total = sum(t.numel for _, t in tensors)
    print(f"{src} -> {dst}")
    print(f"  tensors : {len(tensors)}")
    print(f"  params  : {total/1e6:.2f}M")
    print(f"  bytes   : {offset/1048576:.1f} MB")


def main():
    args = [a for a in sys.argv[1:] if a != "--voice"]
    if len(args) != 2:
        raise SystemExit(__doc__)
    convert(args[0], args[1])


if __name__ == "__main__":
    main()
