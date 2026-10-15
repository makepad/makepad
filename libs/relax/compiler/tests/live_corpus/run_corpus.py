#!/usr/bin/env python3
"""The live-coding scoreboard: replays an AI-edit corpus (corpus/<app>.py) against the running app.

usage: run_corpus.py --app <counter|todo|...> --tree <makepad tree> --makepad <loader> [--only A..B] [--out JSON]

The app runs under the loader (a working tree: live) with a fresh session log (MAKEPAD_RUN_LIVE_LOG); each edit
is one full-file write, then the driver waits for its log record (an edit; after a restart also
the restarted run's first present). At the end: % applied in place (patch + unchanged) per category
and overall, p50/p95 save->applied of in-place edits, p50/p95 save->present of restarts, the
restart/refuse reasons; the source file is restored and the app stopped (by its PID).
Linux: needs WAYLAND_DISPLAY (node 165: wayland-hrl from ~/hrl/weston_up.sh, outside x64run).
"""
import argparse, importlib.util, json, os, signal, subprocess, sys, time

def load_corpus(app):
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)), "corpus", app + ".py")
    spec = importlib.util.spec_from_file_location("corpus_" + app, p)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m

def records(log, start):
    out = []
    try:
        with open(log) as f:
            for l in f.readlines()[start:]:
                try:
                    out.append(json.loads(l))
                except ValueError:
                    pass
    except FileNotFoundError:
        pass
    return out

def nlines(log):
    try:
        with open(log) as f:
            return sum(1 for _ in f)
    except FileNotFoundError:
        return 0

def pct(v, p):
    if not v:
        return None
    v = sorted(v)
    return v[min(len(v) - 1, int(round((len(v) - 1) * p)))]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--app", required=True)
    ap.add_argument("--tree", required=True)
    ap.add_argument("--makepad", required=True)
    ap.add_argument("--only", default="")
    ap.add_argument("--out", default="")
    ap.add_argument("--wait", type=float, default=30.0, help="seconds to wait for one edit's record")
    a = ap.parse_args()
    c = load_corpus(a.app)
    edits = c.EDITS
    if a.only:
        lo, hi = a.only.split("..")
        edits = edits[int(lo):int(hi)]
    path = os.path.join(a.tree, c.FILE)
    orig = open(path).read()
    log = "/tmp/live-corpus-%s-%d.jsonl" % (a.app, os.getpid())
    out_txt = "/tmp/live-corpus-%s-%d.out" % (a.app, os.getpid())
    env = dict(os.environ, MAKEPAD_RUN_LIVE_LOG=log, MAKEPAD_TRACE="startup", MAKEPAD_RUN_TRACE="startup", SANDBOX_MUTE="1")
    env.setdefault("MAKEPAD_RUN_STD", os.path.join(a.tree, "libs/relax/compiler/std"))
    cmd = [a.makepad, "run", c.PKG] + list(getattr(c, "ARGS", []))
    outf = open(out_txt, "w")
    proc = subprocess.Popen(cmd, cwd=a.tree, env=env, stdout=outf, stderr=subprocess.STDOUT, start_new_session=True)
    results = []
    try:
        t0 = time.time()
        while "FIRST PRESENT" not in open(out_txt).read():
            if proc.poll() is not None or time.time() - t0 > 600:
                print("app did not present (exit %s); tail:\n%s" % (proc.poll(), open(out_txt).read()[-2000:]))
                return 2
            time.sleep(0.25)
        time.sleep(2)
        text = orig
        for i, e in enumerate(edits):
            name, cat = e[0], e[1]
            # (name, cat, old, new), or (name, cat, [(old, new), ...]): one save of several replaces
            pairs = e[2] if len(e) == 3 else [(e[2], e[3])]
            bad = [o for o, _ in pairs if text.count(o) != 1]
            if bad:
                results.append({"edit": name, "cat": cat, "rung": "corpus-mismatch"})
                print("%2d %-24s %-14s CORPUS MISMATCH (%r found %d times)" % (i, name, cat, bad[0][:40], text.count(bad[0])))
                continue
            for o, n in pairs:
                text = text.replace(o, n, 1)
            n0 = nlines(log)
            with open(path, "w") as f:
                f.write(text)
            rec, present = None, None
            t1 = time.time()
            while time.time() - t1 < a.wait:
                rs = records(log, n0)
                rec = next((r for r in rs if r.get("event") == "edit"), rec)
                if rec and rec["rung"] != "restart":
                    break
                if rec and rec["rung"] == "restart":
                    present = next((r for r in rs if r.get("event") == "restart_present"), None)
                    if present:
                        break
                if proc.poll() is not None:
                    break
                time.sleep(0.02)
            r = {"edit": name, "cat": cat, "rung": rec["rung"] if rec else "no-record", "ms": rec["ms"]["total"] if rec else None,
                 "work_ms": rec["ms"]["work"] if rec else None, "reasons": rec.get("reasons", [])[:3] if rec else [],
                 "present_ms": present["ms"] if present else None,
                 "phases": rec.get("ms") if rec else None,
                 "rebuilt": rec.get("rebuilt") if rec else None, "ahead": rec.get("ahead") if rec else None, "ahead_cached": rec.get("ahead_cached") if rec else None,
                 "field_named": rec.get("field_named") if rec else None, "field_deps": rec.get("field_deps") if rec else None}
            results.append(r)
            work = ""
            if r.get("rebuilt") is not None:
                work = "rebuilt %d ahead %d (%d cached)" % (r["rebuilt"], r["ahead"] or 0, r.get("ahead_cached") or 0)
                if r.get("field_named"):
                    work += " (field: %d name it, %d use it)" % (r["field_named"], r["field_deps"])
            print("%2d %-24s %-14s %-10s %7s ms %-34s %s" % (i, name, cat, r["rung"], "%.0f" % r["ms"] if r["ms"] is not None else "-", work,
                  ("present %d ms" % r["present_ms"]) if r["present_ms"] else (r["reasons"][0][:100] if r["reasons"] else "")))
            if proc.poll() is not None:
                print("app exited (%s); stopping" % proc.poll())
                break
            time.sleep(0.6)
    finally:
        with open(path, "w") as f:
            f.write(orig)
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            time.sleep(2)
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
            time.sleep(1)
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
    # the scoreboard
    scored = [r for r in results if r["rung"] not in ("corpus-mismatch",)]
    inplace = lambda r: r["rung"] in ("patch", "unchanged")
    cats = {}
    for r in scored:
        cats.setdefault(r["cat"], []).append(r)
    print("\nscoreboard %s: %d edits" % (a.app, len(scored)))
    print("%-14s %5s %8s %8s %7s" % ("category", "edits", "in place", "restart", "refuse"))
    for k in sorted(cats):
        v = cats[k]
        print("%-14s %5d %7.0f%% %8d %7d" % (k, len(v), 100.0 * sum(map(inplace, v)) / len(v), sum(r["rung"].startswith("restart") for r in v), sum(r["rung"] == "refuse" for r in v)))
    ip = [r["ms"] for r in scored if inplace(r) and r["ms"] is not None]
    rs = [r["present_ms"] for r in scored if r["present_ms"]]
    summary = {
        "app": a.app, "edits": len(scored),
        "in_place_pct": 100.0 * sum(map(inplace, scored)) / max(1, len(scored)),
        "restart": sum(r["rung"].startswith("restart") for r in scored), "refuse": sum(r["rung"] == "refuse" for r in scored),
        "no_record": sum(r["rung"] == "no-record" for r in scored),
        "in_place_p50_ms": pct(ip, 0.5), "in_place_p95_ms": pct(ip, 0.95),
        "restart_present_p50_ms": pct(rs, 0.5), "restart_present_p95_ms": pct(rs, 0.95),
        "results": results,
    }
    print("overall: %.0f%% in place, %d restart, %d refuse, %d without a record; in place p50 %s / p95 %s ms; restart->present p50 %s / p95 %s ms" % (
        summary["in_place_pct"], summary["restart"], summary["refuse"], summary["no_record"],
        summary["in_place_p50_ms"], summary["in_place_p95_ms"], summary["restart_present_p50_ms"], summary["restart_present_p95_ms"]))
    # where an in-place edit's time goes (p50 of each phase)
    ph = [r["phases"] for r in scored if inplace(r) and r.get("phases")]
    if ph:
        keys = ["detect", "parse", "check", "compile", "install", "work", "total"]
        print("in-place phases p50: " + ", ".join("%s %.1f" % (k, pct([p.get(k, 0) for p in ph], 0.5)) for k in keys) + " ms (detect = save -> seen; total - detect - work = settle + queue)")
    reasons = {}
    for r in scored:
        if r["rung"] != "patch" and r["rung"] != "unchanged":
            for x in r["reasons"][:1]:
                key = x.split(": ", 1)[-1][:90]
                reasons.setdefault(r["rung"] + ": " + key, []).append(r["edit"])
    for k, v in sorted(reasons.items(), key=lambda kv: -len(kv[1])):
        print("  %s  <- %s" % (k, ", ".join(v)))
    if a.out:
        json.dump(summary, open(a.out, "w"), indent=1)
    return 0

if __name__ == "__main__":
    sys.exit(main())
