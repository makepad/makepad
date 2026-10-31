#!/usr/bin/env python3
"""Panic unwinding through frames with drop guards, in every mode (rustc runs each frame's drops,
innermost first, on the way to the catch):

  plain            `makepad run`, the panic unwinds (panic and index-out-of-bounds)
  park-panic       the panic parks for `makepad inspect`; `resume P1 panic` unwinds it
  park-restart     `restartable work`; the parked panic is resumed `restart` (the fn runs again)
  park-return      `restartable work`; resumed `return 4242` (the call returns the value)
  (live sessions: live/unwind_guards, run by run_cases.py; `makepad test` recovery: unwind_nested_catch_restore.rs)

usage: unwind_modes.py --makepad <makepad binary> [--workspace <dir>]
Output compared: the lines starting with "out ".
"""
import json, os, subprocess, sys, tempfile, time

PKG = ["relax-provided-fixture", "--bin", "relax-unwind-probe"]

def args():
    a = {"relax": None, "ws": os.getcwd()}
    v = sys.argv[1:]
    i = 0
    while i < len(v):
        if v[i] == "--makepad": a["relax"] = os.path.abspath(v[i + 1]); i += 2
        elif v[i] == "--workspace": a["ws"] = v[i + 1]; i += 2
        else: sys.exit(__doc__)
    if not a["relax"]: sys.exit(__doc__)
    return a

def outs(text):
    return [l for l in text.splitlines() if l.startswith("out ")]

DROPS = ["leaf", "mid-box", "mid-a", "parked-body"]
def unwound():
    return ["out drop " + d for d in DROPS] + ["out restore", "out drop parked-guard", "out drop top", "out caught true", "out drop outer", "out done"]

EXPECT = {
    "panic": unwound(),
    "oob": unwound(),
    "park-panic": unwound(),
    "park-restart": ["out work attempt 1", "out drop leaf", "out drop mid-box", "out drop mid-a", "out drop work-inner", "out drop work",
                     "out work attempt 2", "out drop work", "out work result Ok(1007)", "out drop outer", "out done"],
    "park-return": ["out work attempt 1", "out drop leaf", "out drop mid-box", "out drop mid-a", "out drop work-inner", "out drop work",
                    "out work result Ok(4242)", "out drop outer", "out done"],
}

def inspect(relax, pid, *cmd):
    r = subprocess.run([relax, "inspect", str(pid), *cmd, "as=json"], capture_output=True, text=True, timeout=60)
    return r.stdout + r.stderr

def run_plain(a, mode):
    r = subprocess.run([a["relax"], "run", *PKG, "--", mode], cwd=a["ws"], capture_output=True, text=True, timeout=240)
    return outs(r.stdout)

def run_parked(a, mode):
    tmp = tempfile.mkdtemp()
    ready, go = os.path.join(tmp, "ready"), os.path.join(tmp, "go")
    env = dict(os.environ, UNWIND_PROBE_READY=ready, UNWIND_PROBE_GO=go, MAKEPAD_RUN_PANIC_PARK="120")
    probe_mode = {"park-panic": "panic", "park-restart": "restart", "park-return": "return"}[mode]
    p = subprocess.Popen([a["relax"], "run", "--agent", *PKG, "--", probe_mode], cwd=a["ws"], env=env,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        t0 = time.time()
        while not os.path.exists(ready):
            if p.poll() is not None or time.time() - t0 > 200:
                raise RuntimeError("probe did not start: " + (p.stderr.read() if p.poll() is not None else "timeout"))
            time.sleep(0.2)
        if mode != "park-panic":
            inspect(a["relax"], p.pid, "restartable", "work")
        open(go, "w").write("go")
        t0 = time.time()
        while "P1" not in inspect(a["relax"], p.pid, "panics"):
            if time.time() - t0 > 60: raise RuntimeError("no parked panic")
            time.sleep(0.3)
        how = {"park-panic": ["panic"], "park-restart": ["restart"], "park-return": ["return", "4242"]}[mode]
        inspect(a["relax"], p.pid, "resume", "P1", *how)
        out, _ = p.communicate(timeout=60)
        return outs(out)
    finally:
        if p.poll() is None:
            p.kill()

def main():
    a = args()
    bad = 0
    for mode in ["panic", "oob", "park-panic", "park-restart", "park-return"]:
        try:
            got = run_plain(a, mode) if not mode.startswith("park") else run_parked(a, mode)
        except Exception as e:
            got = ["error: %s" % e]
        ok = got == EXPECT[mode]
        print(("PASS " if ok else "FAIL ") + mode)
        if not ok:
            bad += 1
            print("  want:", EXPECT[mode]); print("  got: ", got)
    sys.exit(1 if bad else 0)

main()
