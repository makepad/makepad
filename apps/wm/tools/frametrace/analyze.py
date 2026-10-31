#!/usr/bin/env python3
"""Frame-exact motion checker for MAKEPAD_WM_FRAMETRACE traces.

    analyze.py frames.jsonl [--marks marks.json] [--from N] [--to N] [--json]

For every window it follows what the person SEES each frame: the live
record, or during a slide its record inside the space capture, moved by the
slide offset. Then it flags:

  SIZE-NO-TWEEN   the seen size changed and nothing governs it (no spring,
                  popin, pull-out/edge/unfold ease, compact fade)
  SIZE-JUMP       a governed size change whose step is >2.5x its neighbours'
                  (a snap, a pose switch mid-tween)
  POS-JUMP        the seen position moved >40 pt in one frame with nothing
                  governing it (a drag or a slide governs position)
  HANDOFF         the frame a slide starts or ends: the capture and the live
                  window disagree (seen rect differs by >1 pt)
  ORIGIN          a window seen at the desk origin (x<=1, y<=desk.y+1) that
                  was elsewhere the frame before
  DEGENERATE      a drawn rect of w or h <= 1
  CFG-HIDDEN      a client reconfigured while it is not on a shown space
                  (drawn into a capture) or during a slide
  CFG-CHURN       a client's configured size changed while its target did not
  CFG-TWICE       a client reconfigured twice within 3 s (one action, two resizes)
  STRETCH         a live window drawn from a frame of another aspect
                  (presented px vs drawn rect, >3%) -- runs of frames
  SHELF-SHOWN     a parked window seen on screen while its shelf is hidden (hide>=0.99)
  BLANK-SHOT      a slide frame drew no capture for a space it shows
  NO-FRAME        a live window drawn with no client frame (startup excluded)
  LONG-FRAME      (pacing, real dumps) the gap to the previous desk frame while
                  something animates is >1.5x the display interval (--hz)
  SLOW-DRAW       (pacing) the desk draw itself took >50% of the interval
  RESIZE-STALL    (pacing) a client's new configured size took >100 ms to come
                  back as a presented frame of that size (stretched meanwhile)

`--hz 240` sets the display rate for the pacing checks; `--no-pacing` skips
them (virtual-clock rig runs). The end prints the worst frames with their
cause tags.

Exit status 1 when anything is flagged (a regression gate).
"""
import argparse, json, sys

SIZE_TOL = 1.0
JUMP_FRAC, JUMP_PT = 0.06, 16.0
POS_TOL = 40.0


def load(path):
    out = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass
    return out


def seen_rects(frames):
    """Per frame: client -> (rect, kind, record). kind 'live' or 'shot'."""
    last_cap = {}  # (disp,slot) -> {client: record}
    out = []
    for fr in frames:
        for w in fr["w"]:
            if w["cap"] is not None:
                last_cap.setdefault(tuple(w["cap"]), {})
        # captures made this frame replace the old records of that key
        made = {tuple(m["key"]) for m in fr["made"]}
        for k in made:
            last_cap[k] = {}
        for w in fr["w"]:
            if w["cap"] is not None and tuple(w["cap"]) in made:
                last_cap[tuple(w["cap"])][w["c"]] = w
        seen = {}
        for w in fr["w"]:
            if w["cap"] is None:
                seen[w["c"]] = (w["s"], "live", w)
        disp = {d["i"]: d for d in fr["disp"]}
        for sh in fr["shots"]:
            if not sh["drawn"]:
                continue
            key = tuple(sh["key"])
            d = disp.get(key[0])
            if not d:
                continue
            sw = d["r"][2]
            dx = sh["off"] * sw
            for c, w in last_cap.get(key, {}).items():
                s = w["s"]
                r = [s[0] + dx, s[1], s[2], s[3]]
                # cropped away entirely: not seen
                if r[0] + r[2] <= d["r"][0] or r[0] >= d["r"][0] + sw:
                    continue
                if c not in seen:
                    seen[c] = (r, "shot", w)
        out.append(seen)
    return out


def governed(fr, w, c):
    a = w.get("anim") or {}
    g = []
    if a.get("mv"):
        g.append("spring")
    if a.get("open", 1.0) < 1.0:
        g.append("open")
    if a.get("close") is not None:
        g.append("close")
    if fr["leave"] and fr["leave"]["c"] == c:
        g.append("leave")
    if fr["enter"] and fr["enter"]["c"] == c:
        g.append("enter")
    if fr["unfold"] and fr["unfold"]["c"] == c:
        g.append("unfold")
    if w.get("frz"):
        g.append("compact")
    if w.get("swl"):
        g.append("swell")
    if w.get("src") == "dockwarp" or w.get("min"):
        g.append("minimise")
    if c in fr.get("dragging", []):
        g.append("pointer")  # a move or resize drag: the size follows the hand
    return g


def dragging(fr, c):
    return c in fr.get("dragging", [])


def sliding(fr):
    return any(d["slide"] for d in fr["disp"])


def pacing(frames, lo, hi, hz, flag):
    iv = 1.0 / hz
    for i in range(max(lo, 1), hi):
        fr, pv = frames[i], frames[i - 1]
        dt = fr["t"] - pv["t"]
        moving = pv.get("anim") or any(d["slide"] for d in pv["disp"]) or pv.get("dragging")
        if moving and dt > 1.5 * iv:
            flag(i, "LONG-FRAME", "desk", f"{dt * 1000:.1f} ms since #{pv['n']} ({dt / iv:.1f} intervals at {hz:.0f} Hz)")
        if fr.get("draw_ms", 0.0) > 500.0 * iv:
            flag(i, "SLOW-DRAW", "desk", f"desk draw {fr['draw_ms']:.2f} ms")
    # Resize round trips: cfg change -> first presented frame at the new aspect.
    pending = {}
    for i in range(lo, hi):
        fr = frames[i]
        for w in fr["w"]:
            h = w.get("host")
            if not h or w["cap"] is not None:
                continue
            c = w["c"]
            cfg = tuple(h["cfg"])
            if c in pending and pending[c][1] != cfg:
                pending.pop(c)
            if c not in pending:
                prev = pending.get(("last", c))
                pending[("last", c)] = cfg
                if prev is not None and prev != cfg:
                    pending[c] = (i, cfg, h["ser"])
                continue
            i0, want, ser0 = pending[c]
            px = h["px"]
            if px and h["ser"] != ser0 and abs(px[0] / max(px[1], 1) - want[0] / want[1]) < 0.02 * (want[0] / want[1]):
                dt = fr["t"] - frames[i0]["t"]
                if dt > 0.1:
                    flag(i, "RESIZE-STALL", f'{c}:{w.get("app", "")}', f"configured {want} at #{frames[i0]['n']}, first frame of that size {dt * 1000:.0f} ms later")
                pending.pop(c)


def analyze(frames, lo=0, hi=None, marks=None, hz=None):
    hi = len(frames) if hi is None else hi
    seen = seen_rects(frames)
    issues = []

    def flag(i, kind, c, msg):
        issues.append({"n": frames[i]["n"], "t": frames[i]["t"], "kind": kind, "c": c, "msg": msg})

    first_frame_seen = {}
    cfgs = {}  # client -> configure changes
    steps = {}  # client -> governed size steps (frame index, dw, dh, ...)
    for i in range(max(lo, 1), hi):
        fr, pv = frames[i], frames[i - 1]
        S, P = seen[i], seen[i - 1]
        desk = fr["desk"]
        apps = {w["c"]: w.get("app", "") for w in fr["w"]}
        for c, (r, kind, w) in S.items():
            name = f'{c}:{apps.get(c, "")}'
            if r[2] <= 1 or r[3] <= 1:
                flag(i, "DEGENERATE", name, f"seen {r} src={w['src']}")
            if c not in P:
                continue
            q, pkind, pw = P[c]
            dw, dh = r[2] - q[2], r[3] - q[3]
            dx, dy = r[0] - q[0], r[1] - q[1]
            g = governed(fr, w, c) + governed(pv, pw, c)
            if kind != pkind:
                if max(abs(dw), abs(dh), abs(dx), abs(dy)) > 1.0:
                    flag(i, "HANDOFF", name, f"{pkind}->{kind}: {fmt(q)} -> {fmt(r)} src={w['src']}")
                continue
            if abs(dw) > SIZE_TOL or abs(dh) > SIZE_TOL:
                if not g:
                    flag(i, "SIZE-NO-TWEEN", name, f"{fmt(q)} -> {fmt(r)} d=({dw:+.1f},{dh:+.1f}) src={pw['src']}->{w['src']}")
                else:
                    steps.setdefault(c, []).append((i, dw, dh, q, r, "+".join(g), pw["src"], w["src"], name, min(max(fr["t"] - pv["t"], 1 / 120), 0.025) if "pointer" not in g else 1 / 60))
            if (abs(dx) > POS_TOL or abs(dy) > POS_TOL) and not g and not dragging(fr, c) and not dragging(pv, c) and kind == "live":
                flag(i, "POS-JUMP", name, f"{fmt(q)} -> {fmt(r)} src={pw['src']}->{w['src']}")
            tgt = (w.get("anim") or {}).get("tgt") or [9e9, 9e9]
            at_origin_by_layout = tgt[0] <= desk[0] + 1 and tgt[1] <= desk[1] + 1
            if r[0] <= desk[0] + 1 and r[1] <= desk[1] + 1 and not (q[0] <= desk[0] + 1 and q[1] <= desk[1] + 1) and not at_origin_by_layout:
                flag(i, "ORIGIN", name, f"{fmt(q)} -> {fmt(r)} src={w['src']}")
        # client configuration
        prev_cfg = {}
        for w in pv["w"]:
            if w.get("host"):
                prev_cfg[w["c"]] = (w["host"]["cfg"], (w.get("anim") or {}).get("tgt"))
        for w in fr["w"]:
            h = w.get("host")
            if not h or w["c"] not in prev_cfg:
                continue
            pc, ptgt = prev_cfg[w["c"]]
            if any(abs(a - b) > 0.5 for a, b in zip(h["cfg"], pc)):
                name = f'{w["c"]}:{w.get("app", "")}'
                why = []
                if w["cap"] is not None:
                    why.append("in a space capture")
                if sliding(fr):
                    why.append("during a slide")
                if why:
                    flag(i, "CFG-HIDDEN", name, f"configured {pc} -> {h['cfg']} ({', '.join(why)})")
                if w["c"] not in fr.get("dragging", []):  # a resize drag reconfigures as the hand moves
                    cfgs.setdefault(w["c"], []).append((i, pc, h["cfg"], name))
                tgt = (w.get("anim") or {}).get("tgt")
                if ptgt and tgt and ptgt == tgt and not w.get("half") and not w.get("frz") and not (fr["leave"] and fr["leave"]["c"] == w["c"]):
                    flag(i, "CFG-CHURN", name, f"configured {pc} -> {h['cfg']} with target unchanged {fmt(tgt)}")
        # slide frames: every space in view must have its capture drawn
        for d in fr["disp"]:
            if d["slide"]:
                drawn = [s for s in fr["shots"] if s["key"][0] == d["i"] and s["drawn"]]
                if not drawn:
                    flag(i, "BLANK-SHOT", f'disp{d["i"]}', f"slide pos {d['slide']['pos']:.3f} drew no capture")
                for s in fr["shots"]:
                    if s["key"][0] == d["i"] and not s["drawn"]:
                        flag(i, "BLANK-SHOT", f'disp{d["i"]}', f"capture {s['key']} missing at off {s['off']:.3f}")
        # shelf items while the shelf is hidden
        for w in fr["w"]:
            if w["cap"] is not None:
                continue
            p = w.get("pose") or {}
            side = p.get("side", 0)
            if side == 0 or dragging(fr, w["c"]) or (fr["leave"] and fr["leave"]["c"] == w["c"]) or w.get("half"):
                continue
            d0 = fr["disp"][0]
            hide = d0["hide"][0 if side < 0 else 1]
            s = w["s"]
            scr = d0["r"]
            on = s[0] < scr[0] + scr[2] - 0.5 and s[0] + s[2] > scr[0] + 0.5
            if hide >= 0.99 and on:
                flag(i, "SHELF-SHOWN", f'{w["c"]}:{w.get("app", "")}', f"side {side} hide {hide:.2f} seen {fmt(s)} swell={w.get('swl')}")
        # stretched live windows and windows without frames
        for w in fr["w"]:
            h = w.get("host")
            if w["cap"] is not None or not h or w["src"] in ("reuse", "frozen", "dockwarp", "min-gone"):
                continue
            if not h["has"]:
                c = w["c"]
                if c in first_frame_seen:
                    flag(i, "NO-FRAME", f'{c}:{w.get("app", "")}', "live window has no client frame")
                continue
            first_frame_seen.setdefault(w["c"], i)
            px = h["px"]
            req = w.get("req")
            if px and req and req[0] > 1 and req[1] > 1:
                a_px = px[0] / max(px[1], 1)
                a_req = req[0] / req[1]
                if abs(a_px / a_req - 1.0) > 0.03:
                    flag(i, "STRETCH", f'{w["c"]}:{w.get("app", "")}', f"frame {px} px for {req} pt (aspect {a_px:.3f} vs {a_req:.3f}) src={w['src']}")
    if hz:
        pacing(frames, max(lo, 0), hi, hz, flag)
    # A governed size change is a tween: its steps change smoothly. A step far
    # bigger than its neighbours (a snap to the end, a pose switch) is a jump.
    for c, st in steps.items():
        # A tween advances by real time: compare speeds (per 1/60 s), so a frame
        # that came late (load) and the short one after it are not a jump; the
        # animation clock itself is capped at 25 ms a frame, so is dt here;
        # a frame under half a frame long (a tween's first, nominal step) is judged as half. A step
        # that follows the pointer moves with the hand, not the clock: dt is 1.
        for k, (i, dw, dh, q, r, g, ps, cs, name, dt) in enumerate(st):
            m = max(abs(dw), abs(dh))
            if m < JUMP_PT * 0.5:
                continue
            nb = []
            if k > 0 and st[k - 1][0] >= i - 12:
                nb.append(max(abs(st[k - 1][1]), abs(st[k - 1][2])) * (dt / st[k - 1][9]))
            if k + 1 < len(st) and st[k + 1][0] <= i + 12:
                nb.append(max(abs(st[k + 1][1]), abs(st[k + 1][2])) * (dt / st[k + 1][9]))
            ref = max(nb) if nb else 0.0
            if (nb and m > 2.5 * max(ref, 2.0)) or (not nb and m > 3 * JUMP_PT):
                flag(i, "SIZE-JUMP", name, f"{fmt(q)} -> {fmt(r)} d=({dw:+.1f},{dh:+.1f}) neighbour steps {ref:.1f} by={g} src={ps}->{cs}")
    # One action, one configure: two size changes of a client within 1.5 s.
    for c, ch in cfgs.items():
        for (i0, a0, b0, name), (i1, a1, b1, _) in zip(ch, ch[1:]):
            if frames[i1]["t"] - frames[i0]["t"] < 3.0:
                flag(i1, "CFG-TWICE", name, f"{fmt(a0)} -> {fmt(b0)} at #{frames[i0]['n']}, then -> {fmt(b1)} {frames[i1]['t'] - frames[i0]['t']:.2f} s later")
    issues.sort(key=lambda it: it["n"])
    return issues


def fmt(r):
    return "[" + ",".join(f"{v:.1f}" for v in r) + "]"


def collapse(issues):
    """Runs of the same (kind, client) on consecutive frames become one line."""
    out = []
    for it in issues:
        if out and out[-1]["kind"] == it["kind"] and out[-1]["c"] == it["c"] and it["n"] - out[-1]["n_end"] <= 1 and it["kind"] in ("STRETCH", "SHELF-SHOWN", "NO-FRAME", "CFG-HIDDEN"):
            out[-1]["n_end"] = it["n"]
            out[-1]["count"] += 1
            continue
        out.append(dict(it, n_end=it["n"], count=1))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("--marks")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--from", dest="lo", type=int, default=0)
    ap.add_argument("--to", dest="hi", type=int, default=None)
    ap.add_argument("--ignore", default="", help="comma list of kinds not to fail on")
    ap.add_argument("--hz", type=float, default=240.0, help="display rate for the pacing checks")
    ap.add_argument("--no-pacing", action="store_true", help="skip pacing checks (virtual-clock runs)")
    a = ap.parse_args()
    frames = load(a.trace)
    marks = []
    if a.marks:
        try:
            marks = json.load(open(a.marks))
        except (OSError, ValueError):
            marks = []
    lo = next((i for i, f in enumerate(frames) if f["n"] >= a.lo), 0)
    hi = None if a.hi is None else next((i for i, f in enumerate(frames) if f["n"] > a.hi), len(frames))
    issues = collapse(analyze(frames, lo, hi, hz=None if a.no_pacing else a.hz))
    ignore = set(x for x in a.ignore.split(",") if x)
    if a.json:
        print(json.dumps(issues))
    else:
        mi = 0
        for it in issues:
            while mi < len(marks) and marks[mi][0] <= it["n"]:
                print(f"--- frame {marks[mi][0]}: {marks[mi][1]}")
                mi += 1
            rng = f'{it["n"]}' if it["count"] == 1 else f'{it["n"]}-{it["n_end"]} ({it["count"]}x)'
            print(f'{rng:>16} {it["kind"]:<14} {it["c"]:<14} {it["msg"]}')
        counts = {}
        for it in issues:
            counts[it["kind"]] = counts.get(it["kind"], 0) + 1
        # The worst frames: every tag a frame collected, the most first.
        tags = {}
        for it in issues:
            tags.setdefault(it["n"], []).append(it["kind"])
        worst = sorted(tags.items(), key=lambda kv: -len(kv[1]))[:10]
        if worst:
            print("WORST", "; ".join(f"#{n}: {'+'.join(sorted(set(k)))}" for n, k in worst))
        if frames and not a.no_pacing:
            dts = sorted(frames[i]["t"] - frames[i - 1]["t"] for i in range(1, len(frames)) if frames[i - 1].get("anim"))
            if dts:
                p = lambda q: dts[min(len(dts) - 1, int(q * len(dts)))] * 1000
                print(f"PACING animating frames {len(dts)}: median {p(0.5):.1f} ms, p95 {p(0.95):.1f} ms, max {dts[-1] * 1000:.1f} ms (display {1000 / a.hz:.2f} ms)")
        print("SUMMARY", len(frames), "frames;", ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "clean")
    sys.exit(1 if any(it["kind"] not in ignore for it in issues) else 0)


if __name__ == "__main__":
    main()
