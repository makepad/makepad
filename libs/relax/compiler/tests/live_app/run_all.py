#!/usr/bin/env python3
"""live_app's scripts under one binary: run.py (the live driver end to end), startup.py, progress.py
and session_log.py. Exit 1 when any fails.

usage: run_all.py [<relax or makepad binary>]   (without one, from the workspace root: relax-runner's
relax, built by cargo)
"""
import os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
if len(sys.argv) > 1:
    binary = os.path.abspath(sys.argv[1])
else:
    subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "relax-runner", "--bin", "relax"], check=True)
    binary = os.path.abspath(os.path.join(os.environ.get("CARGO_TARGET_DIR", "target"), "host", "relax"))
failed = []
for script, args in [("run.py", ["--makepad", binary]), ("startup.py", [binary]), ("progress.py", [binary]), ("session_log.py", [binary])]:
    t = time.time()
    r = subprocess.run([sys.executable, os.path.join(HERE, script)] + args, capture_output=True, text=True)
    print("%s %s (%.1f s)" % ("ok" if r.returncode == 0 else "FAILED", script, time.time() - t), flush=True)
    if r.returncode != 0:
        failed.append(script)
        print((r.stdout + r.stderr)[-4000:], flush=True)
print("live_app: %s" % ("ok" if not failed else "FAILED (%s)" % ", ".join(failed)))
sys.exit(1 if failed else 0)
