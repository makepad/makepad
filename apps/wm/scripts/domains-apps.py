#!/usr/bin/env python3
"""Writes one gate script per launcher app of libs/app_registry/apps.json into <out dir>:
launch it from the WM's shell menu (the menu filters on KeyDown), use it, close it with the WM
binding, launch it again (warm). Run each with the command in domains.hal's header, one
fresh MAKEPAD_HOME per app. usage: domains-apps.py <out dir> [app id ...]"""
import json, os, sys
out = sys.argv[1]
only = set(sys.argv[2:])
os.makedirs(out, exist_ok=True)
apps = json.load(open(os.path.join(os.path.dirname(__file__), "../../../libs/app_registry/apps.json")))
def keys(label):
    return "\n".join("key Key" + c.upper() for c in label if c.isalpha()) + "\n"
for a in apps:
    if a["wm_launchable"] and not a["menu_visible"] and (not only or a["id"] in only):
        # Not in any menu: boot it by registry id (MAKEPAD_WM_TEST_APP=<id>), the WM's own rig;
        # the assistant is the pane the F10 key opens.
        h = f"# gate for {a['id']} ({a['package']}), hidden from the menu: MAKEPAD_WM_TEST_APP={a['id']}\nwait_frame\n"
        if a["id"] == "aichat": h += "sleep_frames 20\nkey F10\n"
        h += f"domain {a['package']}\nwait_frame\nsleep_frames 60\ndomain -\nsnapshot {a['id']}_first\n"
        h += "click 700 450\nsleep_frames 20\nsnapshot " + a["id"] + "_used\n"
        h += f"key F5 ctrl\nsleep_frames 120\ndomain {a['package']}\ndomain -\nsnapshot {a['id']}_relaunched\n"
        h += "key KeyW logo\nsleep_frames 120\nsnapshot " + a["id"] + "_closed\n"
        open(os.path.join(out, a["id"] + ".hal"), "w").write(h)
        continue
    if not (a["wm_launchable"] and a["menu_visible"]): continue
    if only and a["id"] not in only: continue
    word = a["label"].split()[0]
    launch = f"key Space logo alt\nsleep_frames 10\n{keys(word)}key ReturnKey\ndomain {a['package']}\nwait_frame\nsleep_frames 60\n"
    s = f"# gate for {a['id']} ({a['package']})\nwait_frame\nsleep_frames 20\n"
    s += launch + f"domain -\nsnapshot {a['id']}_first\n"
    s += "click 700 450\nsleep_frames 20\nsnapshot " + a["id"] + "_used\n"
    s += "key KeyW logo\nsleep_frames 180\n"
    s += launch + f"domain -\nsnapshot {a['id']}_warm\n"
    open(os.path.join(out, a["id"] + ".hal"), "w").write(s)
