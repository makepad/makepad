#!/usr/bin/env python3
"""Measure compose_partial on 3,000 lines using the release makepad binary argument.

Run from libs/relax/compiler with MAKEPAD_RUN_STD=$PWD/std MAKEPAD_RUN_CACHE=0. No build or download.
"""
import os
import pathlib
import re
import statistics
import subprocess
import sys
import tempfile


def main():
    with tempfile.TemporaryDirectory(prefix="relax-stream-bench-") as tmp:
        root = pathlib.Path(tmp)
        # Many separate items exercise identity matching, rather than one large body.
        items = "".join(f"fn f{i}() -> i32 {{\n    {i}\n}}\n" for i in range(999))
        source = items + '#[test]\nfn run() {\n    println!("out {}", f0()); }\n'
        assert len(source.splitlines()) == 3000
        initial = root / "v0.rs"
        initial.write_text(source)
        versions = []
        for i in range(1, 41):
            path = root / f"v{i}.rs"
            # The first item changes; the final item has only its header so far.
            path.write_text(source.replace("    0\n", f"    {i}\n", 1).split("#[test]")[0] + "#[test]\nfn ru")
            versions.append(str(path))
        env = dict(os.environ, MAKEPAD_RUN_CACHE="0", MAKEPAD_RUN_STREAM_TIMING="1")
        run = subprocess.run([sys.argv[1], "live", str(initial), *versions],
                             env=env, text=True, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, timeout=180)
        costs = [float(x) for x in re.findall(r"makepad-run stream: compose_partial \d+ bytes ([\d.]+) ms", run.stdout)]
        if run.returncode or len(costs) != len(versions) or "rung=refuse" in run.stdout or "rung=restart" in run.stdout:
            sys.exit(run.stdout)
        ordered = sorted(costs)
        print(f"compose_partial: 3,000 lines, {len(costs)} deltas; "
              f"median {statistics.median(costs):.3f} ms, "
              f"p95 {ordered[round((len(costs)-1)*.95)]:.3f} ms, max {max(costs):.3f} ms")


if __name__ == "__main__":
    main()
