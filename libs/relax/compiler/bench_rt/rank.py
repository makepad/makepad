#!/usr/bin/env python3
"""rank: the hottest Relax functions ranked by  samples x (Relax instructions / rustc instructions),
the work list for asm comparison (asmdiff.py). Profiles come from Relax's own sampler (MAKEPAD_RUN_PROFILE,
steady state), so this works on macOS too; rustc sizes from the symbols of a native release build.

  rank.py --out DIR bench [--makepad R] [--native BIN] [--warm MS] [kernel ...]
        run bench_rt kernels under Relax at steady state, one profile per kernel (DIR/bench-<k>.tsv)
  rank.py --out DIR add <label> <profile.tsv> [<native binary>]
        add another profile (an Amp / Stage steady-state run made with MAKEPAD_RUN_PROFILE=<file>)
  rank.py --out DIR show [--min PCT] [--top N]
        the ranking over every source in DIR (each source weighs 100; a fn's share is of its source)

Instruction counts: arm64 = bytes / 4 (both sides); x64 = bytes / 4 too (an estimate: the ratio of code
bytes; asmdiff shows exact counts). A fn without a rustc symbol of the same path (inlined by rustc, or a
Relax-only name) gets ratio "?" and is ranked by its samples alone (x 1).
"""
import os, sys, json, subprocess, re

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import asmdiff  # noqa: E402


def load_sources(d):
    p = os.path.join(d, "sources.json")
    return json.load(open(p)) if os.path.exists(p) else []


def save_sources(d, s):
    json.dump(s, open(os.path.join(d, "sources.json"), "w"), indent=1)


def read_profile(p):
    rows = []
    for l in open(p):
        x = l.rstrip("\n").split("\t")
        if len(x) == 4:
            rows.append((int(x[0]), int(x[1]), int(x[2]), x[3]))
    return rows


def norm_name(n):
    """Relax names carry `@<line>` and generic args in their own spelling: compare paths without them"""
    n = re.sub(r"@\d+", "", n)
    return n


def strip_generics(n):
    out, depth = "", 0
    for c in n:
        if c == "<":
            depth += 1
        elif c == ">":
            depth -= 1
        elif depth == 0:
            out += c
    return out


def main():
    a = sys.argv[1:]
    if len(a) < 3 or a[0] != "--out":
        sys.exit(__doc__)
    out = a[1]
    os.makedirs(out, exist_ok=True)
    cmd = a[2]
    rest = a[3:]
    srcs = load_sources(out)
    if cmd == "bench":
        relax = os.environ.get("MAKEPAD_RUN_BIN", "makepad")
        native = None
        warm = "3000"
        ks = []
        i = 0
        while i < len(rest):
            if rest[i] == "--makepad":
                relax = rest[i + 1]; i += 2
            elif rest[i] == "--native":
                native = rest[i + 1]; i += 2
            elif rest[i] == "--warm":
                warm = rest[i + 1]; i += 2
            else:
                ks.append(rest[i]); i += 1
        if not native:
            native = asmdiff.native_bin({"native": None})
        if not ks:
            src = open(os.path.join(HERE, "src/main.rs")).read()
            ks = re.findall(r'\("(\w+)", \w+\),', src)
        for k in ks:
            prof = os.path.abspath(os.path.join(out, "bench-%s.tsv" % k))
            env = dict(os.environ, MAKEPAD_RUN_PROFILE=prof, BENCH_WARM_MS=warm, BENCH_REPS=os.environ.get("BENCH_REPS", "20"), SANDBOX_MUTE="1")
            env.setdefault("MAKEPAD_RUN_STD", os.path.join(HERE, "../std"))
            env.setdefault("BENCH_FONT", os.path.normpath(os.path.join(HERE, "../../../../widgets/resources/IBMPlexSans-Text.ttf")))
            r = subprocess.run(["nice", "-n", "10", relax, "run", "--quiet", "relax-bench-rt", "--", k], cwd=HERE, env=env, capture_output=True, text=True)
            ok = os.path.exists(prof)
            print("%-12s %s" % (k, "profile ok" if ok else "NO PROFILE " + r.stderr[-300:]), flush=True)
            if ok:
                srcs = [s for s in srcs if s["label"] != "bench:" + k]
                srcs.append({"label": "bench:" + k, "profile": prof, "native": os.path.abspath(native), "kernel": k})
        save_sources(out, srcs)
        return
    if cmd == "add":
        label, prof = rest[0], os.path.abspath(rest[1])
        native = os.path.abspath(rest[2]) if len(rest) > 2 else None
        srcs = [s for s in srcs if s["label"] != label]
        srcs.append({"label": label, "profile": prof, "native": native})
        save_sources(out, srcs)
        return
    if cmd != "show":
        sys.exit(__doc__)
    mn, top = 1.0, 40
    i = 0
    while i < len(rest):
        if rest[i] == "--min":
            mn = float(rest[i + 1]); i += 2
        elif rest[i] == "--top":
            top = int(rest[i + 1]); i += 2
        else:
            i += 1
    symcache = {}
    rows = []
    for s in srcs:
        prof = read_profile(s["profile"])
        tot = sum(r[0] for r in prof) or 1
        syms = {}
        if s.get("native"):
            if s["native"] not in symcache:
                symcache[s["native"]] = {k: v[1] for k, v in asmdiff.rust_symbols(s["native"]).items()}
            syms = symcache[s["native"]]
        bare = {}
        for k, v in syms.items():
            bare.setdefault(strip_generics(k), []).append((k, v))
        for n, tier, size, name in prof:
            share = 100.0 * n / tot
            if share < mn:
                continue
            nn = norm_name(name)
            rs = syms.get(nn)
            how = "="
            if rs is None:
                c = bare.get(strip_generics(nn), [])
                if len(c) == 1:
                    rs = c[0][1]
                    how = "~"
                elif len(c) > 1:
                    rs = max(x[1] for x in c)
                    how = "~%d" % len(c)
            ratio = (size / rs) if rs else None
            rows.append((share * (ratio if ratio else 1.0), share, ratio, how, tier, size, rs, s["label"], name, s.get("kernel")))
    rows.sort(key=lambda r: -r[0])
    print("%7s %6s %6s %3s %5s %7s %7s  %-18s %s" % ("score", "share", "ratio", "", "tier", "relaxB", "rustcB", "source", "fn"))
    for r in rows[:top]:
        print("%7.1f %5.1f%% %6s %3s %5s %7d %7s  %-18s %s" % (r[0], r[1], ("%.2f" % r[2]) if r[2] else "?", r[3] if r[2] else "", "t%d" % r[4], r[5], r[6] or "-", r[7][:18], r[8][:110]))
    print("\nnext: asmdiff.py [--kernel K | --run ...] <fn>   (ratio '~' = matched without generic args, '?' = no rustc symbol)")


if __name__ == "__main__":
    main()
