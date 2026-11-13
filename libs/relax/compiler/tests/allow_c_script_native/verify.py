#!/usr/bin/env python3
"""verify.py [<makepad binary> <scratch directory>]: without them (CI, from the workspace root) it
builds this tree's makepad and works in a scratch dir under its target folder.

`helper`'s build script calls into `cnative`, a path crate it build-depends on whose own script
compiles C. The script runs as its own `makepad run --units` child, whose unit graph is a part
of the run's: it cannot compute the run's native key, so it reads the native build's outputs
(cfgs, env, OUT_DIR) and library from the receipt the run hands it. Without that the child
refuses `cnative` as C only `--allow-c` builds, and the script fails.

The receipt's name is its content: a warm run hands the script the same name, so the script's
rerun key holds and it does not run again (it counts its runs in its OUT_DIR).

Uses the offline C fixture, cargo and the system C toolchain. Keeps outputs in scratch for
inspection.
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
           RELAX_CACHE_DIR=str(scratch / "jit"))
expected = "answer 42 built true runs 1\n"


def run(label, args, success=True):
    result = subprocess.run(args, cwd=fixture, env=env, capture_output=True, text=True)
    (scratch / (label + ".stdout")).write_text(result.stdout)
    (scratch / (label + ".stderr")).write_text(result.stderr)
    assert (result.returncode == 0) == success, (
        f"{label}: exit {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


run("lock", ["cargo", "generate-lockfile", "--offline"])

# Cold: cargo builds cnative first, and the script's child run calls into its library.
cold = run("run", [binary, "run", "--allow-c", "script-native-fixture"])
assert "building native dependencies with cargo" in cold.stderr, "cold native build was not exercised"
assert cold.stdout == expected, cold.stdout
print("PASS cold: a build script's child run calls the C its build-dependency built, with its cfg")

# A check without the flag hands the script the receipt of the cached build: the same one.
check = run("check", [binary, "check", "script-native-fixture"])
assert "building native dependencies with cargo" not in check.stderr, "the check built native code"
assert "0 errors" in check.stderr, check.stderr
print("PASS check: a flagless check runs the script from the cached native build")

# Warm: no cargo, and the script's output stands (one receipt, named by its content).
warm = run("run-warm", [binary, "run", "--allow-c", "script-native-fixture"])
assert "building native dependencies with cargo" not in warm.stderr, "the warm run rebuilt native code"
assert warm.stdout == expected, f"the script ran again:\n{warm.stdout}"
receipts = sorted(p.name for p in scratch.rglob("native-scripts-*.json"))
assert len(receipts) == 1, f"expected one receipt, got {receipts}"
print("PASS warm: the script does not run again and the run, check and warm run share one receipt")
