"""Frame-exact WM rig: an owned headless Weston, the WM under --remote
--virtual-clock with MAKEPAD_WM_FRAMETRACE, driven one frame at a time.

    from ftrig import Rig
    rig = Rig(root, "tag", apps="notes,clock").up()
    rig.frames(30)                 # 30 frames, real time given to the clients between them
    rig.key("ArrowRight", logo=1)  # SUPER+Right
    rig.frames(40)
    rig.down()                     # quits, reaps only the processes it started
    # rig.trace is the JSONL path; analyze.py reads it

Hidden only (MAKEPAD_HIDE_WINDOWS=1, SANDBOX_MUTE=1, a private Weston
socket). Kills only the PIDs it recorded (Weston, the WM, the WM's
children by pid + start time). Needs a release build with frametrace.
"""
import json, os, signal, subprocess as sp, tempfile, time

HOME = os.path.expanduser("~")


def _birth(pid):
    try:
        return open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[19]
    except OSError:
        return None


class Rig:
    def __init__(self, root, tag, apps="", env=None, size="1280x800", scale="1.3", dpi=2, real=0.012, out=None, files=None, virtual=True):
        self.root, self.tag, self.apps = root, tag, apps
        self.env = env or {}
        # MAKEPAD_HOME-relative files written before the WM starts (settings).
        self.files = files or {}
        self.size, self.scale, self.dpi, self.real = size, scale, dpi, real
        # virtual: --virtual-clock, one frame per `frames` step; else the WM
        # runs in real time and `frames(n)` waits n/60 s (pacing is real).
        self.virtual = virtual
        self.out = out or os.path.join(HOME, "lanes", "wm-review-runs", tag)
        os.makedirs(self.out, exist_ok=True)
        self.trace = os.path.join(self.out, "frames.jsonl")
        self.log = os.path.join(self.out, "wm.log")
        self.R = os.path.join(root, "target/release/makepad-remote")
        self.owned = {}  # pid -> birth
        self.kids = {}
        self.marks = []  # (frame line count at the mark, label)

    # ---- process lifetime -------------------------------------------------
    def up(self):
        if os.path.exists(self.trace):
            os.unlink(self.trace)  # one literal path, ours
        xdg = f"/run/user/{os.getuid()}"
        self.sock = f"wayland-ftrig-{''.join(c for c in self.tag if c.isalnum())}-{os.getpid()}"
        e = dict(os.environ, XDG_RUNTIME_DIR=xdg)
        e = {k: v for k, v in e.items() if not k.startswith(("MAKEPAD_", "RELAX_", "RAPID_"))}
        w, h = (int(v) for v in self.size.split("x"))
        self.wp = sp.Popen(["nice", "-n", "10", "weston", "--backend=headless", "--renderer=gl", f"--socket={self.sock}",
                            f"--width={w + 200}", f"--height={h + 200}", "--idle-time=0"],
                           stdout=open(os.path.join(self.out, "weston.log"), "w"), stderr=sp.STDOUT, env=e, start_new_session=True)
        self.owned[self.wp.pid] = _birth(self.wp.pid)
        for _ in range(100):
            if "weston-desktop-shell" in open(os.path.join(self.out, "weston.log"), errors="replace").read():
                break
            time.sleep(0.1)
        self.state = tempfile.mkdtemp(prefix=f"ftrig-{self.tag}-")
        for rel, text in self.files.items():
            path = os.path.join(self.state, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                f.write(text)
        e.update(WAYLAND_DISPLAY=self.sock, SANDBOX_MUTE="1", MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_WM_BACKDROP="warp-grid",
                 MAKEPAD_WM_NO_WARM="1", MAKEPAD_WM_ROOT=self.root, MAKEPAD_HOME=self.state,
                 XDG_CONFIG_HOME=self.state + "/c", XDG_DATA_HOME=self.state + "/d", XDG_CACHE_HOME=self.state + "/k",
                 MAKEPAD_WM_FRAMETRACE=self.trace, CARGO_NET_OFFLINE="true", MAKEPAD_CEF_OFFLINE="1")
        if self.apps:
            e["MAKEPAD_WM_TEST_APP"] = self.apps
        e.update(self.env)
        args = [os.path.join(self.root, "target/release/makepad-app-wm"), "--remote", f"--window={self.size}@{self.dpi}",
                f"-scale={self.scale}"] + (["--virtual-clock"] if self.virtual else [])
        self.ap = sp.Popen(["nice", "-n", "10"] + args, cwd=self.root, stdout=open(self.log, "w"), stderr=sp.STDOUT,
                           env=e, start_new_session=True)
        self.owned[self.ap.pid] = _birth(self.ap.pid)
        for _ in range(300):
            if "socket=" in open(self.log, errors="replace").read():
                break
            if self.ap.poll() is not None:
                raise RuntimeError("wm exited at start; see " + self.log)
            time.sleep(0.1)
        return self

    def _track(self):
        rows = sp.run(["ps", "-e", "-o", "pid=,ppid="], capture_output=True, text=True).stdout.split("\n")
        par = {}
        for r in rows:
            if r.strip():
                c, p = map(int, r.split())
                par.setdefault(p, []).append(c)
        todo = list(par.get(self.ap.pid, []))
        while todo:
            c = todo.pop()
            b = _birth(c)
            if b:
                self.kids[c] = b
            todo.extend(par.get(c, []))

    def down(self):
        self._track()
        try:
            self.r("quit")
            self.ap.wait(timeout=10)
        except Exception:
            pass
        for pid, b in list(self.owned.items()) + list(self.kids.items()):
            if _birth(pid) == b:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        end = time.time() + 6
        while time.time() < end and any(_birth(p) == b for p, b in list(self.owned.items()) + list(self.kids.items())):
            time.sleep(0.1)
        for pid, b in list(self.owned.items()) + list(self.kids.items()):
            if _birth(pid) == b:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    # ---- remote -----------------------------------------------------------
    def r(self, *a):
        p = sp.run([self.R, "app", str(self.ap.pid), *[str(x) for x in a]], capture_output=True, text=True, timeout=90)
        return p.stdout.strip()

    def frames(self, n=1, real=None):
        """n frames of 1/60 s, giving the clients `real` seconds between frames."""
        real = self.real if real is None else real
        if not self.virtual:
            time.sleep(n / 60.0)
            self._track()
            return
        for _ in range(n):
            self.r("step", "frames=1")
            if real:
                time.sleep(real)
        self._track()

    def settle(self, max_frames=600, real=0.03):
        for _ in range(max_frames // 10):
            self.frames(10, real)
            s = self.r("settled")
            try:
                if json.loads(s).get("settled"):
                    return True
            except ValueError:
                pass
        return False

    def mark(self, label):
        """Label the next frame line in the trace (analyze.py prints it)."""
        n = sum(1 for _ in open(self.trace)) if os.path.exists(self.trace) else 0
        self.marks.append((n + 1, label))
        with open(os.path.join(self.out, "marks.json"), "w") as f:
            json.dump(self.marks, f)

    def key(self, code, **mods):
        m = [f"{k}=1" for k, v in mods.items() if v]
        self.r("key", f"c={code}", "k=down", *m)
        self.r("key", f"c={code}", "k=up", *m)

    def mouse(self, k, x, y, **extra):
        return self.r("mouse", f"k={k}", f"x={x:.1f}", f"y={y:.1f}", *[f"{a}={b}" for a, b in extra.items()])

    def drag(self, path, frames_per_step=1, dwell_end=0, release=True, real=None):
        """Press at path[0], move through the rest one step per frame batch, optionally release."""
        x, y = path[0]
        self.mouse("move", x, y)
        self.frames(2, real)
        self.mouse("down", x, y)
        self.frames(1, real)
        for (x, y) in path[1:]:
            self.mouse("move", x, y)
            self.frames(frames_per_step, real)
        if dwell_end:
            self.frames(dwell_end, real)
        if release:
            self.mouse("up", x, y)
            self.frames(1, real)

    def grab(self, path):
        return self.r("grab", f"out={path}")

    def launch(self, app):
        return self.r("call", "os.launch", json.dumps({"app": app}))

    # ---- trace ------------------------------------------------------------
    def last(self):
        """The newest complete frame record (the WM may be writing the next line as it is read:
        a line without its newline is not finished yet)."""
        last = None
        with open(self.trace) as f:
            for line in f:
                if line.endswith("\n"):
                    last = line
        return json.loads(last) if last else None

    def window(self, client=None, app_index=None):
        """Live (not captured) records of the newest frame, by client."""
        fr = self.last()
        live = [w for w in fr["w"] if w["cap"] is None]
        if client is not None:
            return next((w for w in live if w["c"] == client), None)
        return live
