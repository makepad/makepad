#!/usr/bin/env python3
"""Run on a build node: verify.py <makepad binary> <scratch directory>.

Uses the existing offline C fixture, cargo and the system C toolchain. Keeps
outputs in scratch for inspection; no fixture source or workspace is rewritten.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys

binary = str(Path(sys.argv[1]).resolve())
scratch = Path(sys.argv[2]).resolve()
scratch.mkdir(parents=True, exist_ok=True)
fixture = scratch / "fixture"
shutil.copytree(Path(__file__).parent, fixture,
                ignore=shutil.ignore_patterns("target", "Cargo.lock", "__pycache__"))
env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_NET_OFFLINE="true",
           CARGO_TARGET_DIR=str(scratch / "cargo"),
           MAKEPAD_RUN_CACHE_DIR=str(scratch / "jit"))


def run(label, args, success=True):
    result = subprocess.run(args, cwd=fixture, env=env, capture_output=True, text=True)
    (scratch / (label + ".stdout")).write_text(result.stdout)
    (scratch / (label + ".stderr")).write_text(result.stderr)
    assert (result.returncode == 0) == success, (
        f"{label}: exit {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


# Two unused patches reproduce the lockfile blocks from #1388. These packages
# are intentionally absent from the resolved graph.
for name in ["unused_a", "unused_b"]:
    package = fixture / name
    package.mkdir()
    (package / "Cargo.toml").write_text(
        f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
        '[lib]\npath = "lib.rs"\n')
    (package / "lib.rs").write_text("")
with (fixture / "Cargo.toml").open("a") as manifest:
    manifest.write('\n[patch.crates-io]\nunused_a = { path = "unused_a" }\n'
                   'unused_b = { path = "unused_b" }\n')
run("lock", ["cargo", "generate-lockfile", "--offline"])
baseline = run("cargo", ["cargo", "run", "--release", "--locked", "--quiet"])
command = [binary, "run", "--allow-c", "allow-c-fixture"]
first = run("first", command)
assert first.stdout == baseline.stdout, "native/JIT fixture output differs from cargo"
assert "callback 50\n" in first.stdout, f"C code calling back into Rust: {first.stdout}"
assert "building native dependencies with cargo" in first.stderr, "cold native build was not exercised"

native = scratch / "jit" / "native"
keys = sorted(p.name for p in native.iterdir() if (p / "messages.jsonl").is_file())
assert len(keys) == 1, f"expected one cold native key, got {keys}"
# The optimized background tier can replace libnative after the foreground
# exits. messages.jsonl is the foreground build receipt and stays unchanged.
receipts = {key: (native / key / "messages.jsonl").stat().st_mtime_ns for key in keys}
lock = fixture / "Cargo.lock"
blocks = lock.read_text().split("[[patch.unused]]")
assert len(blocks) == 3, "fixture did not produce two unused patches"
lock.write_text(blocks[0] + "".join("[[patch.unused]]" + block for block in reversed(blocks[1:])))
second = run("second", command)
assert second.stdout == baseline.stdout, "warm fixture output differs from cargo"
assert "building native dependencies with cargo" not in second.stderr, "warm launch rebuilt native code"
assert keys == sorted(p.name for p in native.iterdir() if (p / "messages.jsonl").is_file()), "lock order changed native key"
assert receipts == {key: (native / key / "messages.jsonl").stat().st_mtime_ns for key in keys}, "warm launch rewrote native build receipt"
print("PASS cache: reordered unused patches keep one key; second launch reuses native build")

# Keep the loader case independent of Cargo's unused-patch order checks. Even
# equivalent patch ordering can make Cargo demand a lock rewrite on a cold build;
# --locked correctly refuses that instead of mutating the input behind the key.
manifest = fixture / "Cargo.toml"
manifest.write_text(manifest.read_text().split("[patch.crates-io]")[0])
run("lock-without-patches", ["cargo", "generate-lockfile", "--offline"])

# An unused function still carries an unresolved import. Neither a library nor
# the program defines it: the native bundle is rejected before even the
# fixture's first println runs.
c_source = fixture / "cnative" / "csrc" / "cnative.c"
c_text = c_source.read_text()
c_source.write_text(c_text + '\nextern int cn_missing_native_symbol(void);\n'
                    'int cn_unresolved(void) { return cn_missing_native_symbol(); }\n')
failed = run("unresolved", command, success=False)
assert failed.returncode == 1, f"loader failure exit status: {failed.returncode}"
assert not failed.stdout, "program started despite a missing native import"
for expected in ["cannot load", "libnative.", "--allow-c native library of cnative 0.1.0", "`cn_missing_native_symbol`", "no `#[no_mangle]` fn of the program"]:
    assert expected in failed.stderr, f"missing loader diagnostic {expected!r}: {failed.stderr}"
print("PASS loader: unresolved import reports library, crate and loader error; exit 1; no program output")

# C code calling into the program while the library loads (a C constructor) runs before the
# loader binds the callbacks: the stub reports the symbol on stderr and exits 1 (cargo's static
# link has no such window).
c_source.write_text(c_text + '\n__attribute__((constructor)) static void cn_init(void) { (void)cn_rust_square(3); }\n')
early = run("constructor", command, success=False)
assert early.returncode == 1, f"constructor callback exit status: {early.returncode}"
assert not early.stdout, "program started after a constructor called into it"
for expected in ["cannot load the --allow-c native library", "`cn_rust_square`", "a C constructor"]:
    assert expected in early.stderr, f"missing constructor diagnostic {expected!r}: {early.stderr}"
c_source.write_text(c_text)
print("PASS constructor: a C constructor calling into Rust reports the symbol; exit 1; no program output")

# --locked must reject a changed graph rather than repairing its lockfile.
# Change the path dependency's version after resolution has a lock to read.
manifest = fixture / "cnative" / "Cargo.toml"
manifest.write_text(manifest.read_text().replace('version = "0.1.0"', 'version = "0.1.1"'))
lock_before = lock.read_bytes()
locked = run("locked", command, success=False)
assert lock.read_bytes() == lock_before, "native cargo rewrote a stale lockfile"
assert "locked" in locked.stderr.lower(), f"missing locked diagnostic: {locked.stderr}"
assert not locked.stdout, "program started with a stale lockfile"
print("PASS locked: changed graph fails without rewriting Cargo.lock or starting program")
