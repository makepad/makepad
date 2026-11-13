#!/usr/bin/env python3
"""Cold-process load+parse measurements; persistent Relax caches are disabled."""
import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("label")
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--std", type=Path, required=True)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--jobs", type=int, nargs="+", default=[16], help="worker counts; 0 tests the default with RELAX_JOBS unset")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--package", default="makepad-runner")
    parser.add_argument("--profile", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, RELAX_CACHE="off", RELAX_OFFLINE="1",
               RUSTC_BOOTSTRAP="1",
               RELAX_CACHE_DIR=str(args.target.resolve()), RELAX_STD=str(args.std.resolve()))
    for key in ("RELAX_PROFILE", "RELAX_BOOT_TRACE"):
        env.pop(key, None)
    if args.profile:
        env["RELAX_PROFILE"] = "1"
    results = []
    for jobs in args.jobs:
        if jobs == 0:
            env.pop("RELAX_JOBS", None)
        else:
            env["RELAX_JOBS"] = str(jobs)
        for run in range(1, args.runs + 1):
            log = args.output / f"{args.label}-{jobs}-{run}.log"
            start = time.monotonic()
            with log.open("w") as out:
                result = subprocess.run([str(args.binary.resolve()), "check", "--no-serve", args.package],
                                        cwd=args.workspace, env=env, stdout=out, stderr=subprocess.STDOUT)
            data = log.read_text()
            load = re.search(r"load\+parse[^\n]*?\s([\d.]+) ms", data)
            errors = re.search(r"relax check: [^\n]* (\d+) errors", data)
            row = dict(run=run, jobs=jobs, load_ms=float(load[1]) if load else None,
                       errors=int(errors[1]) if errors else None, exit=result.returncode,
                       wall_s=round(time.monotonic() - start, 2), log=str(log))
            results.append(row)
            (args.output / f"{args.label}.json").write_text(json.dumps(results, indent=2) + "\n")
            print(json.dumps(row), flush=True)
            if result.returncode or row["errors"] != 0 or row["load_ms"] is None:
                raise SystemExit("check gate failed; inspect " + str(log))
        median = statistics.median(r["load_ms"] for r in results if r["jobs"] == jobs)
        print(f"{args.label}: jobs={jobs} median={median:.1f} ms", flush=True)


if __name__ == "__main__":
    main()
