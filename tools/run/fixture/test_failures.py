"""Publish the gate's kept outputs after their files have been copied."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import tempfile
import unicodedata


def append_bytes(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("ab", buffering=0) as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        view = memoryview(data)
        while view:
            view = view[stream.write(view):]


def append_manifest(manifest_path, records):
    lines = "".join(json.dumps(r, separators=(",", ":"), ensure_ascii=False) + "\n"
                    for r in records)
    if lines:
        append_bytes(manifest_path, lines.encode("utf-8"))


def flush_manifest(staged_path, dest_path):
    data = Path(staged_path).read_bytes()
    if data.strip():
        append_bytes(dest_path, data if data.endswith(b"\n") else data + b"\n")


def valid_rel(path):
    return (all(part not in ("", ".", "..") for part in path.split("/"))
            and "\\" not in path
            and not any(c.isspace() or unicodedata.category(c) == "Cc" for c in path))


def write_run_pointer(pointer_path, run_dir):
    if "/" in run_dir or not valid_rel(run_dir):
        raise ValueError("invalid gate run directory")
    path = Path(pointer_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
        try:
            tmp.write((run_dir + "\n").encode("utf-8"))
            tmp.flush()
            os.replace(tmp.name, path)
        finally:
            Path(tmp.name).unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--manifest")
    mode.add_argument("--flush-manifest", nargs=2)
    parser.add_argument("--run-pointer", nargs=2)
    parser.add_argument("--kind", choices=["test", "hung", "unit", "busy", "busy_clear"])
    parser.add_argument("--unit")
    parser.add_argument("--test")
    parser.add_argument("--unit-log")
    parser.add_argument("--names")
    parser.add_argument("files", nargs="*")
    args = parser.parse_args()
    if args.manifest:
        if not args.kind or not args.unit:
            parser.error("a manifest record needs --kind and --unit")
        if not all(valid_rel(f) for f in args.files + [f for f in (args.unit_log, args.names) if f]):
            parser.error("manifest filenames must be relative to the gate run directory")
        record = {"v": 1, "kind": args.kind, "unit": args.unit}
        if args.kind == "unit" and args.files:
            record["files"] = args.files
        elif args.kind in ("test", "hung", "busy") and len(args.files) == 1:
            record["file"] = args.files[0]
            if args.kind != "busy":
                if not args.test:
                    parser.error("a per-test record needs --test")
                record["test"] = args.test
                for k in ("unit_log", "names"):
                    if getattr(args, k):
                        record[k] = getattr(args, k)
        elif args.kind != "busy_clear" or args.files:
            parser.error("wrong number of files for this record")
        append_manifest(args.manifest, [record])
    elif args.flush_manifest:
        flush_manifest(*args.flush_manifest)
    elif not args.run_pointer:
        parser.error("a manifest or run pointer is required")
    if args.run_pointer:
        write_run_pointer(*args.run_pointer)


if __name__ == "__main__":
    main()
