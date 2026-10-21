#!/usr/bin/env python3
"""Real-world benchmark table / A-B: each workload of src/main.rs under rustc --release and under one or
more `makepad run` binaries, one process per workload per side per round.
Usage: run.py <target dir> <name=makepad binary>...      (a bare path means name "relax")
The first name is the base. Rounds are interleaved: each round runs every workload once, rustc first and
then the binaries in rotating order (round 1: base, new; round 2: new, base; ...), so drift on a shared
machine hits all sides alike. Per name and workload: median over rounds of each round's steady ms (spread =
min..max), the ratio to the rustc run of the same round (median of per-round ratios, spread), median
time-to-steady, Relax's O3 coverage line, checksums equal across all runs. With more than one name a final
A/B table gives new/base per workload (median of per-round ratios, spread) and the geomean of the medians.
The commercial set (apps/commercial/relax_tests/src/bench.rs) joins when present; RW_COMMERCIAL=0 skips it,
NATIVE_COMMERCIAL names its prebuilt rustc binary.
Env: WARM=1 (O3/code disk cache on, one dir per name, one untimed fill run per workload and name before the timed rounds; the O3 column then shows hot / installed / from the disk cache), ROUNDS (default 4), RW_MS (per-process iteration budget ms, default 4000), WORKLOADS ("a b": only those),
MAKEPAD_RUN_STD_<NAME> (that name's own std tree, e.g. its compiler tree's std; a Relax change that touches std must be measured with its own std), NATIVE (prebuilt rustc binary; else built here with cargo --release into <target dir>/rustc), MAKEPAD_RUN_STD
(default ../std), RUN_PREFIX (default "nice -n 10", "none" on Windows), RW_ROOT (the tree holding the inputs).
O3PROF=1 records an untimed profile per name/workload in <target>/o3-profiles, then replays it.
MAKEPAD_RUN_ENV_<NAME> supplies shell-quoted KEY=value overrides for that name.
Timing belongs on a quiet machine; the spreads show how quiet it was."""
import os, re, subprocess, sys, shlex, time, statistics
from pathlib import Path

here = os.path.dirname(os.path.abspath(__file__))
target = os.path.abspath(sys.argv[1])
bins = []
for a in sys.argv[2:]:
    n, _, p = a.partition("=") if "=" in a else ("relax", "", a)
    bins.append((n, os.path.abspath(p)))
assert bins, "no makepad binary given"
rounds = int(os.environ.get("ROUNDS", "4"))
src = open(os.path.join(here, "src/main.rs")).read()
names = re.findall(r'\("(\w+)", \w+::\w+\),', src)
# The commercial set: apps/commercial/relax_tests (package relax-commercial-tests, binary
# relax-rwbench-commercial, the same harness) joins when that checkout is present (RW_COMMERCIAL=0 skips it).
# Its workloads run from their own binaries; everything else (rounds, tables, checksums) is shared.
tree = os.environ.get("RW_ROOT", os.path.abspath(os.path.join(here, "../../../..")))
cbench = os.path.join(tree, "apps/commercial/relax_tests/src/bench.rs")
CPKG, CBIN = "relax-commercial-tests", "relax-rwbench-commercial"
cnames = []
if os.environ.get("RW_COMMERCIAL", "1") != "0" and os.path.isfile(cbench):
    cnames = [n for n in re.findall(r'\("(\w+)", [\w:]+\),', open(cbench).read()) if n not in names]
names += cnames
if os.environ.get("WORKLOADS"):
    names = os.environ["WORKLOADS"].split()
cnames = [n for n in names if n in cnames]
pf = os.environ.get("RUN_PREFIX", "nice -n 10" if os.name != "nt" else "none")
prefix = [] if pf in ("", "none") else shlex.split(os.path.expandvars(pf))
exe = ".exe" if os.name == "nt" else ""
env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=os.path.join(target, "rustc"))
# The rustc baseline is "shipping rustc": the target's default CPU, not the workspace's host target-cpu=native
# (.cargo/config.toml). RUSTFLAGS replaces the config's flags; RW_RUSTFLAGS picks another baseline
# (e.g. "-C target-cpu=native" for the native-cpu comparison).
env["RUSTFLAGS"] = os.environ.get("RW_RUSTFLAGS", "")
native = os.environ.get("NATIVE")
if not native:
    subprocess.run(prefix + ["cargo", "build", "--release", "--offline", "-q", "-j", "8"], cwd=here, env=env, check=True)
    native = os.path.join(target, "rustc/release/relax-rwbench" + exe)
cnative = os.environ.get("NATIVE_COMMERCIAL")
if cnames and not cnative:
    subprocess.run(prefix + ["cargo", "build", "--release", "--offline", "-q", "-j", "8", "-p", CPKG, "--bin", CBIN], cwd=here, env=env, check=True)
    cnative = os.path.join(target, "rustc/release/" + CBIN + exe)
def native_cmd(w): return [cnative if w in cnames else native, w]
def relax_cmd(p, w): return [p, "run", "--quiet", "--verbose"] + ([CPKG, "--bin", CBIN] if w in cnames else ["relax-rwbench"]) + ["--", w]
std = os.environ.get("MAKEPAD_RUN_STD", os.path.join(here, "../std"))
base = dict(os.environ, RW_MS=os.environ.get("RW_MS", "8000"))
LINE = r"^\| (\w+) steady_ms=([\d.]+) tts_main_ms=([\d.]+) tts_proc_ms=([\d.]+) iters=(\d+) first_ms=([\d.]+) o3_ms=(-?\d+) (\d+)$"

def one(cmd, e):
    e2 = dict(e, RW_WALL0=str(time.time_ns()))
    try:
        out = subprocess.run(cmd, cwd=here, env=e2, capture_output=True, text=True, timeout=int(os.environ.get("RW_TIMEOUT", "900")))
    except subprocess.TimeoutExpired:
        return None, "TIMEOUT"
    m = re.search(LINE, out.stdout, re.M)
    if out.returncode != 0 or not m:
        return None, ("exit %d: " % out.returncode) + (out.stdout + out.stderr)[-300:].replace("\n", " ").replace("|", "/")
    t1 = re.search(r"makepad-run: O3: (\d+) hot, (\d+) installed(?:.*?(\d+) from the disk cache)?", out.stderr, re.M)
    replay = "-"
    profile = e.get("MAKEPAD_RUN_O3_PROFILE")
    if profile and os.path.isfile(profile + ".replay"):
        replay = Path(profile + ".replay").read_text().splitlines()[0]
    return dict(replay=replay, steady=float(m.group(2)), tts=float(m.group(4)), sum=m.group(8), o3=int(m.group(7)), tier1="%s hot / %s inst / %s cache" % (t1.group(1), t1.group(2), t1.group(3) or "0") if t1 else "-"), None

# runs[workload]["rustc" | name] = [result per round or None]
warm = os.environ.get("WARM") == "1"
filled = set()
recorded = set()
profiled = os.environ.get("O3PROF") == "1"
profile_dir = Path(target) / "o3-profiles"
if profiled:
    profile_dir.mkdir(parents=True, exist_ok=True)
assert len({n for n, _ in bins}) == len(bins), "Relax names must be unique"
if profiled:
    assert all(n and "/" not in n and "\\" not in n for n, _ in bins), "profile names must not contain path separators"

def relax_env(n, w=""):
    suffix = n.upper().replace("-", "_")
    nstd = os.environ.get("MAKEPAD_RUN_STD_" + suffix, std)
    # rwbench links the platform (the layout workload), so `makepad run` hosts it through the HAL: with no
    # display (a bench box, node 165) it needs the HAL's headless backend; no window opens on the mini either
    e = dict(base, MAKEPAD_RUN_CACHE="0", MAKEPAD_RUN_STD=nstd, MAKEPAD_RUN_CACHE_DIR=os.path.join(target, "relax-" + n))
    e.setdefault("MAKEPAD_HAL_BACKEND", "headless")
    # the timed window starts when the JIT reports its optimised code settled (main.rs RW_O3_PROGRESS)
    prog = os.path.join(target, "o3-progress-%s-%s" % (n, w))
    e["MAKEPAD_RUN_O3_PROGRESS"] = prog
    e["RW_O3_PROGRESS"] = prog
    try: os.unlink(prog)
    except OSError: pass
    if warm:
        e.update(MAKEPAD_RUN_CACHE="on")
    for item in shlex.split(os.environ.get("MAKEPAD_RUN_ENV_" + suffix, "")):
        key, sep, value = item.partition("=")
        if not sep or not key:
            raise ValueError("MAKEPAD_RUN_ENV_" + suffix + " requires KEY=value entries")
        e[key] = value
    return e
def prepare(n, p, w):
    renv = relax_env(n, w)
    cmd = prefix + relax_cmd(p, w)
    if profiled:
        profile = profile_dir / (n + "-" + w + ".t1")
        renv["MAKEPAD_RUN_O3_PROFILE"] = str(profile)
        if (w, n) not in recorded:
            # A stale profile (even under the same name) may belong to another binary.
            profile.unlink(missing_ok=True)
            Path(str(profile) + ".replay").unlink(missing_ok=True)
            rec, err = one(cmd, renv)
            if err or not profile.exists() or not profile.stat().st_size:
                raise RuntimeError("profile record %s/%s: %s" % (n, w, err or "empty profile"))
            recorded.add((w, n))
    if warm and (w, n) not in filled:
        filled.add((w, n))
        _, err = one(cmd, renv)
        if err:
            raise RuntimeError("cache fill %s/%s: %s" % (n, w, err))
    return renv

# Record every build before any measured round (including the native side).
if profiled:
    for w in names:
        for n, p in bins:
            prepare(n, p, w)

runs = {w: {k: [] for k in ["rustc"] + [n for n, _ in bins]} for w in names}
errs = {}
for r in range(rounds):
    order = list(reversed(bins)) if r % 2 == 1 else bins
    for w in names:
        a, ea = one(prefix + native_cmd(w), base)
        runs[w]["rustc"].append(a)
        if ea: errs[(w, "rustc")] = ea
        for n, p in order:
            renv = prepare(n, p, w)
            if profiled:
                Path(renv["MAKEPAD_RUN_O3_PROFILE"] + ".replay").unlink(missing_ok=True)
            b, eb = one(prefix + relax_cmd(p, w), renv)
            runs[w][n].append(b)
            if eb: errs[(w, n)] = eb
    print("# round %d/%d done (order %s)" % (r + 1, rounds, ",".join(n for n, _ in order)), file=sys.stderr, flush=True)

def med(v): return statistics.median(v)
def rng(v): return "%.3g..%.3g" % (min(v), max(v))
def geo(v):
    g = 1.0
    for x in v: g *= x
    return g ** (1.0 / len(v))

def ratios(w, num, den):
    return [x["steady"] / y["steady"] for x, y in zip(runs[w][num], runs[w][den]) if x and y]

for n, _ in bins:
    print("\n== %s  (%d rounds, medians; [min..max] over rounds)" % (n, rounds))
    print("| workload | rustc steady ms | Relax steady ms | O3 replay (installed / recorded / unrecorded-live / divergences) | Relax/rustc (median of per-round ratios) | spread | Relax time-to-steady ms | O3 settled ms (since main; 0 = never within RW_MAX_MS) | Relax O3 coverage | checksums equal |")
    print("|---|---:|---:|---|---:|---|---:|---:|---|---|")
    gm = []
    for w in names:
        a = [x for x in runs[w]["rustc"] if x]; b = [x for x in runs[w][n] if x]
        if not a or not b:
            print("| %s | | | | ERROR | | | | %s |" % (w, errs.get((w, n)) or errs.get((w, "rustc"), ""))); continue
        rt = ratios(w, n, "rustc")
        gm.append(med(rt))
        sums = {x["sum"] for x in a + b}
        print("| %s | %.3f [%s] | %.3f [%s] | %s | %.2f | %s | %.0f | %s | %s | %s |" % (w, med([x["steady"] for x in a]), rng([x["steady"] for x in a]), med([x["steady"] for x in b]), rng([x["steady"] for x in b]), "; ".join(dict.fromkeys(x["replay"] for x in b)), med(rt), rng(rt), med([x["tts"] for x in b]), "/".join(str(x["o3"]) for x in b), b[-1]["tier1"], "yes" if len(sums) == 1 else "**NO** %s" % sorted(sums)))
    if gm:
        print("\ngeomean Relax/rustc: %.3f (%d workloads)" % (geo(gm), len(gm)))

if len(bins) > 1:
    b0 = bins[0][0]
    for n, _ in bins[1:]:
        print("\n== A/B  %s / %s  (per workload: median of per-round ratios, [min..max])" % (n, b0))
        print("| workload | %s/%s | spread | rounds |" % (n, b0))
        print("|---|---:|---|---:|")
        gm = []
        for w in names:
            rt = ratios(w, n, b0)
            if not rt:
                print("| %s | ERROR | | |" % w); continue
            gm.append(med(rt))
            print("| %s | %.3f | %s | %d |" % (w, med(rt), rng(rt), len(rt)))
        if gm:
            print("\ngeomean %s/%s: %.3f (%d workloads; spread of the per-workload ratios %s)" % (n, b0, geo(gm), len(gm), rng(gm)))

# A completed table is not a passing comparison if any process failed or any
# checksum differs, even when other rounds for the same workload succeeded.
failed = any(
    any(x is None for results in sides.values() for x in results)
    or len({x["sum"] for results in sides.values() for x in results if x}) != 1
    for sides in runs.values()
)
sys.exit(1 if failed else 0)
