#!/usr/bin/env python3
"""Drop order of panics through interpreted / JIT / native frames, against rustc.

usage: unwind_cmp.py <makepad binary> [test file ...]
Default test files: the interp_unwind*.rs fixtures and the existing drop/unwind fixtures (relative to
libs/relax/compiler); files named *_abort_* must abort the process everywhere.
tests/interp_unwind_abort_ffi.rs: a panic leaving an `extern "C"` body that C called (qsort's
comparator) aborts, as under rustc (lower.rs abort_unwinding_calls). Each file is built with
`rustc --test` and run (the reference), then run by `makepad test` with the JIT and with the
interpreter in several mixes; the "| " lines must be identical (compared as sorted sets per run,
since test threads may interleave). Exit status 1 on any difference.
"""
import os, subprocess, sys, tempfile

here = os.path.dirname(os.path.abspath(__file__))
comp = os.path.normpath(os.path.join(here, "../.."))
mk = os.path.abspath(sys.argv[1])
files = sys.argv[2:] or ["tests/interp_unwind.rs", "tests/interp_unwind_abort_cleanup.rs", "tests/interp_unwind_abort_ffi.rs", "tests/unwind_drops.rs", "tests/drop_order_scopes.rs", "tests/let_chain_drops.rs", "tests/jump_temp_drops.rs", "tests/drop_elab_flags.rs", "tests/thread_panic.rs", "tests/unwind_nested_catch_restore.rs"]
MODES = [
    ("jit", {}),
    ("interp-all", {"MAKEPAD_RUN_INTERP": "all"}),
    ("interp-xi", {"MAKEPAD_RUN_INTERP": "::xi_"}),
    ("interp-xj", {"MAKEPAD_RUN_INTERP": "::xj_,{closure"}),
    ("interp-std", {"MAKEPAD_RUN_INTERP": "std::,core::,alloc::"}),
    ("nocodegen", {"MAKEPAD_RUN_NO_CODEGEN": "1"}),
]
if os.environ.get("MODES"):
    MODES = [m for m in MODES if m[0] in os.environ["MODES"].split(",")]

def lines(text):
    out = []
    for l in text.splitlines():
        k = l.find("| ")
        if k >= 0 and not l.lstrip().startswith("report:"):
            out.append(l[k:].rstrip())
    return sorted(out)

bad = 0
tmp = tempfile.mkdtemp()
for f in files:
    path = os.path.join(comp, f)
    exe = os.path.join(tmp, os.path.basename(f)[:-3])
    r = subprocess.run(["rustc", "--edition", "2021", "-O", "--test", path, "-o", exe], capture_output=True, text=True)
    if r.returncode != 0 and "Rust 2024" in r.stderr:
        r = subprocess.run(["rustc", "--edition", "2024", "-O", "--test", path, "-o", exe], capture_output=True, text=True)
    if r.returncode != 0:
        print("rustc failed for", f, r.stderr[-600:])
        bad += 1
        continue
    rr = subprocess.run([exe, "--test-threads=1", "--nocapture"], capture_output=True, text=True)
    ref = lines(rr.stdout)
    # files named *_abort_*: the process must abort (SIGABRT) everywhere, without "| " lines
    aborts = "_abort_" in os.path.basename(f)
    if aborts and rr.returncode != -6:
        print("%-42s rustc did not abort (rc %d)" % (f, rr.returncode))
        bad += 1
    for name, env in MODES:
        e = dict(os.environ, MAKEPAD_RUN_CACHE="off", **env)
        p = subprocess.run(["nice", "-n", "5", mk, "test", path], cwd=comp, env=e, capture_output=True, text=True, timeout=900)
        got = lines(p.stdout + p.stderr)
        ok = got == ref and (p.returncode in (-6, 134) if aborts else p.returncode == 0)
        print("%-42s %-11s %s (%d lines, rc %d)" % (f, name, "SAME" if ok else "DIFF", len(got), p.returncode), flush=True)
        if not ok:
            bad += 1
            for l in sorted(set(ref) - set(got)):
                print("    rustc only:", l)
            for l in sorted(set(got) - set(ref)):
                print("    run only:  ", l)
            if p.returncode != 0:
                print("    tail:", (p.stdout + p.stderr)[-800:].replace("\n", "\n          "))
sys.exit(1 if bad else 0)
