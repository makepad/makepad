#!/usr/bin/env python3
"""WM usability gate: plain user tasks, scripted with human-like pointer
timing against the real WM binary and real apps (hidden, node 165).

    suite.py <checkout-with-release-build> [--only name,name] [--out DIR]

Each task acts like a person (pointer moves ~every 16 ms, presses, dwells),
then checks the end state from the WM's motion recorder (window rects, mode,
shelf membership, focus, desktop) and the motion in between with
../frametrace/analyze.py (no size or position jumps, no shelf flash, no
blank captures, no hidden reconfigure). A screenshot ends every task.
Prints one PASS/FAIL table; exit 1 on any FAIL.
"""
import argparse, json, os, sys, time, traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "frametrace"))
from ftrig import Rig  # noqa: E402
import analyze  # noqa: E402

# Motion findings that fail a task. STRETCH (a stale frame shown at a new
# size for a moment) and pacing are reported, not failed.
BAD = {"SIZE-NO-TWEEN", "SIZE-JUMP", "POS-JUMP", "HANDOFF", "ORIGIN", "SHELF-SHOWN", "BLANK-SHOT",
       "DEGENERATE", "CFG-HIDDEN", "NO-FRAME"}
STEP = 0.016  # s between pointer moves


class Fail(Exception):
    pass


class T:
    """The session: a rig plus person-like actions and state reads."""

    def __init__(self, rig, out):
        self.rig, self.out = rig, out
        self.fixups = []

    # ---- state -------------------------------------------------------------
    def frames(self):
        return analyze.load(self.rig.trace)

    def last(self):
        return self.rig.last()

    def n(self):
        f = self.last()
        return f["n"] if f else 0

    def win(self, app, fr=None):
        fr = fr or self.last()
        for w in fr["w"]:
            if w["cap"] is None and w.get("app") == app:
                return w
        return None

    def cid(self, app):
        w = self.win(app)
        if not w:
            raise Fail(f"no {app} window on show")
        return w["c"]

    def hit(self, app, kind):
        fr = self.last()
        c = self.cid(app)
        for h in fr.get("hits", []):
            if h["c"] == c and h["k"] == kind:
                return h["r"]
        raise Fail(f"{app} has no '{kind}' hit rect drawn")

    def screen(self):
        return self.last()["disp"][0]["r"]

    def still(self, timeout=4.0):
        """Wait until nothing animates (no frame for a while or anim false)."""
        end = time.time() + timeout
        time.sleep(0.15)
        while time.time() < end:
            a = self.last()
            time.sleep(0.25)
            b = self.last()
            if a["n"] == b["n"] or not b.get("anim"):
                if not b.get("anim"):
                    time.sleep(0.1)
                    return
            time.sleep(0.05)

    # ---- person ------------------------------------------------------------
    def move(self, x, y):
        self.rig.mouse("move", x, y)
        time.sleep(STEP)

    def glide(self, a, b, steps=None, speed=900.0):
        """Pointer from a to b at about `speed` pt/s, a sample every STEP."""
        dist = ((b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2) ** 0.5
        steps = steps or max(2, int(dist / (speed * STEP)))
        for i in range(1, steps + 1):
            t = i / steps
            e = t * t * (3 - 2 * t)  # a hand eases in and out
            self.move(a[0] + (b[0] - a[0]) * e, a[1] + (b[1] - a[1]) * e)

    def drag(self, a, b, dwell=0.0, speed=900.0, release=True):
        self.move(*a)
        time.sleep(0.12)
        self.rig.mouse("down", *a)
        time.sleep(0.08)
        self.glide(a, b, speed=speed)
        if dwell:
            time.sleep(dwell)
        if release:
            self.rig.mouse("up", *b)
            time.sleep(0.05)

    def click(self, x, y):
        self.move(x, y)
        time.sleep(0.08)
        self.rig.mouse("down", x, y)
        time.sleep(0.07)
        self.rig.mouse("up", x, y)
        time.sleep(0.05)

    def dclick(self, x, y):
        self.click(x, y)
        time.sleep(0.09)
        self.rig.mouse("down", x, y)
        time.sleep(0.06)
        self.rig.mouse("up", x, y)

    def key(self, code, **mods):
        self.rig.key(code, **mods)
        time.sleep(0.05)

    def center(self, r):
        return (r[0] + r[2] / 2, r[1] + r[3] / 2)

    def caption(self, app):
        return self.center(self.hit(app, "title"))

    def uncovered_point(self, app):
        """A point of the window no window above it covers (None: covered)."""
        fr = self.last()
        w = self.win(app, fr)
        order = fr["zorder"]
        if w["c"] not in order:
            return None
        above = [x["s"] for x in fr["w"] if x["cap"] is None and x["c"] in order and order.index(x["c"]) > order.index(w["c"])]
        s = w["s"]
        for fy in (0.03, 0.5, 0.9, 0.3, 0.7):
            for fx in (0.5, 0.2, 0.8, 0.97, 0.03):
                if fx < 0.1 and fy < 0.2:
                    continue          # the traffic lights
                p = (s[0] + s[2] * fx, s[1] + max(6, s[3] * fy))
                if not any(a[0] <= p[0] <= a[0] + a[2] and a[1] <= p[1] <= a[1] + a[3] for a in above):
                    return p
        return None

    def raise_(self, app):
        fr = self.last()
        c = self.cid(app)
        if fr["zorder"] and fr["zorder"][-1] == c:
            return
        p = self.uncovered_point(app)
        if p is None:
            raise Fail(f"{app} is fully covered")
        self.click(*p)
        self.still()

    def ensure_normal(self, app):
        """Precondition: a normal, unshelved window on top (fixes it up if not)."""
        w = self.win(app)
        if w is None:
            raise Fail(f"no {app} window")
        if w.get("parked") or (w.get("pose") or {}).get("side"):
            self.click(*self.center(w["s"]))
            self.still()
            self.key("ArrowDown", logo=1)
            self.still()
            w = self.win(app)
            if w.get("parked") or (w.get("pose") or {}).get("side"):
                # still on (or warped toward) a shelf: drag it to the middle
                scr = self.screen()
                self.drag(self.center(w["s"]), (scr[0] + scr[2] * 0.5, scr[1] + scr[3] * 0.4), speed=800)
                self.still()
                self.fixups.append(f"{app} had to be dragged off a shelf")
        w = self.win(app)
        if w and w.get("mode") == 1:
            self.dclick(*self.caption(app))
            self.still()
        w = self.win(app)
        if not w or w.get("parked") or w.get("mode") != 0:
            raise Fail(f"precondition: {app} is not a normal window ({w and (w.get('mode'), w.get('parked'))})")
        self.raise_(app)
        return self.win(app)


def close(a, b, tol=6.0):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def size_of(w):
    return w["s"][2], w["s"][3]


# ---- the tasks -----------------------------------------------------------------
# Each returns a note; raise Fail for a wrong end state.

def t_move(t):
    w = t.ensure_normal("notes")
    a = t.caption("notes")
    # Within the centre band (a drop that overlaps a shelf zone is eased back into it).
    t.drag(a, (a[0] + 90, a[1] + 60))
    t.still()
    v = t.win("notes")
    dx, dy = v["s"][0] - w["s"][0], v["s"][1] - w["s"][1]
    if not (close((dx, dy), (90, 60), 8) and close(size_of(v), size_of(w), 2)):
        raise Fail(f"moved by ({dx:.0f},{dy:.0f}) size {size_of(w)} -> {size_of(v)}; want (90,60), same size")
    return f"moved ({dx:.0f},{dy:.0f})"


def t_move_near_edge(t):
    w = t.ensure_normal("notes")
    a = t.caption("notes")
    scr = t.screen()
    # The window's left edge to 24 pt from the screen edge; the pointer stays far from it.
    dx = (scr[0] + 24) - w["s"][0]
    t.drag(a, (a[0] + dx, a[1]), dwell=0.4)
    t.still()
    v = t.win("notes")
    if v.get("parked") or (v.get("pose") or {}).get("side") or not close(size_of(v), size_of(w), 2):
        raise Fail(f"near the edge it changed: parked={v.get('parked')} side={v['pose']['side']} size {size_of(w)} -> {size_of(v)}")
    t.drag(t.caption("notes"), (scr[0] + scr[2] / 2, scr[1] + 120))
    t.still()
    return f"left edge at {v['s'][0]:.0f}"


def resize_task(edge, d):
    def run(t):
        w = t.ensure_normal("clock")
        r = t.hit("clock", edge)
        a = t.center(r)
        sx = {"-": -1, "0": 0, "+": 1}[edge[1]]
        sy = {"-": -1, "0": 0, "+": 1}[edge[2]]
        t.drag(a, (a[0] + sx * d, a[1] + sy * d), speed=500)
        t.still()
        v = t.win("clock")
        dw, dh = v["s"][2] - w["s"][2], v["s"][3] - w["s"][3]
        want = (abs(sx) * d, abs(sy) * d)
        if v.get("parked") or not close((dw, dh), want, 10):
            raise Fail(f"resize {edge} by {d}: size change ({dw:.0f},{dh:.0f}) want {want}, parked={v.get('parked')}")
        # and back
        r = t.hit("clock", edge)
        a = t.center(r)
        t.drag(a, (a[0] - sx * d, a[1] - sy * d), speed=500)
        t.still()
        return f"({dw:.0f},{dh:.0f})"
    return run


def t_resize_to_screen_edge(t):
    w = t.ensure_normal("clock")
    scr = t.screen()
    a = t.center(t.hit("clock", "r-0"))
    t.drag(a, (scr[0], a[1]), dwell=0.6, speed=700)
    t.still()
    v = t.win("clock")
    if v.get("parked") or (v.get("pose") or {}).get("side"):
        raise Fail(f"resizing to the screen edge shelved it (parked={v.get('parked')} side={v['pose']['side']})")
    if v["s"][2] <= w["s"][2] + 20:
        raise Fail(f"did not grow: {w['s'][2]:.0f} -> {v['s'][2]:.0f}")
    # corner to the screen corner
    a = t.center(t.hit("clock", "r--"))
    t.drag(a, (scr[0], scr[1]), dwell=0.6, speed=700)
    t.still()
    v = t.win("clock")
    if v.get("parked") or (v.get("pose") or {}).get("side"):
        raise Fail("resizing to the screen corner shelved it")
    # back to a sane size
    a = t.center(t.hit("clock", "r--"))
    t.drag(a, (a[0] + 260, a[1] + 120), speed=700)
    t.still()
    return f"width {w['s'][2]:.0f} -> {v['s'][2]:.0f}, never shelved"


def t_maximise_restore(t):
    w = t.ensure_normal("clock")
    t.dclick(*t.caption("clock"))
    t.still()
    v = t.win("clock")
    if v.get("mode") != 1:
        raise Fail(f"double click: mode {v.get('mode')} (want maximised)")
    t.dclick(*t.caption("clock"))
    t.still()
    u = t.win("clock")
    if u.get("mode") != 0 or not close(u["s"], w["s"], 4):
        raise Fail(f"restore: mode {u.get('mode')} rect {u['s']} want {w['s']}")
    return f"max {v['s'][2]:.0f}x{v['s'][3]:.0f}, restored"


def t_fullscreen_restore(t):
    w = t.ensure_normal("clock")
    t.click(*t.center(t.hit("clock", "max")))
    t.still(6)
    v = t.win("clock")
    scr = t.screen()
    fr = t.last()
    if not v or v.get("mode") != 2:
        raise Fail(f"green button: mode {v and v.get('mode')} (want fullscreen)")
    # top-edge caption reveal
    t.glide((scr[0] + scr[2] / 2, scr[1] + 200), (scr[0] + scr[2] / 2, scr[1] + 1))
    time.sleep(0.8)
    try:
        cap = t.hit("clock", "max")
    except Fail:
        raise Fail("fullscreen: no caption at the top edge")
    t.click(*t.center(cap))
    t.still(6)
    u = t.win("clock")
    if not u or u.get("mode") != 0 or not close(u["s"], w["s"], 6):
        raise Fail(f"restore: mode {u and u.get('mode')} rect {u and u['s']} want {w['s']}")
    return f"fullscreen {v['s'][2]:.0f}x{v['s'][3]:.0f} on desktop {fr['disp'][0]['cur']}, caption revealed, restored"


def shelve(t, app):
    scr = t.screen()
    a = t.caption(app)
    t.drag(a, (scr[0] + 1, a[1]), dwell=0.45, speed=900)
    t.still()
    v = t.win(app)
    if not v or not v.get("parked"):
        raise Fail(f"edge drag + dwell did not shelve {app}")
    return v


def t_shelve_edge(t):
    w = t.ensure_normal("notes")
    v = shelve(t, "notes")
    return f"tile {v['s'][2]:.0f}x{v['s'][3]:.0f}"


def t_tile_half_fold(t):
    v = t.win("notes")
    if not v or not v.get("parked"):
        shelve(t, "notes")
        v = t.win("notes")
    t.click(*t.center(v["s"]))
    t.still()
    h = t.win("notes")
    if not h.get("half"):
        raise Fail("click on the tile: no half-size unfold")
    scr = t.screen()
    t.click(scr[0] + scr[2] * 0.6, scr[1] + scr[3] * 0.5)
    t.still()
    f = t.win("notes")
    if f.get("half") or not f.get("parked"):
        raise Fail(f"click elsewhere: half={f.get('half')} parked={f.get('parked')}")
    return f"half {h['s'][2]:.0f}x{h['s'][3]:.0f}, folded"


def t_pull_out_tile(t, full=None):
    v = t.win("notes")
    if not v or not v.get("parked"):
        shelve(t, "notes")
        v = t.win("notes")
    scr = t.screen()
    a = t.center(v["s"])
    t.drag(a, (scr[0] + scr[2] * 0.5, scr[1] + scr[3] * 0.45), speed=800)
    t.still()
    u = t.win("notes")
    if u.get("parked") or (u.get("pose") or {}).get("side") or u.get("mode") != 0:
        raise Fail(f"pull-out: still parked={u.get('parked')} side={u['pose']['side']}")
    return f"out at {u['s'][2]:.0f}x{u['s'][3]:.0f}"


def t_pull_out_half(t):
    shelve(t, "notes")
    v = t.win("notes")
    t.click(*t.center(v["s"]))
    t.still()
    h = t.win("notes")
    if not h.get("half"):
        raise Fail("no half unfold")
    scr = t.screen()
    a = t.caption("notes")
    t.drag(a, (scr[0] + scr[2] * 0.55, scr[1] + scr[3] * 0.4), speed=800)
    t.still()
    u = t.win("notes")
    if u.get("parked") or u.get("half"):
        raise Fail(f"pull-out from half: parked={u.get('parked')} half={u.get('half')}")
    return f"out at {u['s'][2]:.0f}x{u['s'][3]:.0f}"


def t_esc_fold(t):
    shelve(t, "notes")
    v = t.win("notes")
    t.click(*t.center(v["s"]))
    t.still()
    if not t.win("notes").get("half"):
        raise Fail("no half unfold")
    t.key("Escape")
    t.still()
    f = t.win("notes")
    if f.get("half") or not f.get("parked"):
        raise Fail(f"Esc: half={f.get('half')} parked={f.get('parked')}")
    return "folded by Esc"


def t_super_down(t):
    v = t.win("notes")
    if not v.get("parked"):
        shelve(t, "notes")
    v = t.win("notes")
    t.click(*t.center(v["s"]))  # focus it (unfolds)
    t.still()
    t.key("ArrowDown", logo=1)
    t.still()
    u = t.win("notes")
    if u.get("parked") or u.get("half"):
        raise Fail(f"SUPER+Down: parked={u.get('parked')} half={u.get('half')}")
    return "un-shelved"


def t_desktops(t):
    t.ensure_normal("notes")
    shelve(t, "notes")
    fr0 = t.last()
    start = fr0["disp"][0]["cur"]
    t.key("Key0", logo=1)  # a new desktop, shown
    t.still()
    for k in ["ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "ArrowLeft"]:
        t.key(k, logo=1)
        t.still()
    fr = t.last()
    if fr["disp"][0]["cur"] != start:
        raise Fail(f"back on desktop {fr['disp'][0]['cur']}, want {start}")
    # fast
    for k in ["ArrowRight", "ArrowLeft", "ArrowRight", "ArrowLeft"]:
        t.key(k, logo=1)
        time.sleep(0.08)
    t.still()
    v = t.win("notes")
    if not v or not v.get("parked"):
        raise Fail("the shelved window is not on its shelf after the switches")
    return f"{len(fr['disp'][0]['slots'])} desktops"


def dock_icon(t, app):
    for d in t.last().get("dock", []):
        if d["app"] == app:
            return d["r"]
    raise Fail(f"no {app} icon in the dock")


def shown(w):
    """The app has presented its first frame into the window (what a person waits for before
    reaching for its buttons: until then the window may still take the size the app asks)."""
    return bool(w and w.get("host") and w["host"].get("has"))


def t_dock_open_close_reopen(t):
    app = "calculator"
    r = dock_icon(t, app)
    t.click(*t.center(r))
    deadline = time.time() + 60
    while time.time() < deadline and not shown(t.win(app)):
        time.sleep(0.5)
    t.still()
    w = t.win(app)
    if not w:
        raise Fail("dock click: no window")
    if t.last().get("focus") != w["c"]:
        raise Fail("dock click: the new window has no focus")
    t.click(*t.center(t.hit(app, "close")))
    deadline = time.time() + 10
    while time.time() < deadline and t.win(app):
        time.sleep(0.3)
    if t.win(app):
        raise Fail("close button: still there")
    t.click(*t.center(dock_icon(t, app)))
    deadline = time.time() + 60
    while time.time() < deadline and not shown(t.win(app)):
        time.sleep(0.5)
    t.still()
    if not t.win(app):
        raise Fail("reopen from the dock: no window")
    return "opened, closed, reopened"


def t_focus_click(t):
    t.ensure_normal("clock")
    t.ensure_normal("notes")
    for app in ("clock", "notes"):
        w = t.win(app)
        # a point of this window not covered by any later one
        fr = t.last()
        order = fr["zorder"]
        above = [x["s"] for x in fr["w"] if x["cap"] is None and x["c"] in order and order.index(x["c"]) > order.index(w["c"])]
        pt = None
        s = w["s"]
        for fx in (0.15, 0.5, 0.85, 0.97, 0.03):
            for fy in (0.3, 0.6, 0.9, 0.97):
                p = (s[0] + s[2] * fx, s[1] + s[3] * fy)
                if not any(a[0] <= p[0] <= a[0] + a[2] and a[1] <= p[1] <= a[1] + a[3] for a in above):
                    pt = p
                    break
            if pt:
                break
        if not pt:
            raise Fail(f"{app} is fully covered")
        t.click(*pt)
        t.still()
        if t.last().get("focus") != w["c"]:
            raise Fail(f"click on {app}: focus is {t.last().get('focus')}")
    return "focus followed both clicks"


def t_minimise_restore(t):
    w = t.ensure_normal("clock")
    t.click(*t.center(t.hit("clock", "min")))
    t.still()
    v = t.win("clock")
    if v and not v.get("min"):
        raise Fail("minimise: still on show")
    t.click(*t.center(dock_icon(t, "clock")))
    t.still()
    u = t.win("clock")
    if not u or u.get("min") or not close(u["s"], w["s"], 6):
        raise Fail(f"restore from the dock: {u and u['s']} want {w['s']}")
    return "minimised, restored from the dock"


def t_overlap_raise(t):
    a = t.ensure_normal("notes")
    b = t.ensure_normal("clock")
    # clock over notes, offset
    ca = t.caption("clock")
    t.drag(ca, (ca[0] + (a["s"][0] + 120 - b["s"][0]), ca[1] + (a["s"][1] + 60 - b["s"][1])))
    t.still()
    t.click(*t.caption("clock"))
    t.still()
    fr = t.last()
    if fr["zorder"][-1] != t.cid("clock"):
        raise Fail("clock not on top after its click")
    n = t.win("notes")
    t.click(n["s"][0] + 30, n["s"][1] + n["s"][3] - 30)
    t.still()
    fr = t.last()
    if fr["zorder"][-1] != t.cid("notes") or fr.get("focus") != t.cid("notes"):
        raise Fail(f"click on the covered notes: top {fr['zorder'][-1]}, focus {fr.get('focus')}")
    return "raised by click"


# The size setup() gives its two windows (the layout the tasks after it start from): apps
# open at the size they ask for (955d138a8b), so setup sizes them to this rather than by a
# fixed shrink of whatever they opened at.
SETUP_SIZE = (680, 290)


def setup(t):
    """Two smaller windows side by side in the centre band, clear of the dock."""
    for app, (x, y) in (("notes", (200, 50)), ("clock", (600, 240))):
        t.raise_(app)
        w = t.win(app)
        a = t.center(t.hit(app, "r++"))
        t.drag(a, (a[0] - (w["s"][2] - SETUP_SIZE[0]), a[1] - (w["s"][3] - SETUP_SIZE[1])), speed=1200)
        t.still()
        w = t.win(app)
        c = t.caption(app)
        t.drag(c, (c[0] + x - w["s"][0], c[1] + y - w["s"][1]), speed=1500)
        t.still()


TASKS = [
    ("move a window", t_move),
    ("move near an edge (no shelve)", t_move_near_edge),
] + [(f"resize from {e}", resize_task(e, 60)) for e in ("r+0", "r-0", "r0+", "r0-", "r++", "r--", "r+-", "r-+")] + [
    ("resize to the screen edge (never shelves)", t_resize_to_screen_edge),
    ("maximise by double click, restore", t_maximise_restore),
    ("fullscreen (green), top caption, restore", t_fullscreen_restore),
    ("shelve via the edge", t_shelve_edge),
    ("tile click -> half, click elsewhere -> fold", t_tile_half_fold),
    ("pull out from the tile", t_pull_out_tile),
    ("pull out from the half window", t_pull_out_half),
    ("Esc folds the half window", t_esc_fold),
    ("SUPER+Down un-shelves", t_super_down),
    ("switch desktops back and forth", t_desktops),
    ("dock: open, close, reopen", t_dock_open_close_reopen),
    ("focus follows click", t_focus_click),
    ("minimise and restore", t_minimise_restore),
    ("overlap and raise", t_overlap_raise),
]


# ---- screenshots ---------------------------------------------------------------
# The WM's HOME is a folder of its own, so ~/Screenshots is ours.

def shots(t):
    d = os.path.join(t.rig.env["HOME"], "Screenshots")
    return sorted(f for f in (os.listdir(d) if os.path.isdir(d) else []) if f.endswith(".png"))


def png_size(path):
    from PIL import Image
    with Image.open(path) as im:
        return im.size


def wait_new_shot(t, before, secs=8.0):
    end = time.time() + secs
    while time.time() < end:
        now = shots(t)
        if len(now) > len(before):
            return [f for f in now if f not in before][0]
        time.sleep(0.2)
    raise Fail("no screenshot file was saved")


def shot_path(t, name):
    return os.path.join(t.rig.env["HOME"], "Screenshots", name)


def region_drag(t, a, b):
    t.key("Key4", logo=1, shift=1)
    time.sleep(1.2)  # the frame is taken, then the overlay comes up
    t.drag(a, b, speed=700)


def t_shot_region(t):
    scr = t.screen()
    before = shots(t)
    a = (scr[0] + 300, scr[1] + 200)
    region_drag(t, a, (a[0] + 300, a[1] + 200))
    name = wait_new_shot(t, before)
    w, h = png_size(shot_path(t, name))
    scale = 2.0
    if abs(w - 300 * scale) > 3 or abs(h - 200 * scale) > 3:
        raise Fail(f"{name} is {w}x{h}, want about {int(300 * scale)}x{int(200 * scale)}")
    if not name.startswith("screenshot-") or not name.endswith(".png"):
        raise Fail(f"unexpected name {name}")
    return f"{name} {w}x{h}"


def t_shot_screen(t):
    scr = t.screen()
    before = shots(t)
    t.key("Key3", logo=1, shift=1)
    name = wait_new_shot(t, before)
    w, h = png_size(shot_path(t, name))
    if abs(w - scr[2] * 2) > 3 or abs(h - scr[3] * 2) > 3:
        raise Fail(f"{name} is {w}x{h}, want the screen {int(scr[2] * 2)}x{int(scr[3] * 2)}")
    return f"{name} {w}x{h}"


def t_shot_cancel(t):
    before = shots(t)
    t.key("Key4", logo=1, shift=1)
    time.sleep(1.2)
    t.key("Escape")
    time.sleep(1.0)
    if shots(t) != before:
        raise Fail("Esc still saved a picture")
    return "Esc cancels"


def t_shot_window(t):
    before = shots(t)
    # The window on top (apps open at their asked sizes, centred: Code's 1440x900 lies over
    # the middle of Notes's 1240x800), picked at its centre.
    fr = t.last()
    order = fr["zorder"]
    w = max((t.win(app, fr) for app in ("notes", "code")), key=lambda x: order.index(x["c"]) if x and x["c"] in order else -1)
    s = w["s"]
    t.key("Key4", logo=1, shift=1)
    time.sleep(1.2)
    t.key("Space")
    time.sleep(0.2)
    p = (s[0] + s[2] / 2, s[1] + s[3] / 2)
    t.move(*p)
    time.sleep(0.2)
    t.click(*p)
    name = wait_new_shot(t, before)
    pw, ph = png_size(shot_path(t, name))
    # The app's own image: the window's width, its height less the WM's caption strip.
    if abs(pw - s[2] * 2) > 6 or not (s[3] * 2 * 0.85 <= ph <= s[3] * 2 + 6):
        raise Fail(f"{name} is {pw}x{ph}, want the window's own image about {int(s[2] * 2)}x{int(s[3] * 2)}")
    return f"{name} {pw}x{ph}"


def t_shot_thumbnail_into_code(t):
    scr = t.screen()
    before = shots(t)
    a = (scr[0] + 120, scr[1] + 120)
    region_drag(t, a, (a[0] + 300, a[1] + 200))
    name = wait_new_shot(t, before)
    time.sleep(0.8)  # it eases in
    # The thumbnail: 240x160 at the screen's bottom-right corner, 20 pt in.
    tx, ty = scr[0] + scr[2] - 20 - 120, scr[1] + scr[3] - 20 - 80
    code = t.win("code")
    if not code:
        raise Fail("no code window")
    s = code["s"]
    target = (s[0] + s[2] * 0.7, s[1] + s[3] * 0.55)
    n0 = len(open(t.rig.log, errors="replace").read())
    t.drag((tx, ty), target, speed=500)
    end = time.time() + 6
    log = ""
    while time.time() < end:
        log = open(t.rig.log, errors="replace").read()[n0:]
        if "code: attached" in log and "dropped on client" in log:
            break
        time.sleep(0.3)
    if "dropped on client" not in log:
        raise Fail("the WM did not hand the file to a window")
    if "code: attached" not in log or name not in log:
        raise Fail(f"Code did not take {name} as an attachment")
    return f"{name} attached in Code"


SHOT_TASKS = [
    ("screenshot: rectangle", t_shot_region),
    ("screenshot: Esc cancels", t_shot_cancel),
    ("screenshot: whole screen", t_shot_screen),
    ("screenshot: pick a window", t_shot_window),
    ("screenshot: thumbnail dropped into Code", t_shot_thumbnail_into_code),
]

AUTOHIDE_TASKS = [
    ("auto-hide: shelve via the edge", t_shelve_edge),
    ("auto-hide: tile click -> half -> fold", t_tile_half_fold),
    ("auto-hide: pull out from the tile", t_pull_out_tile),
    ("auto-hide: switch desktops", t_desktops),
]


def run_session(root, out, tasks, autohide, only, rows, screens, desk, shots_session=False):
    files = {"wm/current-shelf-autohide": "left right\n"} if autohide else {}
    env = {"MAKEPAD_WM_FAKE_SCREENS": screens} if screens else {}
    tag = "usability-autohide" if autohide else "usability-shots" if shots_session else "usability"
    apps = "notes,code" if shots_session else "notes,clock"
    if shots_session:
        # A home of its own: ~/Screenshots is the session's.
        env["HOME"] = os.path.join(out, tag, "home")
        os.makedirs(env["HOME"], exist_ok=True)
    rig = Rig(root, tag, apps=apps, files=files, env=env, size=desk, virtual=False, out=os.path.join(out, tag)).up()
    t = T(rig, out)
    try:
        deadline = time.time() + 120
        while time.time() < deadline:
            fr = t.last() if os.path.exists(rig.trace) else None
            first, second = apps.split(",")
            if fr and t.win(first, fr) and t.win(second, fr) and all(w["host"] and w["host"]["has"] for w in fr["w"] if w["cap"] is None and w.get("host")):
                break
            time.sleep(1)
        t.still()
        if not shots_session:
            setup(t)
        for name, fn in tasks:
            if only and not any(o in name for o in only):
                continue
            n0 = t.n()
            t0 = time.time()
            t.fixups = []
            status, note = "PASS", ""
            try:
                note = fn(t) or ""
            except Fail as e:
                status, note = "FAIL", str(e)
            except Exception as e:  # a broken step is a failure too
                status, note = "FAIL", f"{type(e).__name__}: {e}"
                traceback.print_exc()
            time.sleep(0.2)
            n1 = t.n()
            frames = t.frames()
            lo = next((i for i, f in enumerate(frames) if f["n"] > n0), len(frames))
            hi = next((i for i, f in enumerate(frames) if f["n"] > n1), len(frames))
            issues = analyze.analyze(frames, lo, hi, hz=60.0)
            bad = [it for it in issues if it["kind"] in BAD]
            warn = [it for it in issues if it["kind"] not in BAD and it["kind"] not in ("LONG-FRAME", "SLOW-DRAW")]
            longs = [it for it in issues if it["kind"] == "LONG-FRAME"]
            if bad and status == "PASS":
                status = "FAIL"
            shot = os.path.join(rig.out, f"{len(rows) + 1:02d}.png")
            try:
                rig.grab(shot)
            except Exception:
                shot = ""
            if t.fixups:
                note += " [precondition fix-up: " + "; ".join(t.fixups) + "]"
            rows.append(dict(name=name, status=status, note=note, frames=(n0 + 1, n1), bad=bad, warn=warn,
                             longs=len(longs), secs=time.time() - t0, shot=shot, trace=rig.trace))
            print(f"{status} {name}: {note} {'; '.join(b['kind'] + '#' + str(b['n']) for b in bad[:4])}", flush=True)
    finally:
        rig.down()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--only", default="")
    ap.add_argument("--out", default=os.path.expanduser("~/lanes/wm-review-runs/usability"))
    # A desk like the real screens (149, a laptop): apps open at the size they ask for
    # (955d138a8b) -- Notes 1240x800, Code 1440x900 -- and fit on it beside each other.
    ap.add_argument("--desk", default="1920x1200", help="the WM window's size in points (the Rig's weston output)")
    ap.add_argument("--screens", default=None, help="fake per-screen desk like Linux direct ('' = nested single desk; default: one screen the size of --desk)")
    ap.add_argument("--no-autohide", action="store_true")
    a = ap.parse_args()
    if a.screens is None:
        a.screens = "eDP-1:0,0,{},{}".format(*a.desk.split("x"))
    only = [o for o in a.only.split(",") if o]
    rows = []
    t0 = time.time()
    def wanted(tasks):
        return not only or any(o in name for name, _ in tasks for o in only)
    if wanted(TASKS):
        run_session(a.root, a.out, TASKS, False, only, rows, a.screens, a.desk)
    if not a.no_autohide and wanted(AUTOHIDE_TASKS):
        run_session(a.root, a.out, AUTOHIDE_TASKS, True, only, rows, a.screens, a.desk)
    if wanted(SHOT_TASKS):
        run_session(a.root, a.out, SHOT_TASKS, False, only, rows, a.screens, a.desk, shots_session=True)
    print()
    print(f"{'#':>2}  {'task':<46} {'result':<6} {'frames':<13} detail")
    for i, r in enumerate(rows, 1):
        det = r["note"]
        if r["bad"]:
            det += " | " + ", ".join(f"{b['kind']} #{b['n']} {b['c']}" for b in r["bad"][:3]) + (f" (+{len(r['bad']) - 3})" if len(r["bad"]) > 3 else "")
        if r["warn"]:
            kinds = sorted(set(w["kind"] for w in r["warn"]))
            det += " | warn " + ",".join(kinds)
        print(f"{i:>2}  {r['name']:<46} {r['status']:<6} {r['frames'][0]:>5}-{r['frames'][1]:<6} {det}")
    fails = sum(r["status"] == "FAIL" for r in rows)
    print(f"\n{len(rows) - fails}/{len(rows)} pass in {time.time() - t0:.0f} s; traces + screenshots in {a.out}")
    with open(os.path.join(a.out, "table.json"), "w") as f:
        json.dump(rows, f, indent=1, default=str)
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
