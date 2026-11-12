#!/usr/bin/env python3
"""verify.py [<makepad binary> <scratch directory>]: without them (CI, from the workspace root) it
builds this tree's makepad and works in a scratch dir under its target folder.

A path crate whose build script compiles C and writes an OUT_DIR file its lib includes: the
shape of a -sys crate's generated bindings. A `--allow-c` run writes the native build and its
receipt; a later `makepad check` without the flag reads that receipt's cfgs, env and OUT_DIR, so
the crate type-checks whole instead of losing the included type and every signature naming it.

Uses the offline C fixture, cargo and the system C toolchain. Keeps outputs in scratch for
inspection; no fixture source or workspace is rewritten.
"""
import atexit
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

if len(sys.argv) > 2:
    binary = str(Path(sys.argv[1]).resolve())
    scratch = Path(sys.argv[2]).resolve()
    scratch.mkdir(parents=True, exist_ok=True)
else:
    # CI, from the workspace root: this tree's makepad, and a scratch dir under its target folder
    # that goes with the run
    subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad"], check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve()
    binary = str(target / "host" / "makepad")
    scratch = Path(tempfile.mkdtemp(prefix=Path(__file__).parent.name + "-", dir=target))
    atexit.register(shutil.rmtree, scratch, True)
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


run("lock", ["cargo", "generate-lockfile", "--offline"])
baseline = run("cargo", ["cargo", "run", "--release", "--locked", "--quiet"])
assert "answer=42 generated=7" in baseline.stdout, baseline.stdout

# Cold: no native build yet, so the OUT_DIR file is unknown and the check says so. The
# `include!` failure is the one a -sys crate's bindings report as dozens of unresolved imports.
cold = run("check-cold", [binary, "check", "cached-check-fixture"], success=False)
assert "not run without `--allow-c`" in cold.stderr, cold.stderr
assert "gen.rs" in cold.stderr, cold.stderr
print("PASS cold: a check with no native build reports the C script and the missing OUT_DIR")

# One --allow-c run writes the native build (and the receipt the check reads) into the cache.
first = run("run", [binary, "run", "--allow-c", "cached-check-fixture"])
assert first.stdout == baseline.stdout, "native/JIT fixture output differs from cargo"
assert "building native dependencies with cargo" in first.stderr, "cold native build was not exercised"
receipts = sorted(p.parent.name for p in (scratch / "jit" / "native").glob("*/messages.jsonl"))
assert len(receipts) == 1, f"expected one native key, got {receipts}"

# The check without the flag reads the same key's script outputs: cfgs, env, OUT_DIR.
cached = run("check", [binary, "check", "cached-check-fixture"])
assert "building native dependencies with cargo" not in cached.stderr, "the check built native code"
assert "0 errors" in cached.stderr, cached.stderr
assert "gen.rs" not in cached.stderr, "the check did not resolve the OUT_DIR include"
assert "cannot find type `Generated`" not in cached.stderr, cached.stderr
print("PASS cached check: a warm --allow-c build makes the flagless check type-check whole")

# Dropping the cache returns the cold verdict: the check is reading the receipt, not guessing.
shutil.rmtree(scratch / "jit")
again = run("check-no-cache", [binary, "check", "cached-check-fixture"], success=False)
assert "gen.rs" in again.stderr, again.stderr
print("PASS no cache: with the receipt gone the check reports the missing OUT_DIR again")
