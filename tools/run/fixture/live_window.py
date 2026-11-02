#!/usr/bin/env python3
"""A live update never changes a running app's window state.

usage: live_window.py [--app <package>] [--edit <rel path> <old> <new>]... [--relaunch] [--host <rel path>]
                      [--live-resize] [--geom "X Y W H"]
  (run from the workspace root, on a Mac with a display: a visible window on the real GPU backend, at the
   display's own scale -- 2 on a retina display; writes target/live_window/)

The app runs under this tree's JIT. Its window is resized in small steps (shrinking and growing, the way a
person drags an edge) before any update, after each live edit (the JIT patches the running program; a file
with script_mod! designs also re-applies them), after `session relaunch` (--relaunch: the restart in place)
and after a restart into a rebuilt makepad (--host <file of makepad run's sources>: makepad rebuilds, offers
the restart, the click). After every step the app's window table, the scale its main pass rasterised at and
the drawn surface must agree: surface = round(size * dpi), pass_dpi = dpi. A window whose app keeps another
size or scale than its surface draws warped and takes input offset (JITUPDATE3's XR scale, JITRESIZE).
--live-resize: the window is also dragged by its corner every few seconds through AppKit's live-resize loop
(MAKEPAD_TEST_LIVE_RESIZE=repeat), and each drag's end is checked the same way.
Defaults: the runner, one design edit of its main.rs and one edit of Window::handle_event (widgets), the
first edit a person's landing most often brings.

Ends with `live_window: <app> N checks, M mismatches`; exit 3 on a mismatch or an edit without a verdict.
"""
import json, os, re, shutil, subprocess, sys, time

ROOT = os.getcwd()
WORK = os.path.join(ROOT, "target", "live_window")
JIT = ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--"]
CLIENT = os.path.join(WORK, "makepad-client")
VERDICT = 240  # save to the JIT's verdict on an edit, on a busy box
COLD = 1500  # a cold first start of a big app on a busy box
STATUS = re.compile(r'\{"i":(\d+),"t":"(?:[^"\\]|\\.)*","sz":\[([-\d.]+),([-\d.]+)\],"px":\[[-\d.]+,[-\d.]+\],"dpi":([-\d.]+),"pass_dpi":([-\d.]+)')
DEFAULT_EDITS = [
    ("apps/commercial/runner/src/main.rs", "let Dim = #x8a909a", "let Dim = #x8a909b"),
    ("widgets/core/src/window.rs", "            self.last_synced_title = None;\n", "            self.last_synced_title = None;\n            self.drag_query_cache = None;\n"),
]


def opt(name, default):
    a = sys.argv[1:]
    return a[a.index(name) + 1] if name in a else default


def edits():
    a = sys.argv[1:]
    out = [tuple(a[i + 1:i + 4]) for i, w in enumerate(a) if w == "--edit"]
    return out or DEFAULT_EDITS


class App:
    def __init__(self, package):
        env = dict(os.environ, MAKEPAD_REMOTE="1", MAKEPAD_MUTE="1", SANDBOX_MUTE="1", MUSIC_MUTE="1", MAKEPAD_HOME=os.path.join(WORK, "home"))
        env.pop("MAKEPAD_HIDE_WINDOWS", None)
        if "--live-resize" in sys.argv:
            env["MAKEPAD_TEST_LIVE_RESIZE"] = "repeat"
        self.log_path = os.path.join(WORK, "app.log")
        self.log = open(self.log_path, "w")
        self.proc = subprocess.Popen(JIT + ["run", package], env=env, stdin=subprocess.DEVNULL, stdout=self.log, stderr=self.log, start_new_session=True)
        self.pid = None

    def text(self):
        with open(self.log_path, errors="replace") as f:
            return f.read()

    def ask(self, *cmd, timeout=60):
        try:
            r = subprocess.run([CLIENT, "inspect", str(self.pid), *cmd, "as=json"], capture_output=True, text=True, timeout=timeout)
            return r.returncode, (r.stdout if r.returncode == 0 else r.stdout + r.stderr)
        except subprocess.TimeoutExpired:
            return 124, "no answer in %d s" % timeout

    def wait_frame(self, seen, secs):
        t = time.time()
        while time.time() - t < secs:
            text = self.text()
            m = re.search(r"makepad-remote: pid (\d+)", text)
            if m:
                self.pid = int(m.group(1))
            if self.pid and text.count("first frame") > seen:
                return None
            if self.proc.poll() is not None:
                return "exited (%s)" % self.proc.returncode
            time.sleep(0.1)
        return "no first frame in %d s" % secs

    def stop(self):
        if self.pid and self.proc.poll() is None:
            self.ask("gq", timeout=30)
            t = time.time()
            while self.proc.poll() is None and time.time() - t < 15:
                time.sleep(0.2)
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, 9)
        self.proc.wait()
        self.log.close()


CHECKS = [0, 0]  # checks, mismatches


def check(app, label):
    """The window table, the main pass's scale and the drawn surface of the first window agree."""
    code, st = app.ask("status")
    m = STATUS.search(st)
    png = os.path.join(WORK, "%s.png" % label)
    if os.path.exists(png):
        os.remove(png)
    app.ask("grab", "out=" + png)
    try:
        head = open(png, "rb").read(24)
        surface = (int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big"))
    except OSError:
        surface = None
    CHECKS[0] += 1
    if not m or surface is None:
        CHECKS[1] += 1
        print("MISMATCH %s: no status or picture: %s" % (label, st.strip()[:200]), flush=True)
        return
    w, h, dpi, pass_dpi = (float(x) for x in m.groups()[1:])
    want = (round(w * dpi), round(h * dpi))
    ok = abs(surface[0] - want[0]) <= 1 and abs(surface[1] - want[1]) <= 1 and (pass_dpi == 0 or pass_dpi == dpi)
    if not ok:
        CHECKS[1] += 1
    print("%s %s: %gx%g at dpi %g, pass dpi %g, surface %dx%d" % ("ok" if ok else "MISMATCH", label, w, h, dpi, pass_dpi, surface[0], surface[1]), flush=True)


def settle_drag(app):
    """Until no live-resize drag is in progress."""
    if "--live-resize" not in sys.argv:
        return
    while True:
        text = app.text()
        if text.count("live_resize: start") <= text.count("live_resize: stop"):
            return
        time.sleep(0.1)


def steps(app, label, n=8):
    """Small size changes, shrinking then growing, each checked once its frame is drawn."""
    code, st = app.ask("status")
    m = STATUS.search(st)
    w, h = (float(m.group(2)), float(m.group(3))) if m else (1100.0, 720.0)
    for k in range(n):
        # never across a live-resize drag (--live-resize): the two would fight over the size
        settle_drag(app)
        d = -1 if k < n // 2 else 1
        w, h = w + d * 24, h + d * 16
        app.ask("resize", "w=%d" % w, "h=%d" % h, "wait=1")
        # the resize answers after the frame that shows it; one more frame for the grab's present
        app.ask("mouse", "k=move", "x=5", "y=5", "wait=1")
        if "--live-resize" in sys.argv:
            # a drag that began meanwhile resized it too: checked at the drag's end instead
            text = app.text()
            if text.count("live_resize: start") > text.count("live_resize: stop"):
                continue
        check(app, "%s-step%d" % (label, k))


def drags(app, label, secs=14):
    """The live-resize drags' ends in `secs` (--live-resize), each checked."""
    if "--live-resize" not in sys.argv:
        return
    seen = app.text().count("live_resize: stop")
    t = time.time()
    k = 0
    while time.time() - t < secs:
        n = app.text().count("live_resize: stop")
        if n > seen:
            seen = n
            app.ask("mouse", "k=move", "x=5", "y=5", "wait=1")
            check(app, "%s-drag%d" % (label, k))
            k += 1
        time.sleep(0.2)


def edit_and_wait(app, path, old, new):
    """One live edit: the JIT's verdict event, or None."""
    code, ev = app.ask("events")
    seen = max([int(x) for x in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
    full = os.path.join(ROOT, path)
    src = open(full).read()
    if old not in src:
        raise SystemExit("live_window: the edit's text is not in %s" % path)
    with open(full, "w") as f:
        f.write(src.replace(old, new, 1))
    t = time.time()
    while time.time() - t < VERDICT:
        code, said = app.ask("watch", "on=patchverified,patchrefused,patchreverted", "since=%d" % seen, "timeout_s=10", timeout=60)
        line = next((l for l in said.splitlines() if '"event"' in l and '"event":"none"' not in l), "")
        if line:
            return json.loads(line)
        if app.proc.poll() is not None:
            return None
    return None


def main():
    os.makedirs(WORK, exist_ok=True)
    package = opt("--app", "makepad-runner")
    subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad"], check=True)
    target = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"], capture_output=True, text=True, check=True).stdout)["target_directory"]
    shutil.copy2(os.path.join(target, "host", "makepad"), CLIENT)
    geom = opt("--geom", "")
    if geom:
        # the window's saved place and size (makepad run keeps it per app)
        h = 0xcbf29ce484222325
        for b in package.encode():
            h = ((h ^ b) * 0x100000001b3) & 0xffffffffffffffff
        d = os.path.join(target, "makepad-run", "windows")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "%016x.geom" % h), "w") as f:
            f.write(geom + "\n")
    app = App(package)
    failed = []
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_window: %s: %s\n%s" % (package, why, app.text()[-1500:]))
        app.ask("jit", "ms=100")
        check(app, "start")
        steps(app, "before")
        drags(app, "before")
        for n, (path, old, new) in enumerate(edits()):
            event = edit_and_wait(app, path, old, new)
            print("edit %d %s: %s" % (n, path, (event or {}).get("event", "no verdict")), flush=True)
            if not event:
                failed.append("edit %d (%s) got no verdict" % (n, path))
            check(app, "edit%d" % n)
            steps(app, "edit%d" % n)
            drags(app, "edit%d" % n)
        if "--relaunch" in sys.argv:
            frames = app.text().count("first frame")
            print("relaunch: %s" % app.ask("session", "relaunch")[1].strip()[:200], flush=True)
            why = app.wait_frame(frames, COLD)
            if why:
                failed.append("relaunch: %s" % why)
            else:
                check(app, "relaunch")
                steps(app, "relaunch")
                drags(app, "relaunch")
        host = opt("--host", "")
        if host:
            since = len(app.text())
            with open(os.path.join(ROOT, host), "a") as f:
                f.write("\n// live_window host edit\n")
            t = time.time()
            while time.time() - t < COLD and "a restart applies it" not in app.text()[since:]:
                time.sleep(1)
            frames = app.text().count("first frame")
            app.ask("session", "relaunch")
            why = app.wait_frame(frames, COLD)
            if why:
                failed.append("host restart: %s" % why)
            else:
                check(app, "host")
                steps(app, "host")
                drags(app, "host")
    finally:
        app.stop()
    print("live_window: %s %d checks, %d mismatches%s" % (package, CHECKS[0], CHECKS[1], "; " + "; ".join(failed) if failed else ""), flush=True)
    return 3 if CHECKS[1] or failed else 0


sys.exit(main())
