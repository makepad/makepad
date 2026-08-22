#!/usr/bin/env python3
"""List the tensors inside a PyTorch `.pth` using the standard library only.

A `.pth` is a zip around a pickle whose tensors are `persistent_id` references
into raw storage records. Stubbing the handful of `torch` symbols the pickle
names is enough to read the structure without installing torch or numpy.

    python3 inspect_pth.py kokoro-v1_0.pth [--tree]
"""

import pickle
import sys
import zipfile
from collections import OrderedDict

# storage class name -> (short dtype, bytes per element)
DTYPES = {
    "FloatStorage": ("f32", 4),
    "HalfStorage": ("f16", 2),
    "BFloat16Storage": ("bf16", 2),
    "DoubleStorage": ("f64", 8),
    "LongStorage": ("i64", 8),
    "IntStorage": ("i32", 4),
    "ShortStorage": ("i16", 2),
    "CharStorage": ("i8", 1),
    "ByteStorage": ("u8", 1),
    "BoolStorage": ("bool", 1),
}


class Storage:
    __slots__ = ("key", "dtype", "numel")

    def __init__(self, key, dtype, numel):
        self.key = key
        self.dtype = dtype
        self.numel = numel


class Tensor:
    __slots__ = ("storage", "offset", "shape", "stride")

    def __init__(self, storage, offset, shape, stride):
        self.storage = storage
        self.offset = offset
        self.shape = shape
        self.stride = stride

    @property
    def numel(self):
        n = 1
        for d in self.shape:
            n *= d
        return n


def _rebuild_tensor_v2(storage, storage_offset, size, stride, *_rest):
    return Tensor(storage, storage_offset, tuple(size), tuple(stride))


def _rebuild_parameter(data, *_rest):
    return data


class _Unpickler(pickle.Unpickler):
    def find_class(self, module, name):
        if module.startswith("torch"):
            if name == "_rebuild_tensor_v2":
                return _rebuild_tensor_v2
            if name == "_rebuild_parameter":
                return _rebuild_parameter
            if name.endswith("Storage"):
                # Returned as a marker; persistent_load reads it back out.
                return name
        if module == "collections" and name == "OrderedDict":
            return OrderedDict
        raise pickle.UnpicklingError(f"refusing to load {module}.{name}")

    def persistent_load(self, saved_id):
        assert saved_id[0] == "storage", saved_id[0]
        storage_type, key, _location, numel = saved_id[1], saved_id[2], saved_id[3], saved_id[4]
        return Storage(str(key), str(storage_type), numel)


def load(path):
    with zipfile.ZipFile(path) as archive:
        pkl_name = next(n for n in archive.namelist() if n.endswith("data.pkl"))
        with archive.open(pkl_name) as handle:
            return _Unpickler(handle).load()


def walk(node, prefix=""):
    """Yield (dotted_name, Tensor) for every tensor in a nested dict."""
    if isinstance(node, Tensor):
        yield prefix, node
        return
    if isinstance(node, dict):
        for key, value in node.items():
            child = f"{prefix}.{key}" if prefix else str(key)
            yield from walk(value, child)


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "kokoro-v1_0.pth"
    tree = "--tree" in sys.argv

    root = load(path)
    print(f"top-level keys: {list(root)}\n" if isinstance(root, dict) else f"root: {type(root)}\n")

    tensors = list(walk(root))
    if tree:
        for name, tensor in tensors:
            dtype = DTYPES.get(tensor.storage.dtype, ("?", 0))[0]
            print(f"  {name:<70} {dtype:>4} {list(tensor.shape)}")

    # Parameter counts per top-level module, which is what tells you where the
    # 82M actually lives.
    by_module = {}
    total = 0
    for name, tensor in tensors:
        module = name.split(".")[0]
        by_module[module] = by_module.get(module, 0) + tensor.numel
        total += tensor.numel

    print(f"\n{len(tensors)} tensors, {total/1e6:.2f}M parameters")
    for module, count in sorted(by_module.items(), key=lambda kv: -kv[1]):
        print(f"  {module:<20} {count/1e6:>7.2f}M  ({100*count/total:4.1f}%)")


if __name__ == "__main__":
    main()
