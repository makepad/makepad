#!/usr/bin/env python3
"""verify.py [<makepad binary> <scratch directory>]: without them (CI, from the workspace root) it
builds this tree's makepad and works in a scratch dir under its target folder.

C code (cdemo's static shim) calls a shared library that the build script names with
`rustc-link-lib=dylib=demo` in a `rustc-link-search` folder outside the dynamic loader's search
path, as an SDK installed outside it is. The program's own link arguments (here an rpath in
RUSTFLAGS) are what let the binary find it. `makepad run --allow-c` must do what the rustc-built
binary does, in the same order of runs, with one native cache:
  no rpath: both fail to load the library (no "no #[no_mangle] fn" callback error);
  loader path: right after, nothing else changed, the loader's path names the folder: the
            native library that failed to load is not reused but linked again, and runs;
  rpath:    both print the answer (the link arguments reach libnative and its key, and the
            callback lookup finds demo_answer in the search folder);
  warm:     the rpath run again reuses the native build;
  cold:     with the native library gone and cargo's units fresh, the link arguments are read again.
Uses cargo and the system C toolchain; no registry dependency.
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
    subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad"], check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve()
    binary = str(target / "host" / "makepad")
    scratch = Path(tempfile.mkdtemp(prefix=Path(__file__).parent.name + "-", dir=target))
    atexit.register(shutil.rmtree, scratch, True)
fixture = scratch / "fixture"
shutil.copytree(Path(__file__).parent, fixture,
                ignore=shutil.ignore_patterns("target", "Cargo.lock", "__pycache__"))
sdk = scratch / "sdk" / "lib"
loader_path = "DYLD_LIBRARY_PATH" if sys.platform == "darwin" else "LD_LIBRARY_PATH"
base = {k: v for k, v in os.environ.items() if k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", loader_path)}
base.update(CARGO_INCREMENTAL="0", CARGO_NET_OFFLINE="true", DEMO_SDK_DIR=str(sdk),
            CARGO_TARGET_DIR=str(scratch / "cargo"), MAKEPAD_RUN_CACHE_DIR=str(scratch / "jit"))
rpath = {"RUSTFLAGS": f"-C link-arg=-Wl,-rpath,{sdk}"}
library = "libdemo.dylib" if sys.platform == "darwin" else "libdemo.so"


def run(label, args, extra=None, success=True):
    env = dict(base, **(extra or {}))
    result = subprocess.run(args, cwd=fixture, env=env, capture_output=True, text=True)
    (scratch / (label + ".stdout")).write_text(result.stdout)
    (scratch / (label + ".stderr")).write_text(result.stderr)
    assert (result.returncode == 0) == success, (
        f"{label}: exit {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


run("lock", ["cargo", "generate-lockfile", "--offline"])
command = [binary, "run", "--allow-c", "allow-c-dylib-fixture"]

# rustc's verdicts: the binary finds the library through the rpath alone
cargo_rpath = run("cargo-rpath", ["cargo", "run", "--release", "--locked", "--quiet"], rpath)
assert cargo_rpath.stdout == "answer 42\n", cargo_rpath.stdout
cargo_bare = run("cargo-bare", ["cargo", "run", "--release", "--locked", "--quiet"], success=False)
assert "libdemo" in cargo_bare.stderr, f"the rustc-built binary's loader error: {cargo_bare.stderr}"

bare = run("no-rpath", command, success=False)
assert not bare.stdout, f"the program ran without the library: {bare.stdout}"
assert "libdemo" in bare.stderr and "no `#[no_mangle]` fn" not in bare.stderr, (
    f"a library the build script's search folder holds is the loader's error, not a callback: {bare.stderr}")
print("PASS no rpath: the native library cannot load libdemo, as the rustc-built binary cannot")

loader = run("loader-path", command, {loader_path: str(sdk)})
assert loader.stdout == cargo_rpath.stdout, f"the no-rpath build with the loader's path set: {loader.stderr}"
assert "building native dependencies with cargo" in loader.stderr, (
    f"the native library that failed to load was used again: {loader.stderr}")
print("PASS loader path: the native library that failed to load is linked again, and runs")

with_rpath = run("rpath", command, rpath)
assert with_rpath.stdout == cargo_rpath.stdout, f"with the rpath: {with_rpath.stdout}\n{with_rpath.stderr}"
print("PASS rpath: the program's link arguments reach the native library (after a build without them)")

warm = run("warm", command, rpath)
assert warm.stdout == cargo_rpath.stdout, warm.stderr
assert "building native dependencies with cargo" not in warm.stderr, "the same link arguments built again"
print("PASS warm: the same link arguments reuse the native build")

native = scratch / "jit" / "native"
gone = [p for p in native.glob("*/libnative.*") if p.is_file()]
assert gone, f"no native library under {native}"
for p in gone:
    p.unlink()
cold = run("cold", command, rpath)
assert cold.stdout == cargo_rpath.stdout, f"rebuilt with cargo's units fresh: {cold.stdout}\n{cold.stderr}"
assert "building native dependencies with cargo" in cold.stderr, cold.stderr
print("PASS cold: a native build over fresh cargo units links with the program's link arguments")

