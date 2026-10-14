#!/usr/bin/env python3
"""The AI hub panel's model glow (hidden WM, node 165, MAKEPAD_WM_HUB_STUB=1).

    glowgrab.py <checkout> [--out DIR] [--dark] [--icon X,Y]

Opens the hub panel from the bar and grabs it about once a second for 25 s (the stub works
12 polls of 20, plus a vision job in the quiet stretch), so the grabs cover idle, working
and just-finished rows. Real time, no virtual clock.
"""
import argparse, os, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "frametrace"))
from ftrig import Rig  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("--out", default=os.path.expanduser("~/lanes/hubglow-runs"))
ap.add_argument("--dark", action="store_true")
ap.add_argument("--icon", default="")
ap.add_argument("--bar", action="store_true", help="only grab the bar, to find the icon")
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)
rig = Rig(a.root, "hubglow", apps="", virtual=False, out=a.out, env={"MAKEPAD_WM_HUB_STUB": "1"}).up()
try:
    time.sleep(3)
    rig.grab(os.path.join(a.out, "bar.png"))
    if a.bar:
        sys.exit(0)
    if a.dark:
        rig.mouse("click", 98, 12)
        time.sleep(1)
        rig.mouse("move", 182, 282)
        time.sleep(0.2)
        rig.mouse("click", 182, 282)
        time.sleep(1)
    x, y = (float(v) for v in a.icon.split(","))
    rig.mouse("move", x, y)
    time.sleep(0.2)
    rig.mouse("click", x, y)
    t0 = time.time()
    n = 0
    while time.time() - t0 < 25:
        rig.grab(os.path.join(a.out, f"{'dark' if a.dark else 'light'}-{n:02d}.png"))
        n += 1
        time.sleep(0.6)
finally:
    rig.down()
