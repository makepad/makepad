#!/usr/bin/env python3
"""Nothing breaks a running app: real landings replayed as live edits under this tree's JIT.

usage: live_replay.py --app <package> [--count N] [--only <sha,..>] [--history <makepad>,<commercial>]
       live_replay.py --host --app <package> [--wm]
       live_replay.py --chain --app <package> --only <sha,..>   (one app takes them in turn, no restart)
  (run from the workspace root; results also go to target/live_replay/results-<app>.jsonl)

The JIT under test is always this tree's. The app's tree is a second tree (in the temp dir, one index
per repository, moved with `git read-tree -u` like a sync moves a node) at the landing's parent, except
the crates the JIT binary provides (makepad-run's #[relax_api] crates and their path deps) and the
JIT's own: those stay at this tree, as the loader requires them to match its build.

A landing is one first-parent commit of makepad or commercial that changes a crate the app builds
(the other repository at that time), newest first. Far enough back the tree no longer builds against
this tree's JIT and provided crates: after DRIFTED such landings in a row the run ends there:
  1. the app runs at the parent, first frame, settled, answers input; its layout at three window
     sizes, then again after `session relaunch` with no edit: the nodes that move are the app's
     state (what a restart keeps or loses), skipped by the layout checks below;
  2. the tree moves to the landing, as a sync does; the JIT's verdict is awaited;
  3. the app is alive, draws and answers input, no panic, ICE, parked thread or error view;
  4. `session relaunch` (the restart button), on the warm cache of the parent: the same again;
  5. an applied edit lays out as the restart does (stale_layout), and the restart as a new
     process of the same code (stale_restart: something of the previous run outlived it).
The verdict is the user's rule, "restart yes, update yes, crash no":
  pass: applied, or a restart that comes up working (the edit's rung is only recorded);
  fail: a crash, ICE, panic or parked thread, a dead UI, or an error view over the app;
  silent: the watcher took the edit up and gave no verdict: a bug, though the app runs on;
  skip: the tree itself does not build: rustc rejects it too (old crates against this tree's provided
        crates), so it says nothing about the JIT. rustc accepting what the JIT rejects is a fail;
  unreached: the app loads nothing the landing changed, no verdict to judge: not a pass.
--history takes the landings from those commits' first parents instead of this tree's (a history cut
down to one commit has none to replay). A control edit at the start of a run must get a verdict and the health check must see an error view
over the app, or the run stops: its checks could not fail.

--host: the path replay cannot reach, a landing that changes what makepad itself is built from. The
app runs from this tree, as a dock app runs from its node's checkout (the edits go where the job's end
resets them). Each case edits a provided crate or makepad run's own sources: makepad rebuilds in the
background, offers the restart, `session relaunch` is the click, and the app must come back working in
the same process id with its state handed over. --wm: the app is a hosted window of a WM.

Ends with one number per app: `live_replay: <app> pass rate P% (pass/judged, ...)`; exit 3 on a miss, 1
when the run could judge nothing.
"""
import json, os, re, shutil, subprocess, sys, tempfile, time, zlib

ROOT = os.getcwd()
WORK = os.path.join(ROOT, "target", "live_replay")
# outside the workspace: inside it cargo would merge both .cargo/config.toml files, the doubled
# flags a different fingerprint from the workspace's own, and every `cargo run` a rebuild
TREE = os.path.join(tempfile.gettempdir(), "live_replay_tree")
# beside the tree it describes: a move writes only what differs from the index, so an index that
# outlived its tree (the tree in a lease's temp dir, the index in target/) left files unwritten
INDEX = os.path.join(tempfile.gettempdir(), "live_replay_index")
COMMERCIAL = "apps/commercial"
# the JIT binary's providers (tools/run/Cargo.toml's host-shims dependencies)
PROVIDERS = ["makepad-live-id", "makepad-script", "makepad-script-std", "makepad-script-compute", "makepad-shader", "makepad-tween-script", "makepad-script-audio-aot", "makepad-hal", "makepad-script-census", "makepad-relax-api"]
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
# The inspect client is a copy of this tree's binary: only a client of the app's socket, and in --host
# the tree it would be rebuilt from changes under it (a cargo run per ask would race the JIT's rebuild).
CLIENT = os.path.join(WORK, "makepad-client")
REBUILD = 1500  # a rebuild of makepad on a busy box, to the offered restart
COLD = 1500  # a cold first start of a big app on a busy box
BOOT = 600  # a start after a tree move
VERDICT = int(os.environ.get("LIVE_REPLAY_VERDICT", "180"))  # save to the JIT's verdict on the edit
SETTLE = 120  # the JIT idle after a frame
DRIFTED = 5  # landings in a row whose tree rustc rejects here: history older than this JIT can run
RESIZE = "--no-resize" not in sys.argv  # an applied edit's layout is compared with a fresh start's
UNSEEN = 20  # no word from the watcher this long: the edit touched nothing the app loaded


def opt(name, default):
    args = sys.argv[1:]
    return args[args.index(name) + 1] if name in args else default


def git(*a, env=None, ok=True):
    r = subprocess.run(["git", *a], cwd=ROOT, env=env, capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit("git %s: %s" % (" ".join(a), r.stderr.strip()))
    return r.stdout.strip()


def cgit(*a, env=None, ok=True):
    r = subprocess.run(["git", *a], cwd=os.path.join(ROOT, COMMERCIAL), env=env, capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit("git (commercial) %s: %s" % (" ".join(a), r.stderr.strip()))
    return r.stdout.strip()


def metadata():
    return json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--offline"], cwd=ROOT, capture_output=True, text=True, check=True).stdout)


def closure(meta, names):
    """The dirs (in the workspace) of these packages and their path dependencies, outermost only."""
    by_id = {p["id"]: p for p in meta["packages"]}
    deps = {n["id"]: n["dependencies"] for n in meta["resolve"]["nodes"]}
    seen, todo = set(), [p["id"] for p in meta["packages"] if p["name"] in names]
    while todo:
        i = todo.pop()
        if i not in seen and by_id[i]["source"] is None:
            seen.add(i)
            todo.extend(deps.get(i, []))
    return {os.path.relpath(os.path.dirname(by_id[i]["manifest_path"]), ROOT) for i in seen}


def outermost(dirs):
    return sorted(d for d in dirs if not any(d != o and d.startswith(o + "/") for o in dirs))


def kept_dirs(meta):
    """The tree's dirs that stay at this tree: the providers' path dependency closure, the JIT's."""
    return outermost(closure(meta, PROVIDERS) | {"libs/relax", "tools/run", ".cargo"})


def landings(meta, package, kept, tip, side, count):
    """(time, repo, commit) of the landings that change a crate the app builds, oldest first."""
    dirs = outermost(closure(meta, [package]))
    found = []
    excl = [":(exclude)%s" % d for d in kept]
    ours = [d for d in dirs if not d.startswith(COMMERCIAL + "/") and d != "."]
    theirs = [os.path.relpath(d, COMMERCIAL) for d in dirs if d.startswith(COMMERCIAL + "/")]
    if ours:
        for l in git("log", "--first-parent", "--format=%ct %H", "-%d" % count, tip, "--", *ours, *excl).splitlines():
            t, sha = l.split()
            found.append((int(t), "makepad", sha))
    if theirs:
        for l in cgit("log", "--first-parent", "--format=%ct %H", "-%d" % count, side, "--", *theirs).splitlines():
            t, sha = l.split()
            found.append((int(t), "commercial", sha))
    return sorted(found)[-count:]


def hybrid(commit, tip, kept):
    """The tree object of `commit` with the kept dirs from `tip`."""
    index = INDEX + ".build"
    env = dict(os.environ, GIT_INDEX_FILE=index)
    git("read-tree", commit, env=env)
    for d in kept:
        git("rm", "-r", "-q", "-f", "--cached", "--ignore-unmatch", "--", d, env=env)
        if git("cat-file", "-t", "%s:%s" % (tip, d), ok=False) == "tree":
            git("read-tree", "--prefix=%s/" % d, "%s:%s" % (tip, d), env=env)
    return git("write-tree", env=env)


def move_tree(state):
    """The second tree moves to a (makepad tree object, commercial commit): only the files that
    differ are written."""
    tree, commercial = state
    os.makedirs(TREE, exist_ok=True)
    git("read-tree", "--reset", "-u", tree, env=dict(os.environ, GIT_INDEX_FILE=INDEX, GIT_WORK_TREE=TREE))
    if commercial:
        side = os.path.join(TREE, COMMERCIAL)
        os.makedirs(side, exist_ok=True)
        cgit("read-tree", "--reset", "-u", commercial, env=dict(os.environ, GIT_INDEX_FILE=INDEX + ".commercial", GIT_WORK_TREE=side))


class App:
    """One app under the JIT in the second tree, hidden and muted, on the real GPU backend."""

    def __init__(self, package, log, cwd=None, env=None, args=()):
        self.log_path = log
        self.package = package
        self.file = open(log, "w")
        base = dict(os.environ, MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", SANDBOX_MUTE="1", MUSIC_MUTE="1", MAKEPAD_WINDOW=os.environ.get("MAKEPAD_WINDOW", "1280x800@1"), MAKEPAD_HOME=os.path.join(WORK, "home"))
        base["MAKEPAD_REMOTE"] = "1"
        base.pop("MAKEPAD_RUN_STD", None)
        base.update(env or {})
        self.proc = subprocess.Popen(JIT + ["run", package, *args], cwd=cwd or TREE, env=base, stdin=subprocess.DEVNULL, stdout=self.file, stderr=self.file, start_new_session=True)
        self.pid = None

    def text(self):
        with open(self.log_path, errors="replace") as f:
            return f.read()

    def alive(self):
        return self.proc.poll() is None

    def ask(self, *cmd, timeout=60):
        """One inspect command in the app: (exit code, answer)."""
        try:
            r = subprocess.run([CLIENT, "inspect", str(self.pid), *cmd, "as=json"], cwd=ROOT, capture_output=True, text=True, timeout=timeout)
            return r.returncode, r.stdout if r.returncode == 0 else r.stdout + r.stderr
        except subprocess.TimeoutExpired:
            return 124, "no answer in %d s" % timeout

    def first_frames(self):
        return self.text().count("first frame at")

    def wait_frame(self, seen, secs):
        """A first frame past the `seen` count: None, or why not."""
        t = time.time()
        while time.time() - t < secs:
            text = self.text()
            if self.pid is None:
                m = re.search(r"makepad-remote: pid (\d+)", text)
                if m:
                    self.pid = int(m.group(1))
            if text.count("first frame at") > seen and self.pid:
                return None
            if not self.alive():
                return "exited (%s) before its first frame" % self.proc.returncode
            time.sleep(0.25)
        return "no first frame in %d s" % secs

    def stop(self):
        """Ends the app: asked first, by its recorded process group when it does not go. What a
        quit that hangs was doing is the answer (empty when it went)."""
        stuck = ""
        if self.alive() and self.pid:
            code, said = self.ask("gq", timeout=30)
            t = time.time()
            while self.alive() and time.time() - t < 15:
                time.sleep(0.2)
            if self.alive():
                stuck = "gq: %s; why-hung: %s" % (said.strip()[:160], self.ask("why-hung", timeout=30)[1].strip()[:500])
        if self.alive():
            os.killpg(self.proc.pid, 9)
        self.proc.wait()
        self.file.close()
        return stuck


BAD = re.compile(r"panicked|panic parked|stale access parked|internal compiler error|FATAL|fatal runtime|SIGSEGV|segfault|Bus error|the watcher stopped")
COMPILE = re.compile(r"^\s*(error(\[E\d+\])?:|.*compile error)", re.M)


def bad_lines(text, since=0):
    return [l.strip()[:300] for l in text[since:].splitlines() if BAD.search(l)][:8]


# a HAL line about drawing (the GPU backends, the frame path); the media, XR-stream, input and
# window-manager services log their own news with the same prefix
DRAW = re.compile(r"^\[hal-(?!video|media|camera|xr-remote|gamepad|dialog|wm\]|android-permission|android-dialog)")


def healthy(app, since):
    """None when the app runs, draws and answers input with nothing wrong in its log since `since`; else why."""
    if not app.alive():
        return "crash", "the process exited (%s)" % app.proc.returncode
    bad = bad_lines(app.text(), since)
    if bad:
        return ("ice" if any("internal compiler error" in b for b in bad) else "panic"), " | ".join(bad)
    # a frame that loses draws is an error UI, whatever a grab still shows
    draws = [l.strip()[:200] for l in app.text()[since:].splitlines() if DRAW.match(l)]
    if draws:
        # a burst during a handover or a window that stays broken: new frames tell (a hidden idle
        # window draws none by itself)
        for _ in range(3):
            app.ask("mouse", "k=move", "x=240", "y=240", "wait=1")
            app.ask("grab", "scale=0.25", "out=" + os.path.join(WORK, "grab.png"))
        text = app.text()[since:]
        later = sum(bool(DRAW.match(l)) for l in text.splitlines()) - len(draws)
        done_at = text.find("to first frame")
        first_at = text.find(draws[0])
        when = "no relaunch line" if done_at < 0 else "before the relaunch's first frame" if first_at < done_at else "after the relaunch's first frame"
        return "draw_error", "%d GPU errors, %d more over 3 forced frames, the first %s: %s" % (len(draws), later, when, draws[0])
    code, view = app.ask("session", "view")
    if code == 0 and '"over_app":true' in view.replace(" ", ""):
        return "error_view", view.strip()[:400]
    code, panics = app.ask("panics")
    if code == 0 and '"id"' in panics:
        return "parked", panics.strip()[:400]
    code, ui = app.ask("ui")
    if code != 0 or len(ui.strip()) < 40:
        return "dead_ui", "ui: %s" % ui.strip()[:300]
    # input is answered after the next frame: the UI thread is free (a move: a click would
    # change what the app shows, which the layout checks compare)
    code, moved = app.ask("mouse", "k=move", "x=200", "y=200", "wait=1")
    if code != 0:
        return "dead_ui", "input: %s" % moved.strip()[:300]
    # and it draws: a hidden window presents on a grab (the real GPU backend)
    png = os.path.join(WORK, "grab.png")
    if os.path.exists(png):
        os.remove(png)
    code, said = app.ask("grab", "scale=0.25", "out=" + png)
    if code != 0 or not drawn(png):
        return "dead_ui", "no picture: %s" % said.strip()[:300]
    return None


def drawn(path):
    """Whether a PNG holds a picture, not one flat colour."""
    try:
        data = open(path, "rb").read()
    except OSError:
        return False
    pos, idat = 8, b""
    while pos < len(data):
        n = int.from_bytes(data[pos : pos + 4], "big")
        if data[pos + 4 : pos + 8] == b"IDAT":
            idat += data[pos + 8 : pos + 8 + n]
        pos += 12 + n
    return len(set(zlib.decompress(idat))) > 8 if idat else False


RESIZES = [(1000, 680), (1500, 940), (1280, 800)]
LAYOUTS = []


STILL = 40  # looks at the tree, half a second apart, for it to hold still (then judged as it is)


def ui_lines(app):
    """The semantic tree with rects, ids, focus and measured times dropped (they differ between runs)."""
    code, ui = app.ask("ui")
    lines = []
    for l in ui.splitlines():
        l = re.sub(r"#[0-9a-f.]+ ", "", l.rstrip())
        # (times a run measures differ from run to run)
        l = re.sub(r"\b\d+(\.\d+)? ?(ms|s)\b", "N ms", l)
        l = re.sub(r"\b(focused|hovered|pressed)\b,?", "", l)
        lines.append(l)
    return lines


def layout(app):
    """The app's layout at each of RESIZES: per size its semantic tree with rects (ids and focus
    dropped: they differ between runs) and a picture. What a patched app and a fresh start of the
    same code must agree on."""
    out = []
    for n, (w, h) in enumerate(RESIZES):
        app.ask("resize", "w=%d" % w, "h=%d" % h, "wait=1")
        settle(app)
        app.ask("mouse", "k=move", "x=5", "y=5", "wait=1")
        # the tree once it holds still: an app still loading its document (a title, a progress
        # row) lays out what it has so far, which is the moment, not the code
        lines, last = ui_lines(app), None
        for _ in range(STILL):
            if lines == last:
                break
            time.sleep(0.5)
            last, lines = lines, ui_lines(app)
        png = os.path.join(WORK, "layout-%d-%d.png" % (len(LAYOUTS), n))
        app.ask("grab", "out=" + png)
        # the drawable's size: a window whose app keeps another size than its surface draws
        # stretched and takes input offset, whatever the tree says
        try:
            head = open(png, "rb").read(24)
            lines.append("grab-surface 0,0 %dx%d" % (int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")))
        except OSError:
            lines.append("grab-surface 0,0 0x0")
        # the app's windows as it sees them (index, size, scale): a window an earlier run's state
        # was given to, or a scale another window set, shows here
        code, st = app.ask("status")
        for m in re.finditer(r'\{"i":(\d+),"t":"(?:[^"\\]|\\.)*","sz":\[([-\d.]+),([-\d.]+)\],"px":\[[-\d.]+,[-\d.]+\],"dpi":([-\d.]+)', st):
            lines.append("window-%s dpi %s 0,0 %dx%d" % (m.group(1), m.group(4), round(float(m.group(2))), round(float(m.group(3)))))
        with open(png[:-4] + ".txt", "w") as f:
            f.write("\n".join(lines) + "\n")
        out.append(((w, h), lines, png))
    LAYOUTS.append(out)
    return out


def rects(lines):
    """name -> rect of the tree's nodes (the first of a name)."""
    got = {}
    for l in lines:
        m = re.match(r"\s*(.*?)\s(-?\d+),(-?\d+) (\d+)x(\d+)", l)
        if m and m.group(1).strip() not in got:
            got[m.group(1).strip()] = tuple(int(x) for x in m.groups()[1:])
    return got


def differing(a, b):
    """Per size, the names of the nodes that moved or are in one layout only."""
    out = []
    for (_, la, _), (_, lb, _) in zip(a, b):
        ra, rb = rects(la), rects(lb)
        out.append({k for k in ra if k in rb and ra[k] != rb[k]} | (set(ra) ^ set(rb)))
    return out


def layout_diff(a, b, noise=None, than="fresh"):
    """Where a layout differs from another of the same code: the first few, or None. `noise`
    (per size, from `differing`) are the nodes a relaunch with no edit moves too: the app's state
    a restart does not keep, not a stale layout."""
    diffs = []
    for n, ((size, la, _), (_, lb, _)) in enumerate(zip(a, b)):
        ra, rb = rects(la), rects(lb)
        skip = noise[n] if noise else set()
        moved = [(k, ra[k], rb[k]) for k in ra if k in rb and ra[k] != rb[k] and k not in skip]
        only_a, only_b = sorted(set(ra) - set(rb) - skip), sorted(set(rb) - set(ra) - skip)
        if moved or only_a or only_b:
            diffs.append("%dx%d: %d of %d moved, %d only in one (%d state-only skipped)%s%s%s" % (size[0], size[1], len(moved), len(ra), len(only_a) + len(only_b), len(skip),
                "".join("; %s %s -> %s %s" % (m[0], m[1], than, m[2]) for m in moved[:3]),
                "; only here: " + ", ".join(only_a[:6]) if only_a else "", "; only in the %s: " % than + ", ".join(only_b[:6]) if only_b else ""))
    return " | ".join(diffs) or None


def settle(app):
    """The JIT idle (bounded: a busy app is judged as it is)."""
    t = time.time()
    while time.time() - t < SETTLE and app.alive():
        code, said = app.ask("jit", "ms=100")
        if code == 0 and "idle" in said:
            return
        time.sleep(0.5)


def tree_drift():
    """Files of the second tree that differ from what its indexes say it holds (a tree something
    else changed: then a rustc rejection says nothing about the landing): '' when none."""
    out = []
    for index, work, look in [(INDEX, TREE, git), (INDEX + ".commercial", os.path.join(TREE, COMMERCIAL), cgit)]:
        if os.path.exists(index):
            said = look("diff", "--name-only", env=dict(os.environ, GIT_INDEX_FILE=index, GIT_WORK_TREE=work), ok=False).splitlines()
            out += [os.path.relpath(os.path.join(work, f), TREE) for f in said]
    return "%d file(s) differ from the tree's index: %s" % (len(out), ", ".join(out[:5])) if out else ""


def rustc_accepts(package):
    """Whether rustc accepts the second tree as it stands (a JIT rejection is then a JIT bug)."""
    r = subprocess.run(["cargo", "check", "--release", "--offline", "-q", "-p", package], cwd=TREE, capture_output=True, text=True, env=dict(os.environ, CARGO_TARGET_DIR=os.path.join(WORK, "rustc-target")))
    drift = tree_drift() if r.returncode != 0 else ""
    return r.returncode == 0, ("(%s) " % drift if drift else "") + "\n".join(r.stderr.splitlines()[:6])


def verdict(app, seen, since):
    """The JIT's verdict on an edit: its event, or {} when the watcher never took the edit up (no
    line from it in UNSEEN s: nothing the app loaded changed)."""
    t = time.time()
    answer = ""
    while time.time() - t < VERDICT:
        code, answer = app.ask("watch", "on=patchverified,patchrefused,patchreverted", "since=%d" % seen, "timeout_s=10", timeout=60)
        # (a watch that timed out answers the event `none`)
        line = next((l for l in answer.splitlines() if '"event"' in l and '"event":"none"' not in l), "")
        if line:
            return json.loads(line)
        if not app.alive():
            return {}
        if time.time() - t > UNSEEN and "makepad live:" not in app.text()[since:]:
            return {"answer": answer.strip()[:200]} if code != 0 else {}
    return {"answer": "no verdict in %d s: %s" % (VERDICT, answer.strip()[:160])}


def rung_of(event):
    return {"patchverified": "applied", "patchreverted": "reverted"}.get(event.get("event", ""), event.get("outcome", "none"))


def control(package):
    """Nothing here counts unless an edit the app did load gets a verdict: a shard whose edits
    cannot arrive would pass everything."""
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--offline", "--no-deps"], cwd=ROOT, capture_output=True, text=True, check=True).stdout)
    pkg = next(p for p in meta["packages"] if p["name"] == package)
    main = next(t["src_path"] for t in pkg["targets"] if "bin" in t["kind"])
    move_tree((git("rev-parse", "HEAD^{tree}"), cgit("rev-parse", "HEAD")))
    path = os.path.join(TREE, os.path.relpath(main, ROOT))
    app = App(package, os.path.join(WORK, "control.log"))
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_replay: the control app: %s" % why)
        settle(app)
        since = len(app.text())
        code, ev = app.ask("events")
        seen = max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
        with open(path, "a") as f:
            f.write("\nfn live_replay_control() -> u64 { 1 }\n")
        event = verdict(app, seen, since)
        rung = rung_of(event)
        print("live_replay: control edit: %s" % rung, flush=True)
        if rung == "none":
            print("verdict %s; asked since=%d; the app's events: %s" % (event, seen, app.ask("events", "since=0")[1][-1500:]), flush=True)
            print(app.text()[-1500:], flush=True)
            raise SystemExit("live_replay: the control edit got no verdict: this run's edits cannot arrive")
        # this tree itself starts clean (the whole log: an ICE at a fresh start is the worst kind);
        # and the checks can fail: the run's view over the app must be seen as one
        found = healthy(app, 0)
        if found:
            raise SystemExit("live_replay: the control app is not healthy: %s: %s" % found)
        app.ask("session", "view", "open")
        t = time.time()
        while time.time() - t < 30 and '"over_app":true' not in app.ask("session", "view")[1].replace(" ", ""):
            time.sleep(0.5)
        found = healthy(app, since)
        if not found or found[0] != "error_view":
            raise SystemExit("live_replay: the health check does not see the run's view over the app: %s" % (found,))
        print("live_replay: control: the health check sees a view over the app", flush=True)
    finally:
        app.stop()


def done(rec, **fields):
    rec.update(fields)
    return rec


def relaunch(app, package, what):
    """The restart button and the app back healthy: None, or a (result, kind, why) to end on."""
    frames = app.first_frames()
    since = len(app.text())
    code, said = app.ask("session", "relaunch")
    if code != 0:
        return "fail", "restart", "%s: session relaunch: %s" % (what, said.strip()[:300])
    why = app.wait_frame(frames, BOOT)
    if why:
        bad = bad_lines(app.text(), since)
        if bad:
            return "fail", "ice" if "internal compiler error" in bad[0] else "panic", "%s: %s" % (what, " | ".join(bad))
        ok, said = rustc_accepts(package)
        if not ok:
            return "skip", "tree", "rustc rejects the tree here: " + said[:300]
        return "fail", "restart", "%s: %s" % (what, why)
    settle(app)
    found = healthy(app, since)
    if found:
        return "fail", found[0], "after %s: %s" % (what, found[1])
    return None


def replay(sha, parent_tree, tree, package, seq):
    """One landing. The verdict record: result pass|fail|skip, rung, kind, why, seconds per step."""
    rec = {"sha": sha, "app": package, "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}}
    t0 = time.time()
    move_tree(parent_tree)
    log = os.path.join(WORK, "app-%d.log" % seq)
    app = App(package, log)
    try:
        why = app.wait_frame(0, COLD)
        rec["s"]["boot"] = round(time.time() - t0, 1)
        if why or COMPILE.search(app.text()):
            bad = bad_lines(app.text())
            ok, said = rustc_accepts(package)
            if not ok:
                # (what the JIT said about a tree rustc rejects too is recorded, not judged)
                return done(rec, result="skip", kind="tree", why="rustc rejects the parent here: %s; the JIT: %s" % (said[:300], " | ".join(bad)[:300]))
            if bad:
                return done(rec, result="fail", kind="ice" if "internal compiler error" in bad[0] else "panic", why="at the parent: " + " | ".join(bad))
            tail = [l for l in app.text().splitlines() if COMPILE.search(l)][:6]
            return done(rec, result="fail", kind="agreement", why="at the parent, rustc accepts: %s; %s" % (why, " | ".join(tail)))
        settle(app)
        found = healthy(app, 0)
        if found:
            ok, said = rustc_accepts(package)
            if not ok:
                return done(rec, result="skip", kind="tree", why="rustc rejects the parent here: %s; the JIT: %s: %s" % (said[:300], found[0], found[1][:300]))
            return done(rec, result="fail", kind=found[0], why="at the parent: " + found[1])
        noise = None
        if RESIZE:
            # the baseline: this fresh start, relaunched with no edit. What moves between them is
            # the app's own state (what a restart keeps or loses), not code: the layout checks
            # below skip those nodes
            fresh = layout(app)
            t1 = time.time()
            failed = relaunch(app, package, "the relaunch with no edit")
            rec["s"]["baseline"] = round(time.time() - t1, 1)
            if failed:
                return done(rec, result=failed[0], kind=failed[1], why="at the parent: " + failed[2])
            relaunched = layout(app)
            noise = differing(fresh, relaunched)
            rec["state_nodes"] = max(len(n) for n in noise)
            rec["state"] = layout_diff(fresh, relaunched, None, "relaunched") or ""
        # the landing arrives, as a sync moves the tree
        since = len(app.text())
        code, ev = app.ask("events")
        seen = max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
        move_tree(tree)
        t1 = time.time()
        event = verdict(app, seen, since)
        rec["s"]["verdict"] = round(time.time() - t1, 1)
        rec["rung"] = rung_of(event)
        rec["reasons"] = str(event.get("reasons", event.get("error", event.get("answer", ""))))[:400]
        settle(app)
        found = healthy(app, since)
        if found:
            return done(rec, result="fail", kind=found[0], why="after the edit (%s): %s" % (rec["rung"], found[1]))
        patched = layout(app) if rec["rung"] == "applied" and RESIZE else None
        # the restart button, on the parent's warm cache
        t2 = time.time()
        failed = relaunch(app, package, "the restart")
        rec["s"]["restart"] = round(time.time() - t2, 1)
        if failed:
            return done(rec, result=failed[0], kind=failed[1], why=failed[2])
        if RESIZE:
            restarted = layout(app)
            if patched:
                # the restart runs the same code from its start: a patched app lays out as it does
                differs = layout_diff(patched, restarted, noise, "restarted")
                if differs:
                    return done(rec, result="fail", kind="stale_layout", why="the patched app lays out unlike a restart of the same code: " + differs)
            # and the restart (the update the person clicks) lays out as a new process of the
            # same code: nothing of the previous run outlives a relaunch in place
            rec["quit"] = app.stop()
            t3 = time.time()
            app = App(package, os.path.join(WORK, "app-%d-fresh.log" % seq))
            why = app.wait_frame(0, COLD)
            rec["s"]["fresh"] = round(time.time() - t3, 1)
            if why:
                return done(rec, result="fail", kind="restart", why="a new process at the landing: %s: %s" % (why, " | ".join(bad_lines(app.text()))))
            settle(app)
            found = healthy(app, 0)
            if found:
                return done(rec, result="fail", kind=found[0], why="a new process at the landing: " + found[1])
            differs = layout_diff(restarted, layout(app), noise, "new process")
            if differs:
                return done(rec, result="fail", kind="stale_restart", why="the restarted app lays out unlike a new process of the same code: " + differs)
        if rec["rung"] == "none":
            # the watcher took it up and never answered is another thing than an edit it never saw
            rec["result"] = "silent" if event else "unreached"
        return rec
    finally:
        quit = app.stop()
        rec["quit"] = rec.get("quit") or quit
        rec["s"]["total"] = round(time.time() - t0, 1)


def wm_session():
    """A hidden WM of this tree, with a runtime dir of its own: (its app, its socket)."""
    run = os.path.join(WORK, "wm-run")
    os.makedirs(run, exist_ok=True)
    wm = App("makepad-app-wm", os.path.join(WORK, "wm.log"), cwd=ROOT, env={"XDG_RUNTIME_DIR": run}, args=["--", "--remote"])
    why = wm.wait_frame(0, COLD)
    if why:
        raise SystemExit("live_replay: the WM: %s\n%s" % (why, wm.text()[-1500:]))
    t = time.time()
    while time.time() - t < 60:
        found = [f for f in os.listdir(run) if f.startswith("makepad-wm-")]
        if found:
            return wm, os.path.join(run, found[0])
        time.sleep(0.5)
    raise SystemExit("live_replay: the WM published no socket in %s" % run)


# (what changes, the file; a comment: the crate's source identity changes, so the loader must)
HOST_EDITS = [("HAL", "platform/hal/src/lib.rs"), ("script", "platform/script/src/lib.rs"), ("makepad run", "tools/run/src/lib.rs"), ("compiler", "libs/relax/compiler/src/lib.rs")]


def host_case(app, what, path, n, rec):
    """One landing into what makepad is built from: offered after its rebuild, the click, back."""
    since = len(app.text())
    t0 = time.time()
    with open(os.path.join(ROOT, path), "a") as f:
        f.write("\n// live_replay host edit %d\n" % n)
    rec["what"] = what
    while True:
        said = app.text()[since:]
        if "a restart applies it" in said:
            break
        if "FAILED" in said or "does not start" in said or "cannot rebuild" in said:
            return done(rec, result="fail", kind="rebuild", why=" | ".join(l[:240] for l in said.splitlines() if "makepad live" in l)[-700:])
        if not app.alive():
            return done(rec, result="fail", kind="crash", why="exited (%s) while makepad rebuilt: %s" % (app.proc.returncode, said[-400:]))
        if "is unchanged" in said:
            return done(rec, result="unreached", rung="unchanged", why="makepad did not change")
        if time.time() - t0 > REBUILD:
            return done(rec, result="silent", why="no restart offered in %d s: %s" % (REBUILD, said[-400:]))
        time.sleep(1)
    rec["s"]["rebuilt"] = round(time.time() - t0, 1)
    rec["rung"] = "restart"
    found = healthy(app, since)
    if found:
        return done(rec, result="fail", kind=found[0], why="while offered: " + found[1])
    pid, frames = app.pid, app.first_frames()
    since = len(app.text())
    t1 = time.time()
    code, said = app.ask("session", "relaunch")
    if code != 0:
        return done(rec, result="fail", kind="restart", why="session relaunch: " + said.strip()[:300])
    why = app.wait_frame(frames, BOOT)
    rec["s"]["restart"] = round(time.time() - t1, 1)
    if why:
        return done(rec, result="fail", kind="restart", why="the click: %s: %s" % (why, " | ".join(bad_lines(app.text(), since)) or app.text()[-400:]))
    if app.pid != pid:
        return done(rec, result="fail", kind="restart", why="the process changed: %s -> %s" % (pid, app.pid))
    settle(app)
    found = healthy(app, since)
    if found:
        return done(rec, result="fail", kind=found[0], why="after the click: " + found[1])
    kept = re.search(r"restart \(.*?\): kept (\d+) window frame", app.text()[since:])
    if not kept or int(kept.group(1)) < 1:
        return done(rec, result="fail", kind="state", why="no window frame handed over: %s" % (kept.group(0) if kept else "no `kept` line"))
    rec["state"] = kept.group(0)[:160]
    return rec


def host(package, wm):
    os.makedirs(WORK, exist_ok=True)
    env, session = {}, None
    if wm:
        session, socket = wm_session()
        env = {"MAKEPAD_WM_SOCKET": socket, "MAKEPAD_TEST_LANE": os.environ.get("MAKEPAD_TEST_LANE", "jitupdate")}
    app = App(package, os.path.join(WORK, "host.log"), cwd=ROOT, env=env)
    tally, recs = {}, []
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_replay: %s: %s\n%s" % (package, why, app.text()[-1500:]))
        settle(app)
        found = healthy(app, 0)
        if found:
            raise SystemExit("live_replay: %s is not healthy before the edits: %s: %s" % (package, *found))
        noise = None
        if RESIZE:
            # the baseline: what a relaunch with no edit moves is the app's state (see replay)
            fresh = layout(app)
            failed = relaunch(app, package, "the relaunch with no edit")
            if failed:
                raise SystemExit("live_replay: %s: %s" % (package, failed[2]))
            noise = differing(fresh, layout(app))
        for n, (what, path) in enumerate(HOST_EDITS):
            rec = host_case(app, what, path, n, {"app": package, "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}})
            recs.append(rec)
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            print("%-12s %-9s %-9s %-10s %s %s %s" % (what, rec["result"], rec["rung"], rec["kind"], rec["s"], rec["why"], rec.get("state", "")), flush=True)
            if rec["result"] == "fail" and not app.alive():
                break
        if RESIZE and app.alive() and not tally.get("fail"):
            # each restart into a rebuilt makepad replaced the process (exec, its state carried
            # over): after them the app lays out as a new process of the same code
            restarted = layout(app)
            app.stop()
            app = App(package, os.path.join(WORK, "host-fresh.log"), cwd=ROOT, env=env)
            why = app.wait_frame(0, COLD)
            rec = {"app": package, "what": "new process", "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}}
            differs = why or layout_diff(restarted, layout(app), noise, "new process")
            if differs:
                rec.update(result="fail", kind="stale_restart", why="after %d restarts into a rebuilt makepad the app lays out unlike a new process: %s" % (len(recs), differs))
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            print("%-12s %-9s %-9s %-10s %s" % ("new process", rec["result"], rec["rung"], rec["kind"], rec["why"]), flush=True)
    finally:
        said = [l[:240] for l in app.text().splitlines() if re.search(r"makepad live|relaunch|panic", l)]
        app.stop()
        if session:
            print("the WM's log: " + " | ".join(session.text().splitlines()[-4:])[:500], flush=True)
            session.stop()
    if tally.get("fail") or tally.get("silent"):
        print("\n".join(said[-30:]), flush=True)
    return rate(package + (" under the WM" if wm else "") + " rebuild", tally, {})


def chain(package, steps):
    """--chain: one app takes the landings one after another with no restart between them, as a
    person's running app takes a day of landings (the restarts they need wait). Each edit's
    verdict is judged: a refusal of a tree rustc accepts is a JIT bug (refused_valid: the edit was
    checked against what the running program lacks, edits that wait for a restart)."""
    move_tree(steps[0][1])
    app = App(package, os.path.join(WORK, "chain.log"))
    tally, rungs = {}, {}
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_replay: the chain's app: %s" % why)
        settle(app)
        found = healthy(app, 0)
        if found:
            raise SystemExit("live_replay: the chain's app is not healthy at its start: %s: %s" % found)
        for sha, _, state in steps:
            rec = {"sha": sha, "app": package, "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}}
            since = len(app.text())
            code, ev = app.ask("events")
            seen = max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
            move_tree(state)
            t = time.time()
            event = verdict(app, seen, since)
            rec["s"]["verdict"] = round(time.time() - t, 1)
            rec["rung"] = rung_of(event)
            rec["reasons"] = str(event.get("reasons", event.get("error", event.get("answer", ""))))[:400]
            settle(app)
            found = healthy(app, since)
            if found:
                done(rec, result="fail", kind=found[0], why=found[1])
            elif rec["rung"] == "refuse":
                ok, said = rustc_accepts(package)
                if ok:
                    done(rec, result="fail", kind="refused_valid", why="rustc accepts the tree the JIT refused: " + rec["reasons"])
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            rungs[rec["rung"]] = rungs.get(rec["rung"], 0) + 1
            print("%s %-9s %-9s %-13s %s %s %s" % (sha[:10], rec["result"], rec["rung"], rec["kind"], rec["s"], rec["why"][:600], rec["reasons"] if rec["result"] != "fail" else ""), flush=True)
            if not app.alive():
                break
    finally:
        app.stop()
    return rate(package + " chain", tally, rungs)


def rate(name, tally, rungs):
    """The run's one number."""
    judged = tally.get("pass", 0) + tally.get("fail", 0) + tally.get("silent", 0)
    pct = "%.1f%%" % (100.0 * tally.get("pass", 0) / judged) if judged else "n/a"
    print("live_replay: %s pass rate %s (%d/%d judged; %d fail, %d silent, %d skip, %d unreached; rungs %s)" % (name, pct, tally.get("pass", 0), judged, tally.get("fail", 0), tally.get("silent", 0), tally.get("skip", 0), tally.get("unreached", 0), rungs), flush=True)
    if not judged:
        return 1
    return 3 if tally.get("fail") or tally.get("silent") else 0


def main():
    os.makedirs(WORK, exist_ok=True)
    package = opt("--app", "makepad-app-storybook")
    subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad"], cwd=ROOT, check=True)
    shutil.copy2(os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "host", "makepad"), CLIENT)
    if "--host" in sys.argv:
        sys.exit(host(package, "--wm" in sys.argv))
    count = int(opt("--count", "300"))
    tip = git("rev-parse", "HEAD")
    history = [x for x in opt("--history", "").split(",") if x]
    base, side_tip = history if history else (tip, cgit("rev-parse", "HEAD"))
    only = [x for x in opt("--only", "").split(",") if x]
    meta = metadata()
    kept = kept_dirs(meta)
    if only:
        # (a commit of either repository)
        items = []
        for x in only:
            commercial = subprocess.run(["git", "cat-file", "-e", x + "^{commit}"], cwd=ROOT, capture_output=True).returncode != 0
            look = cgit if commercial else git
            items.append((int(look("log", "-1", "--format=%ct", x)), "commercial" if commercial else "makepad", look("rev-parse", x)))
    else:
        items = landings(meta, package, kept, base, side_tip, count)[::-1]
    print("live_replay: %s, %d landings, kept at this tree: %s" % (package, len(items), " ".join(kept)), flush=True)
    results = os.path.join(WORK, "results-%s.jsonl" % package)
    drifted = 0
    tally = {}
    rungs = {}
    trees = {}

    def tree_at(commit):
        if commit not in trees:
            trees[commit] = hybrid(commit, tip, kept)
        return trees[commit]

    def makepad_at(t):
        return git("log", "--first-parent", "-1", "--format=%H", "--before=%d" % t, base)

    def commercial_at(t):
        return cgit("log", "--first-parent", "-1", "--format=%H", "--before=%d" % t, side_tip)

    if "--chain" in sys.argv:
        steps = []
        for t, repo, sha in sorted(items):
            if repo == "makepad":
                side = commercial_at(t)
                steps.append((sha, (tree_at(sha + "^1"), side), (tree_at(sha), side)))
            else:
                ours = tree_at(makepad_at(t))
                steps.append((sha, (ours, cgit("rev-parse", sha + "^1")), (ours, sha)))
        os.makedirs(TREE, exist_ok=True)
        shutil.copy2(os.path.join(ROOT, "Cargo.lock"), os.path.join(TREE, "Cargo.lock"))
        sys.exit(chain(package, steps))
    if items:
        control(package)
    # (the lockfile is not committed: the second tree resolves as this one does)
    os.makedirs(TREE, exist_ok=True)
    shutil.copy2(os.path.join(ROOT, "Cargo.lock"), os.path.join(TREE, "Cargo.lock"))
    with open(results, "w") as out:
        for seq, (t, repo, sha) in enumerate(items):
            if repo == "makepad":
                side = commercial_at(t)
                a, b = (tree_at(sha + "^1"), side), (tree_at(sha), side)
            else:
                ours = tree_at(makepad_at(t))
                a, b = (ours, cgit("rev-parse", sha + "^1")), (ours, sha)
            if a == b:
                rec = {"sha": sha, "app": package, "result": "skip", "rung": "-", "kind": "kept", "why": "only the JIT's and the providers' files changed", "s": {}}
            else:
                rec = replay(sha, a, b, package, seq)
                if rec["result"] in ("fail", "silent"):
                    with open(os.path.join(WORK, "app-%d.log" % seq), errors="replace") as f:
                        # what the JIT said (a draw error repeated each frame buries it otherwise)
                        lines = f.read().splitlines()
                        said = [l[:300] for l in lines if re.search(r"makepad live|makepad-run|relaunch|error|panic", l)]
                        rec["log"] = "\n".join(said[-40:]) + "\n(%d lines, %d draw errors)" % (len(lines), sum(bool(DRAW.match(l)) for l in lines))
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            rungs[rec["rung"]] = rungs.get(rec["rung"], 0) + 1
            drifted = drifted + 1 if rec["kind"] == "tree" else 0
            out.write(json.dumps(rec) + "\n")
            out.flush()
            if rec.get("quit"):
                print("%s quit hangs: %s" % (sha[:10], rec["quit"]), flush=True)
            print("%s %-9s %-9s %-10s %s %s %s" % (sha[:10], rec["result"], rec["rung"], rec["kind"], rec["s"], rec["why"], rec.get("reasons", "") if rec["rung"] not in ("applied", "none") or rec["result"] == "silent" else ""), flush=True)
            if rec.get("state"):
                print("  the relaunch with no edit moved (skipped as state): %s" % rec["state"][:1500], flush=True)
            if rec.get("log"):
                print(rec["log"], flush=True)
            if drifted >= DRIFTED:
                print("live_replay: the tree no longer builds against this JIT from %s back: %d of %d landings not run" % (sha[:10], len(items) - seq - 1, len(items)), flush=True)
                break
    sys.exit(rate(package, tally, rungs))


if __name__ == "__main__":
    main()
