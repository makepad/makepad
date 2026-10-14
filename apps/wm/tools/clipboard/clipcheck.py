#!/usr/bin/env python3
"""Clipboard through the WM (hidden, node 165): a hosted makepad app and Wayland clients.

    clipcheck.py <checkout> [--out DIR] [--hold]

Needs release builds of makepad-app-wm, makepad-app-notes, makepad-remote and the
wayland_server example wlclip. Steps:
  1. Notes: type a line, select all, cut (ctrl+X). The cut text reaches the Wayland
     clients: a wlclip window on the WM's own server is offered exactly it (one cut, not
     an empty second one).
  2. A Wayland client copies (wlclip copy): the focused Wayland window hears it and the WM
     takes it as the makepad clipboard (its log; headless weston has no seat, so the WM's
     own clipboard cannot be read back through weston here).
  3. The Wayland source quits: its text stays the WM server's selection.
  4. Notes copies (ctrl+A, ctrl+C): the Wayland window is offered it.
Prints one line per check and exits 1 on a failure.
"""
import argparse, os, subprocess as sp, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "frametrace"))
from ftrig import Rig  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("--out")
ap.add_argument("--hold", action="store_true", help="leave the WM up at the end (for a look)")
a = ap.parse_args()
WL = os.path.join(a.root, "target/release/examples/wlclip")
XDG = f"/run/user/{os.getuid()}"
fails = []


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)
    if not ok:
        fails.append(name)


def wait_for(pred, secs, step=0.25):
    end = time.time() + secs
    while time.time() < end:
        v = pred()
        if v:
            return v
        time.sleep(step)
    return pred()


def lines(path):
    return open(path).read().splitlines() if os.path.exists(path) else []


out = a.out or os.path.join(os.path.expanduser("~"), "lanes", "wm-review-runs", "wmclip")
log1 = os.path.join(out, "paste-wm.log")
log2 = os.path.join(out, "paste-weston.log")
for f in (log1, log2):
    if os.path.exists(f):
        os.unlink(f)
# The WM starts Wayland apps from its catalog (HOME/.makepad/wm/wayland_apps.txt): a home of
# the check's own lists wlclip; cargo and rustup keep the real ones.
home = os.path.join(out, "home")
os.makedirs(os.path.join(home, ".makepad", "wm"), exist_ok=True)
with open(os.path.join(home, ".makepad", "wm", "wayland_apps.txt"), "w") as f:
    f.write(f"wlclip | WlClip | | {WL} paste --watch --log={log1}\n")
real = os.path.expanduser("~")
env = {"HOME": home, "CARGO_HOME": os.environ.get("CARGO_HOME", os.path.join(real, ".cargo")),
       "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", os.path.join(real, ".rustup"))}
rig = Rig(a.root, "wmclip", apps="notes", virtual=False, out=out, env=env).up()
kids = []
try:
    def notes():
        fr = rig.last() if os.path.exists(rig.trace) else None
        if not fr:
            return None
        for w in fr["w"]:
            if w["cap"] is None and w.get("app") == "notes" and w.get("host") and w["host"].get("has"):
                return w
        return None
    w = wait_for(notes, 400, 1.0)
    check("notes is up", bool(w))
    if not w:
        raise SystemExit
    time.sleep(2)
    x, y, ww, hh = w["d"]
    # 1. cut in Notes
    rig.mouse("move", x + ww * 0.6, y + hh * 0.5)
    rig.mouse("down", x + ww * 0.6, y + hh * 0.5)
    rig.mouse("up", x + ww * 0.6, y + hh * 0.5)
    time.sleep(0.5)
    rig.r("text", "t=cut from notes 42")
    time.sleep(0.5)
    rig.key("KeyA", ctrl=1)
    time.sleep(0.3)
    rig.key("KeyX", ctrl=1)
    time.sleep(1.0)
    rig.grab(os.path.join(rig.out, "after-cut.png"))
    print("launch:", rig.launch("wl.wlclip"), flush=True)
    sock = wait_for(lambda: next((l.split(" on ")[-1].strip() for l in lines(rig.log) if "started wayland client" in l), None), 10)
    check("the WM serves Wayland", bool(sock), sock or "")

    def wlwin():
        fr = rig.last()
        return next((w for w in fr["w"] if w["cap"] is None and w.get("app") == "wl.wlclip"), None)
    v = wait_for(wlwin, 20)
    check("the wlclip window is up", bool(v))
    if v:
        x, y, ww, hh = v["d"]
        rig.mouse("move", x + ww / 2, y + hh / 2)
        rig.mouse("down", x + ww / 2, y + hh / 2)
        rig.mouse("up", x + ww / 2, y + hh / 2)
    got = wait_for(lambda: [l for l in lines(log1) if l.startswith("selection:")], 15)
    check("a Wayland window is offered the text cut in Notes (one cut, not an empty second one)", bool(got) and "cut from notes 42" in open(log1).read(), f"{[g[:80] for g in got]}")
    # a selection is printed as is: the whole note (select all) spans lines
    # 2. a Wayland client copies
    env = dict(os.environ, XDG_RUNTIME_DIR=XDG, WAYLAND_DISPLAY=sock)
    src = sp.Popen([WL, "copy", "copied in wayland ✓"], env=env, stdout=sp.PIPE, text=True)
    kids.append(src)
    got = wait_for(lambda: "selection: copied in wayland ✓" in lines(log1), 10)
    check("the focused Wayland window hears the new selection", got, f"{[l for l in lines(log1) if l.startswith('selection:')]}")
    took = wait_for(lambda: [l for l in lines(rig.log) if "wayland client copied" in l], 10)
    n = len("copied in wayland ✓".encode())
    check("the WM takes it as the makepad clipboard", any(f"copied {n} bytes" in l for l in took), f"{took}")
    # 3. the source quits
    src.terminate()
    src.wait()
    time.sleep(1.5)
    got = [l for l in lines(log1) if l.startswith("selection:")]
    check("the text outlives its source", got[-1:] == ["selection: copied in wayland ✓"] and len(got) >= 3, f"{got}")
    # 4. Notes copies again: the Wayland window hears it at once
    rig.mouse("move", w["d"][0] + w["d"][2] * 0.97, w["d"][1] + w["d"][3] * 0.5)
    rig.mouse("down", w["d"][0] + w["d"][2] * 0.97, w["d"][1] + w["d"][3] * 0.5)
    rig.mouse("up", w["d"][0] + w["d"][2] * 0.97, w["d"][1] + w["d"][3] * 0.5)
    time.sleep(0.5)
    rig.r("text", "t=second line")
    time.sleep(0.3)
    rig.key("KeyA", ctrl=1)
    time.sleep(0.3)
    rig.key("KeyC", ctrl=1)
    time.sleep(0.5)
    rig.launch("wl.wlclip")  # brings the running wlclip to the front (keyboard focus)
    got = wait_for(lambda: [l for l in lines(log1) if l.startswith("selection:") and "second line" in l], 10)
    check("a copy in Notes replaces the Wayland selection", bool(got), f"{[l for l in lines(log1) if l.startswith('selection:')][-2:]}")
    rig.grab(os.path.join(rig.out, "end.png"))
    if a.hold:
        input("held; enter ends")
finally:
    for k in kids:
        if k.poll() is None:
            k.terminate()
    rig.down()
print("RESULT", "FAIL " + ", ".join(fails) if fails else "PASS")
sys.exit(1 if fails else 0)
