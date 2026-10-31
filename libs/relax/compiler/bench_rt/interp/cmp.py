#!/usr/bin/env python3
"""Interpreter vs JIT vs rustc on bench_rt kernels (ios-interp research prototype).
Usage: cmp.py <makepad binary> <rustc bench binary> [kernel ...]; ROUNDS (default 3) interleaved rounds,
best per mode. Modes: rustc; jit-o1 (MAKEPAD_RUN_O3=0 MAKEPAD_RUN_O1=0); jit-o3 (default tiers, BENCH_WARM_MS=2500);
interp (MAKEPAD_RUN_INTERP=all, O3/O1 off). Prints a markdown table of best ms and ratios, checksums compared."""
import os, re, subprocess, sys
here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
mk, native = sys.argv[1], sys.argv[2]
src = open(os.path.join(here, "src/main.rs")).read()
names = sys.argv[3:] or re.findall(r'\("(\w+)", \w+\),', src)
rounds = int(os.environ.get("ROUNDS", "3"))
base = dict(os.environ, MAKEPAD_RUN_STD=os.path.join(here, "../std"), MAKEPAD_RUN_CACHE="off",
            BENCH_FONT=os.path.normpath(os.path.join(here, "../../../../widgets/resources/IBMPlexSans-Text.ttf")))
modes = {
    "rustc": None,
    "jit-o1": dict(MAKEPAD_RUN_O3="0", MAKEPAD_RUN_O1="0"),
    "jit-o3": dict(BENCH_WARM_MS="2500"),
    "interp": dict(MAKEPAD_RUN_INTERP="all", MAKEPAD_RUN_INTERP_STATS="1", MAKEPAD_RUN_O3="0", MAKEPAD_RUN_O1="0"),
    # no machine code at all: precompiled glue, every function interpreted
    "nocodegen": dict(MAKEPAD_RUN_NO_CODEGEN="1", MAKEPAD_RUN_INTERP_STATS="1"),
    # A/B of the lowering's own clean-ups: no constant-branch folding and no loop-constant preload
    "interp-plain": dict(MAKEPAD_RUN_INTERP="all", MAKEPAD_RUN_INTERP_STATS="1", MAKEPAD_RUN_O3="0", MAKEPAD_RUN_O1="0", MAKEPAD_RUN_INTERP_NOFOLD="1"),
}
want = os.environ.get("MODES")
if want:
    modes = {k: v for k, v in modes.items() if k in want.split(",")}
else:
    modes.pop("interp-plain")
    modes.pop("nocodegen")
def one(mode, n):
    if modes[mode] is None:
        cmd, env = ["nice", "-n", "5", native, n], base
    else:
        cmd, env = ["nice", "-n", "5", mk, "run", "--quiet", "relax-bench-rt", "--", n], dict(base, **modes[mode])
    try:
        out = subprocess.run(cmd, cwd=here, env=env, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return None, None, "timeout"
    m = re.search(r"^\| " + n + r" ([\d.]+) (\d+)$", out.stdout, re.M)
    st = re.search(r"makepad-run interp: (\d+) fns interpreted .*?, (\d+) stayed native(.*)", out.stderr)
    if not m:
        return None, None, (out.stdout + out.stderr)[-300:].replace("\n", " ")
    return float(m.group(1)), m.group(2), (st.group(0) if st else "")
res = {}
for r in range(rounds):
    for n in names:
        for mode in modes:
            t, ck, note = one(mode, n)
            k = (n, mode)
            if t is None:
                res.setdefault(k, (None, None, note))
                print("#", n, mode, "FAILED", note, flush=True)
                continue
            old = res.get(k)
            if old is None or old[0] is None or t < old[0]:
                res[k] = (t, ck, note)
        print("# round", r, n, {m: res.get((n, m), (None,))[0] for m in modes}, flush=True)
cols = list(modes)
print("| kernel | " + " | ".join(c + " ms" for c in cols) + " | interp/jit-o1 | interp/jit-o3 | interp/rustc | sums | interp coverage |")
print("|---" * (len(cols) + 6) + "|")
for n in names:
    row = [res.get((n, c), (None, None, ""))for c in cols]
    t = {c: res.get((n, c), (None,))[0] for c in cols}
    def ratio(a, b):
        return "%.2f" % (t[a] / t[b]) if t.get(a) and t.get(b) else "-"
    sums = {r[1] for r in row if r[1]}
    cov = res.get((n, "interp"), (None, None, ""))[2]
    cov = re.sub(r"makepad-run interp: ", "", cov or "")[:160]
    print("| %s | %s | %s | %s | %s | %s | %s |" % (n, " | ".join("%.3f" % x[0] if x[0] else "FAIL" for x in row), ratio("interp", "jit-o1"), ratio("interp", "jit-o3"), ratio("interp", "rustc"), "ok" if len(sums) == 1 else "DIFF", cov))
