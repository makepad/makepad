#!/usr/bin/env python3
"""The user's scenarios, frame by frame (node 165, hidden).

    scenarios.py <root> <scenario> [--autohide left,right] [--tag T]

scenarios: swipe, swipe-fast, shelf, unfold, modes, all
Writes ~/lanes/wm-review-runs/<tag>/frames.jsonl + marks.json and runs
analyze.py over it (exit 1 when anything is flagged).
"""
import argparse, json, os, re, subprocess as sp, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ftrig import Rig

SUPER = dict(logo=1)


def first_frames(rig, n, max_frames=900):
    """Step until n clients reported a first frame."""
    for _ in range(max_frames // 10):
        txt = open(rig.log, errors="replace").read()
        if len(re.findall(r"client \d+ first frame", txt)) >= n:
            return True
        rig.frames(10, real=0.1)
    return False


def live(rig):
    fr = rig.last()
    return {w["c"]: w for w in fr["w"] if w["cap"] is None}, fr


def by_app(rig, app):
    ws, _ = live(rig)
    for c, w in ws.items():
        if w.get("app") == app:
            return w
    return None


def launch(rig, app, count):
    rig.launch(app)
    ok = first_frames(rig, count)
    rig.frames(40, real=0.03)
    return ok


def caption_point(w):
    s = w["s"]
    return (s[0] + s[2] * 0.5, s[1] + 12)


def build_desks(rig, autohide):
    """Desk 1: notes + clock, notes on the left shelf. Desk 2: calculator
    maximised + terminal on the right shelf. Desk 3: weather, then
    fullscreen (its own space after desk 3)."""
    rig.mark("setup: desk 1 notes, clock")
    first_frames(rig, 2)
    rig.frames(30, real=0.03)
    n = by_app(rig, "notes")
    if n:
        rig.mouse("click", *caption_point(n))
        rig.frames(5)
    rig.mark("setup: park notes left (SUPER+CTRL+A)")
    rig.key("KeyA", logo=1, ctrl=1)
    rig.frames(50, real=0.03)
    rig.mark("setup: new desk 2 (SUPER+0)")
    rig.key("Key0", **SUPER)
    rig.frames(50, real=0.03)
    launch(rig, "calculator", 3)
    launch(rig, "terminal", 4)
    rig.mark("setup: park terminal right (SUPER+CTRL+D)")
    rig.key("KeyD", logo=1, ctrl=1)
    rig.frames(50, real=0.03)
    c = by_app(rig, "calculator")
    if c:
        rig.mark("setup: maximise calculator (double click caption)")
        x, y = caption_point(c)
        rig.mouse("click", x, y)
        rig.frames(2)
        rig.mouse("click", x, y)
        rig.frames(60, real=0.03)
    rig.mark("setup: new desk 3 (SUPER+0)")
    rig.key("Key0", **SUPER)
    rig.frames(50, real=0.03)
    launch(rig, "weather", 5)
    rig.mark("setup: fullscreen weather (SUPER+F)")
    rig.key("KeyF", **SUPER)
    rig.frames(80, real=0.03)
    rig.mouse("move", 640, 400)
    rig.frames(30, real=0.03)


def swipes(rig, fast):
    # From the fullscreen space at the end: left to desk 3, desk 2, desk 1 and back.
    seq = ["ArrowLeft"] * 3 + ["ArrowRight"] * 3 + ["ArrowLeft"] * 3
    for k in seq:
        rig.mark(f"SUPER+{k}" + (" (fast)" if fast else ""))
        rig.key(k, **SUPER)
        rig.frames(6 if fast else 45, real=0.02)
    rig.frames(60, real=0.03)
    if fast:
        rig.mark("fast back and forth")
        for k in ["ArrowRight", "ArrowLeft", "ArrowRight", "ArrowRight", "ArrowLeft", "ArrowLeft"]:
            rig.key(k, **SUPER)
            rig.frames(3, real=0.02)
        rig.frames(60, real=0.03)


def reveal(rig, w):
    """An auto-hidden shelf shows when the pointer goes to its screen edge."""
    side = (w.get("pose") or {}).get("side", 0)
    if side == 0:
        return
    fr = rig.last()
    scr = fr["disp"][0]["r"]
    x = scr[0] + 1 if side < 0 else scr[0] + scr[2] - 1
    y = w["s"][1] + w["s"][3] * 0.5
    rig.mark(f"reveal shelf {side}: pointer to the edge")
    rig.mouse("move", x, y)
    rig.frames(40, real=0.03)


def shelf_drag(rig):
    """Pull notes off the left shelf of desk 1 and drop it back."""
    rig.mark("go to desk 1 (SUPER+1)")
    rig.key("Key1", **SUPER)
    rig.frames(50, real=0.03)
    n = by_app(rig, "notes")
    if not n:
        rig.mark("NO notes window")
        return
    reveal(rig, n)
    n = by_app(rig, "notes")
    s = n["s"]
    x0, y0 = s[0] + s[2] * 0.5, s[1] + s[3] * 0.5
    rig.mark(f"pull notes out of the shelf from {x0:.0f},{y0:.0f}")
    path = [(x0, y0)] + [(x0 + 12 * j, y0 - 2 * j) for j in range(1, 40)]
    rig.drag(path, frames_per_step=1, release=True)
    rig.frames(60, real=0.03)
    n = by_app(rig, "notes")
    s = n["s"]
    x0, y0 = s[0] + s[2] * 0.5, s[1] + 12
    rig.mark("drag notes back to the left edge, dwell, release (edge shelve)")
    path = [(x0, y0)] + [(x0 - (x0 - 1) * j / 30.0, y0) for j in range(1, 31)]
    rig.drag(path, frames_per_step=1, dwell_end=20, release=True)
    rig.frames(80, real=0.03)
    n = by_app(rig, "notes")
    reveal(rig, n)
    n = by_app(rig, "notes")
    s = n["s"]
    rig.mark("flick notes out of the shelf")
    path = [(s[0] + s[2] * 0.5, s[1] + s[3] * 0.5), (s[0] + s[2] * 0.5 + 150, s[1] + s[3] * 0.5), (s[0] + 500, s[1] + s[3] * 0.5)]
    rig.drag(path, frames_per_step=1, release=True)
    rig.frames(80, real=0.03)
    rig.mark("park notes left again (SUPER+CTRL+A)")
    rig.key("KeyA", logo=1, ctrl=1)
    rig.frames(80, real=0.03)


def unfold(rig):
    rig.mark("go to desk 1 (SUPER+1)")
    rig.key("Key1", **SUPER)
    rig.frames(50, real=0.03)
    n = by_app(rig, "notes")
    if not n:
        return
    reveal(rig, n)
    n = by_app(rig, "notes")
    s = n["s"]
    rig.mark("click notes tile: unfold")
    rig.mouse("click", s[0] + s[2] * 0.5, s[1] + s[3] * 0.5)
    rig.frames(60, real=0.03)
    rig.mark("Escape: fold")
    rig.key("Escape")
    rig.frames(60, real=0.03)
    rig.mark("unfold then swipe right while unfolded")
    n = by_app(rig, "notes")
    reveal(rig, n)
    n = by_app(rig, "notes")
    s = n["s"]
    rig.mouse("click", s[0] + s[2] * 0.5, s[1] + s[3] * 0.5)
    rig.frames(30, real=0.03)
    rig.key("ArrowRight", **SUPER)
    rig.frames(50, real=0.03)
    rig.mark("and back")
    rig.key("ArrowLeft", **SUPER)
    rig.frames(60, real=0.03)


def modes(rig):
    rig.mark("go to desk 1 (SUPER+1)")
    rig.key("Key1", **SUPER)
    rig.frames(50, real=0.03)
    c = by_app(rig, "clock")
    if not c:
        return
    x, y = caption_point(c)
    rig.mark("maximise clock (double click)")
    rig.mouse("click", x, y)
    rig.frames(2)
    rig.mouse("click", x, y)
    rig.frames(60, real=0.03)
    rig.mark("restore clock (double click)")
    c = by_app(rig, "clock")
    x, y = caption_point(c)
    rig.mouse("click", x, y)
    rig.frames(2)
    rig.mouse("click", x, y)
    rig.frames(60, real=0.03)
    rig.mark("fullscreen clock (SUPER+F)")
    rig.key("KeyF", **SUPER)
    rig.frames(70, real=0.03)
    rig.mark("leave fullscreen (SUPER+F)")
    rig.key("KeyF", **SUPER)
    rig.frames(70, real=0.03)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("scenario")
    ap.add_argument("--autohide", default="")
    ap.add_argument("--tag")
    ap.add_argument("--realtime", action="store_true", help="no virtual clock: real pacing (analyzed at --hz 60)")
    ap.add_argument("--screens", default="", help="MAKEPAD_WM_FAKE_SCREENS: the per-screen desk of Linux direct, e.g. eDP-1:0,0,1280,800")
    a = ap.parse_args()
    tag = a.tag or f"{a.scenario}-{a.autohide or 'shown'}{'-screens' if a.screens else ''}".replace(",", "+")
    files = {"wm/current-shelf-autohide": a.autohide.replace(",", " ") + "\n"} if a.autohide else {}
    env = {"MAKEPAD_WM_FAKE_SCREENS": a.screens} if a.screens else {}
    if a.realtime:
        tag += "-rt"
    rig = Rig(a.root, tag, apps="notes,clock", files=files, env=env, virtual=not a.realtime).up()
    try:
        build_desks(rig, a.autohide)
        if a.scenario in ("swipe", "all"):
            swipes(rig, False)
        if a.scenario in ("swipe-fast", "all"):
            swipes(rig, True)
        if a.scenario in ("shelf", "all"):
            shelf_drag(rig)
        if a.scenario in ("unfold", "all"):
            unfold(rig)
        if a.scenario in ("modes", "all"):
            modes(rig)
        rig.mark("end")
        rig.frames(5)
    finally:
        rig.down()
    here = os.path.dirname(os.path.abspath(__file__))
    r = sp.run([sys.executable, os.path.join(here, "analyze.py"), rig.trace, "--marks", os.path.join(rig.out, "marks.json")] + (["--hz", "60"] if a.realtime else ["--no-pacing"]))
    sys.exit(r.returncode)


if __name__ == "__main__":
    main()
