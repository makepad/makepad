#!/usr/bin/env python3
"""The WM's portal backend on the session bus (hidden WM, node 165): Settings for dark mode.

    portalcheck.py <checkout> [--out DIR]

Needs a release build of makepad-app-wm and makepad-remote, busctl and dbus-monitor, and a
user session bus where nothing else owns org.freedesktop.impl.portal.desktop.makepad.
Checks: Read and ReadAll answer color-scheme; the Properties version; Introspect; flipping
the desktop's Dark mode (shell menu) sends SettingChanged and Read follows.
Exit 1 on a failure.
"""
import argparse, json, os, re, subprocess as sp, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "frametrace"))
from ftrig import Rig  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("root")
ap.add_argument("--out")
a = ap.parse_args()
NAME, PATH = "org.freedesktop.impl.portal.desktop.makepad", "/org/freedesktop/portal/desktop"
SETTINGS = "org.freedesktop.impl.portal.Settings"
fails = []


def check(name, ok, detail=""):
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)
    if not ok:
        fails.append(name)


def bus(*args):
    p = sp.run(["busctl", "--user", *args], capture_output=True, text=True, timeout=10)
    return (p.stdout + p.stderr).strip()


def scheme():
    out = bus("call", NAME, PATH, SETTINGS, "Read", "ss", "org.freedesktop.appearance", "color-scheme")
    m = re.match(r"v u (\d+)", out)
    return int(m.group(1)) if m else out


out = a.out or os.path.join(os.path.expanduser("~"), "lanes", "wm-review-runs", "wmportal")
rig = Rig(a.root, "wmportal", apps="", virtual=False, out=out).up()
mon = None
try:
    up = None
    for _ in range(120):
        if "wm portal: serving" in open(rig.log).read():
            up = True
            break
        time.sleep(0.5)
    check("the WM serves the portal name", bool(up), [l for l in open(rig.log).read().splitlines() if "portal" in l][-1:])
    s0 = scheme()
    check("Read color-scheme answers 1 (dark) or 2 (light)", s0 in (1, 2), f"{s0}")
    all_ = bus("call", NAME, PATH, SETTINGS, "ReadAll", "as", "1", "org.freedesktop.*")
    check("ReadAll org.freedesktop.* has the appearance", "org.freedesktop.appearance" in all_ and "color-scheme" in all_, all_)
    none = bus("call", NAME, PATH, SETTINGS, "ReadAll", "as", "1", "org.gnome.*")
    check("ReadAll of another namespace is empty", none.startswith("a{sa{sv}} 0"), none)
    nf = bus("call", NAME, PATH, SETTINGS, "Read", "ss", "org.freedesktop.appearance", "nope")
    check("an unknown key is NotFound", "NotFound" in nf or "no such setting" in nf, nf)
    ver = bus("get-property", NAME, PATH, SETTINGS, "version")
    check("the version property", ver == "u 1", ver)
    intro = bus("introspect", NAME, PATH)
    check("Introspect lists Settings", SETTINGS in intro and "SettingChanged" in intro, intro[:120])
    # live flip: open the shell menu's Dark mode row and press it
    mon_log = os.path.join(out, "signals.txt")
    mon = sp.Popen(["dbus-monitor", "--session", f"type='signal',interface='{SETTINGS}'"], stdout=open(mon_log, "w"), stderr=sp.STDOUT)
    time.sleep(0.5)
    # the shell menu opens from the bar's leftmost (palette) button
    time.sleep(2)
    rig.mouse("click", 98, 12)
    time.sleep(1.0)
    rig.grab(os.path.join(out, "menu.png"))
    snap = ""
    print("snap:", snap[:300], flush=True)
    flipped = False
    # the menu is drawn, not widgets: its "Dark mode" row sits at (182, 282) on the rig's
    # 1280x800 desk (the 8th row under the bar)
    m = True
    if m:
        rig.mouse("move", 182, 282)
        time.sleep(0.2)
        rig.mouse("click", 182, 282)
        time.sleep(1.5)
        s1 = scheme()
        flipped = s1 in (1, 2) and s1 != s0
        check("Dark mode flips Read", flipped, f"{s0} -> {s1}")
        sig = open(mon_log).read()
        check("SettingChanged went out with the new scheme", "SettingChanged" in sig and f"uint32 {s1}" in sig, sig[-300:])
    else:
        check("the Dark mode row is on screen (for the live flip)", False, snap[:200])
finally:
    if mon:
        mon.terminate()
    rig.down()
print("RESULT", "FAIL " + ", ".join(fails) if fails else "PASS")
sys.exit(1 if fails else 0)
