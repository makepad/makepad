#!/usr/bin/env python3
"""Cold startup and SIGINT shutdown regression; run on the test node via x64run.

Pass --makepad, --workspace, --out and the package to run. MAKEPAD_RUN_STD and the
headless display environment come from the caller. Each launch gets a new cache.
Optional --gdb captures only this harness's child, before forcibly stopping it.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time


def permit_debugger():
    # Yama permits the named tracer and its descendants: the harness and its gdb.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(0x59616D61, os.getppid(), 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_SET_PTRACER")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--makepad", required=True, type=Path)
    parser.add_argument("--workspace", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--after-present", type=float, default=4)
    parser.add_argument("--startup-timeout", type=float, default=240)
    parser.add_argument("--shutdown-timeout", type=float, default=15)
    parser.add_argument("--gdb", action="store_true")
    parser.add_argument("package")
    args = parser.parse_args()
    if args.runs < 1 or min(args.startup_timeout, args.shutdown_timeout) <= 0 or args.after_present < 0:
        parser.error("runs and timeouts must be positive; after-present must be nonnegative")
    if args.gdb and sys.platform != "linux":
        parser.error("--gdb requires Linux")
    args.out.mkdir(parents=True, exist_ok=True)
    out = Path(tempfile.mkdtemp(prefix="shutdown-", dir=args.out.resolve()))
    results = []
    for cycle in range(1, args.runs + 1):
        log = out / f"{cycle}.log"
        env = dict(os.environ, MAKEPAD_RUN_CACHE="on", MAKEPAD_RUN_CACHE_DIR=str(out / f"cache-{cycle}"))
        with log.open("w") as stream:
            child = subprocess.Popen(
                [str(args.makepad.resolve()), "run", "--verbose", args.package],
                cwd=args.workspace, env=env, stdout=stream, stderr=subprocess.STDOUT,
                start_new_session=True, preexec_fn=permit_debugger if args.gdb else None,
            )
            print(f"START {cycle} pid={child.pid} log={log}", flush=True)
            started = time.monotonic()
            present = None
            shutdown = None
            forced = False

            def backtrace(phase):
                if args.gdb:
                    with (out / f"{cycle}-{phase}-gdb.log").open("w") as trace:
                        subprocess.run(
                            ["gdb", "-q", "-batch", "-ex", "set pagination off",
                             "-ex", "thread apply all bt", "-ex", "detach", "-p", str(child.pid)],
                            stdout=trace, stderr=subprocess.STDOUT, timeout=60,
                        )

            try:
                while child.poll() is None:
                    now = time.monotonic()
                    if present is None and " first frame at " in log.read_text(errors="replace"):
                        present = now
                    if (present is not None and now - present >= args.after_present) or now - started >= args.startup_timeout:
                        if present is None:
                            backtrace("startup")
                        stopping = time.monotonic()
                        os.killpg(child.pid, signal.SIGINT)
                        try:
                            child.wait(timeout=args.shutdown_timeout)
                        except subprocess.TimeoutExpired:
                            forced = True
                            backtrace("shutdown")
                        shutdown = time.monotonic() - stopping
                        break
                    time.sleep(0.1)
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
        text = log.read_text(errors="replace")
        first = re.search(r"first frame at ([\d.]+) s", text)
        row = dict(cycle=cycle, pid=child.pid, exit=child.returncode, sigkill=forced,
                   first_frame_s=float(first[1]) if first else None, shutdown_s=shutdown,
                   panics=text.count("panicked at"), compile_errors=text.count("compile error in"), log=str(log))
        results.append(row)
        (out / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(row), flush=True)
        if not first or shutdown is None or forced or child.returncode != 0 or row["compile_errors"]:
            return 1
    print(f"PASS: {args.runs} cold launches and SIGINT shutdowns; evidence {out}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
