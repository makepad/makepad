#!/usr/bin/env python3
"""The window manager at rest draws nothing and costs next to nothing.

usage: wm_rest.py [--makepad <makepad binary>] [--secs <s, 20>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given)

On 11-07 every node's WM drew at rest, 11-25 % of a core: a sticky toast asked for frames as if
counting down, a window waiting to be hidden redrew every frame, the closed AI pane's client
presented at its own rate behind the desk's edge, and each second's thermometer reading redrew the
bar. Each has its own unit test; this guards the sum, and whatever comes next.

The WM runs under the JIT, hidden and muted, in fresh homes. Once it says it is settled (`inspect
settled`) five seconds running, its draws and its UI thread's CPU are measured for --secs seconds (`inspect draws`,
`inspect uicounts`) with the frame-source and bar traces on. It passes with at most one frame (a new minute
on the clock) and under 1 % of a core, in one of up to four windows (a client the desktop starts
may still be starting in the first; a loop fills them all); else it prints the trace's lines,
what made the frames, and the WM's own lines.
Prints `wm_rest: ok ...` or `wm_rest: FAIL <why>`; exit 1 on failure.
"""
import json, os, re, shutil, subprocess, sys, tempfile, threading, time

ROOT = os.getcwd()
# a cold JIT builds the whole WM before its first frame
START = float(os.environ.get("WM_REST_START", "900"))
SETTLE = float(os.environ.get("WM_REST_SETTLE", "120"))
WINDOWS = int(os.environ.get("WM_REST_WINDOWS", "4"))


class Run:
    """The WM's live run, its stderr lines as they arrive."""
    def __init__(self, jit, work):
        self.jit, self.err, self.report = jit, [], os.path.join(work, "report")
        home = os.path.join(work, "runhome")
        for d in ("home", "t"):
            os.makedirs(os.path.join(work, d), exist_ok=True)
        env = dict(os.environ, MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", SANDBOX_MUTE="1", MAKEPAD_HOME=os.path.join(work, "home"),
                   TMPDIR=os.path.join(work, "t"), MAKEPAD_RUN_HOME=home, MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"),
                   MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"))
        env.setdefault("MAKEPAD_RUN_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
        self.env = env
        self.p = subprocess.Popen(jit + ["run", "--report-to", self.report, "makepad-app-wm", "--", "--remote"], cwd=ROOT, env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, errors="replace", start_new_session=True)
        threading.Thread(target=self.pump, daemon=True).start()

    def pump(self):
        for line in self.p.stderr:
            self.err.append(line.rstrip("\n"))

    def presented(self):
        try:
            return "present" in open(self.report, errors="replace").read()
        except OSError:
            return False

    def wait(self, pred, secs, every=0.5):
        t = time.time()
        while time.time() - t < secs:
            if pred():
                return True
            if self.p.poll() is not None:
                return pred()
            time.sleep(every)
        return False

    def inspect(self, *words):
        m = next((re.search(r"makepad-remote: pid (\d+)", l) for l in self.err if "makepad-remote: pid " in l), None)
        pid = m.group(1) if m else str(self.p.pid)
        c = subprocess.run(self.jit + ["inspect", pid] + list(words), capture_output=True, text=True, errors="replace", env=self.env)
        return (c.stdout + c.stderr).strip()

    def end(self):
        try:
            os.killpg(self.p.pid, 15)
        except ProcessLookupError:
            pass
        self.p.wait()


def settled(said):
    try:
        return json.loads(said).get("settled") is True
    except ValueError:
        return False


def cpu_ns(said):
    m = re.search(r'"cpu_ns":(\d+)', said)
    return int(m.group(1)) if m else None


def sources(r):
    """The frame-source trace's lines from the WM's log."""
    try:
        lines = json.loads(r.inspect("log", "n=400")).get("l", [])
    except ValueError:
        return []
    out, on = [], False
    for l in lines:
        if "[frame-sources]" in l:
            out.append(l[l.index("[frame-sources]"):])
            on = True
        elif " - bar redraw: " in l:
            out.append(l[l.index("bar redraw: "):][:300])
            on = False
        elif on and l.startswith(" "):
            out.append(l)
        else:
            on = False
    return out


def said_lines(r):
    """The WM's own log lines (`wm: ...`) of its last 400."""
    try:
        lines = json.loads(r.inspect("log", "n=400")).get("l", [])
    except ValueError:
        return []
    return [l[l.index(" - wm: ") + 3:][:200] for l in lines if " - wm: " in l]


def check(jit, work, secs):
    r = Run(jit, work)
    try:
        if not r.wait(r.presented, START):
            return "no first frame (%s)" % ("the run ended: " + " | ".join(r.err[-6:]) if r.p.poll() is not None else "after %.0f s" % START)
        # at rest: settled five seconds running (a client the desktop starts can come just after)
        calm = [0]
        def calm_for(n):
            calm[0] = calm[0] + 1 if settled(r.inspect("settled")) else 0
            return calm[0] >= n
        if not r.wait(lambda: calm_for(5), SETTLE, every=1.0):
            r.inspect("trace", "topics=frame_sources,bar")
            time.sleep(3)
            r.inspect("trace", "topics=")
            return "never settled in %.0f s: %s; %s" % (SETTLE, r.inspect("settled"), " | ".join(sources(r)[-12:]))
        # a client the desktop starts (the assistant, cold under the JIT) may still be starting:
        # a window that drew at rest is measured again, up to WINDOWS; a loop fills every one
        said = None
        for n in range(1, WINDOWS + 1):
            r.inspect("trace", "topics=frame_sources,bar")
            before = cpu_ns(r.inspect("uicounts"))
            r.inspect("draws", "start", "s=%d" % (secs + 5))
            t0 = time.time()
            time.sleep(secs)
            after = cpu_ns(r.inspect("uicounts"))
            took = time.time() - t0
            draws = r.inspect("draws", "n=6")
            r.inspect("draws", "stop")
            r.inspect("trace", "topics=")
            m = re.search(r"draws: (\d+) frames? over", draws)
            frames = int(m.group(1)) if m else (0 if "nothing redrew" in draws else None)
            if frames is None or before is None or after is None:
                return "no measure: draws %r, uicounts %r -> %r" % (draws[:300], before, after)
            ms = (after - before) / 1e6
            said = "%d frames, %.0f ms UI-thread CPU over %.0f s (%.2f %% of a core), window %d of %d" % (frames, ms, took, ms / took / 10.0, n, WINDOWS)
            print("wm_rest: " + said, flush=True)
            if frames <= 1 and ms <= took * 10.0:
                return None
        return "%s; what made them:\n  %s\nthe WM said:\n  %s\n%s" % (said, "\n  ".join(sources(r)[-24:]), "\n  ".join(said_lines(r)[-16:]), draws)
    finally:
        r.end()


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
elif os.environ.get("MAKEPAD_LOADER"):
    jit = [os.environ["MAKEPAD_LOADER"]]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
secs = int(args[args.index("--secs") + 1]) if "--secs" in args else 20
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="wm-rest-", dir=os.path.join(ROOT, "target"))
why = check(jit, work, secs)
if why:
    print("wm_rest: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work, ignore_errors=True)
print("wm_rest: ok")
