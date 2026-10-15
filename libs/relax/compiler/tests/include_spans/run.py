#!/usr/bin/env python3
"""Compare included proc-macro spans with rustc: run.py --makepad /path/to/makepad.

Only local fixture crates are built. Cargo inherits the caller's target directory.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--makepad", required=True)
args = parser.parse_args()
here = Path(__file__).resolve().parent
relax = str(Path(args.makepad).resolve())
env = dict(os.environ, CARGO_NET_OFFLINE="true", CARGO_INCREMENTAL="0", MAKEPAD_RUN_OFFLINE="1",
           MAKEPAD_RUN_CACHE="off", MAKEPAD_RUN_STD=str(here.parent.parent / "std"))

def run(command, **kwargs):
    result = subprocess.run(command, cwd=here, env=env, text=True, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{command}: exit {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result.stdout

with tempfile.TemporaryDirectory(prefix="include-spans-") as tmp:
    env["MAKEPAD_RUN_HOME"] = tmp + "/home"
    env["MAKEPAD_RUN_CACHE_DIR"] = tmp + "/cache"
    env.setdefault("CARGO_TARGET_DIR", tmp + "/target")
    run(["nice", "-n", "10", "cargo", "build", "--release", "--offline", "-j", "4", "-p", "include-spans-app"])
    expected = run([str(Path(env["CARGO_TARGET_DIR"]) / "release" / "include-spans-app")])
    for mode in ["uncached", "cold", "warm"]:
        env["MAKEPAD_RUN_CACHE"] = "off" if mode == "uncached" else "on"
        actual = run(["nice", "-n", "10", relax, "run", "--quiet", "include-spans-app"])
        if actual != expected:
            raise AssertionError(f"{mode}: rustc:\n{expected}\nRelax:\n{actual}")
        print(f"include_spans: {mode} matches rustc")
    print(actual, end="")
