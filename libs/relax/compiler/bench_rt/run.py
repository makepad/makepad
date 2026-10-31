#!/usr/bin/env python3
"""Runtime ratio table: each kernel of src/main.rs under rustc --release and under `makepad run`,
one process per kernel (a compile failure in one cannot hide the others), best of ROUNDS runs
of each process's best-of-5. Usage: run.py <makepad binary> <target dir> [kernel ...]
Env: O3_PROFILE_DIR (steady runs replay a recorded O3 profile per kernel: MAKEPAD_RUN_O3_PROFILE), ROUNDS (default 2), STEADY_MS (also time after warming each kernel up for this long:
the steady state once Relax's O3 has settled; default 2000, 0 = off), NATIVE (prebuilt rustc binary of this crate), MAKEPAD_RUN_OPT etc. pass through to Relax."""
import os, re, subprocess, sys, platform

here = os.path.dirname(os.path.abspath(__file__))
relax, target = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
rounds = int(os.environ.get("ROUNDS", "2"))
src = open(os.path.join(here, "src/main.rs")).read()
names = re.findall(r'\("(\w+)", \w+\),', src)
if len(sys.argv) > 3:
    names = sys.argv[3:]
nice = ["nice", "-n", "10"]
env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=os.path.join(target, "rustc"))
# NATIVE=<binary>: a rustc build made elsewhere (bench machines run no cargo builds)
native = os.environ.get("NATIVE")
if not native:
    subprocess.run(nice + ["cargo", "build", "--release", "-q", "-j", "4"], cwd=here, env=env, check=True)
    native = os.path.join(target, "rustc/release/relax-bench-rt")
std = os.environ.get("MAKEPAD_RUN_STD", os.path.join(here, "../std"))
os.environ.setdefault("BENCH_FONT", os.path.normpath(os.path.join(here, "../../../../widgets/resources/IBMPlexSans-Text.ttf")))
renv = dict(os.environ, MAKEPAD_RUN_STD=std)

def one(cmd, e, name):
    best = None
    for _ in range(rounds):
        try:
            out = subprocess.run(cmd, cwd=here, env=e, capture_output=True, text=True, timeout=300)
        except subprocess.TimeoutExpired:
            return None, "TIMEOUT (300 s)"
        m = re.search(r"^\| " + name + r" ([\d.]+) (\d+)$", out.stdout, re.M)
        if not m:
            return None, (out.stdout + out.stderr)[-400:]
        cm = re.search(r"cpu typeck (\d+) ms, lower (\d+) ms, opt (\d+) ms, codegen (\d+) ms", out.stderr)
        fm = re.search(r"compiled (\d+) fns, (\d+) KB", out.stderr)
        t = float(m.group(1))
        if best is None or t < best[0]:
            best = (t, m.group(2), cm.groups() if cm else None, fm.groups() if fm else None)
    return best, None

steady_ms = int(os.environ.get("STEADY_MS", "7000"))
print("| kernel | rustc ms | Relax ms | Relax/rustc | steady rustc ms | steady Relax ms | steady Relax/rustc | fns | opt+cg ms | sum ok |")
print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---|")
ratios = []
sratios = []
for n in names:
    a, ea = one(nice + [native, n], os.environ, n)
    runner = relax
    kenv = renv
    b, eb = one(nice + [runner, "run", "--quiet", "relax-bench-rt", "--", n], kenv, n)
    if a is None or b is None:
        print("| %s | %s | %s | ERROR | | | | | | %s |" % (n, a and a[0], b and b[0], (ea or eb or "").replace("\n", " ")[-200:]))
        continue
    r = b[0] / a[0]
    ratios.append(r)
    st = ("", "", "")
    if steady_ms > 0:
        sa, _ = one(nice + [native, n], dict(os.environ, BENCH_WARM_MS=str(steady_ms)), n)
        senv = dict(kenv, BENCH_WARM_MS=str(steady_ms))
        # O3_PROFILE_DIR: steady runs replay one recorded O3 profile per kernel (the first
        # steady run of a kernel without a profile records it), so every compared run (and every
        # binary of an A/B sharing the dir) builds the same hot set from the same inputs; cold rows
        # stay in normal mode
        pdir = os.environ.get("O3_PROFILE_DIR")
        if pdir:
            os.makedirs(pdir, exist_ok=True)
            prof = os.path.join(pdir, n + ".prof")
            senv["MAKEPAD_RUN_O3_PROFILE"] = prof
            if not os.path.exists(prof):
                subprocess.run(nice + [runner, "run", "--quiet", "relax-bench-rt", "--", n], cwd=here, env=senv, capture_output=True, timeout=300)
        sb, _ = one(nice + [runner, "run", "--quiet", "relax-bench-rt", "--", n], senv, n)
        if sa and sb:
            sratios.append(sb[0] / sa[0])
            st = ("%.3f" % sa[0], "%.3f" % sb[0], "%.2f" % (sb[0] / sa[0]))
    cg = "%s+%s" % (b[2][2], b[2][3]) if b[2] else "-"
    print("| %s | %.3f | %.3f | %.2f | %s | %s | %s | %s | %s | %s |" % (n, a[0], b[0], r, st[0], st[1], st[2], b[3][0] if b[3] else "-", cg, "yes" if a[1] == b[1] else "**NO**"), flush=True)
def geo(rs):
    g = 1.0
    for r in rs:
        g *= r
    return g ** (1.0 / len(rs))
if ratios:
    print("\ngeomean Relax/rustc: %.2f (%d kernels, %s)" % (geo(ratios), len(ratios), platform.machine()))
if sratios:
    print("geomean steady Relax/rustc: %.2f (%d kernels, warm-up %d ms)" % (geo(sratios), len(sratios), steady_ms))
