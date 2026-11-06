#!/usr/bin/env python3
"""A live edit that leaves the boot text leaves no crate edited, and the next start is warm.

usage: edit_undone_warm.py [--only none,refused,undone]
  (run from the workspace root; the JIT runs through `cargo run --profile host`; MAKEPAD_RUN_STD
   defaults to libs/relax/compiler/std)

Each case copies live_kinds/ to a fresh dir with its own MAKEPAD_RUN_HOME and runs
`makepad run --verbose --ahead=minimal relax-live-kinds` (no window: a program that never presents)
twice, hidden and muted. The first run boots cold; once its collected program is recorded it
gets the case's edits, then it ends (its exit write runs):
  none   : no edit (the baseline of the second run's cache use);
  refused: an edit with a type error is refused, and the text is put back;
  undone : an edit is patched, then undone (the boot text again).
What each check proves (red = which fails without which change):
  - "recorded while it ran" (every case): a program that never presents records its collected
    program soon after its startup goes quiet, as an app does after its first present, not only
    at its exit (red without the headless record: then it is recorded at the exit, which an edit
    before it stops, so the next start collects again);
  - "the mark clears" (refused, undone): the edit leaves no crate edited ("makepad live: not
    edited"), so the rest of the run records and checks as an unedited run does (red without the
    marks fix: a refused edit, or one that puts the boot text back, left its crate edited);
  - "code / borrowck / keys from the cache" (refused, undone): the second run takes the crate's
    code (cache misses), borrow-check verdicts (gandalf's fns checked) and startup keys (manifest
    slots without cached code) from the cache as the baseline's second run does. These pass
    without either change too: they hold the guards on an edit's leftover versions (no collected
    program captured, no verdict stored, from tables that hold them) to costing the next start
    nothing;
  - "restored": the second run restores the collected program the first run recorded before the
    edit: the edit and the exit's writes leave it in place. Checked only when the record landed
    before the edit (the first run waits up to EDIT_UNDONE_RECORD_WAIT s for it); a run whose edit
    came first records none (its tables hold the edit's versions).
Prints `edit_undone_warm: ok` or `edit_undone_warm: FAIL ...`; exit 1 on failure.
EDIT_UNDONE_TRACE=1 also prints each run's cache, collection and live lines.
"""
import json, os, re, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.join(HERE, "live_kinds")
ROOT = os.path.abspath(os.path.join(HERE, "../../.."))
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.environ.setdefault("MAKEPAD_RUN_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
WAIT = float(os.environ.get("EDIT_UNDONE_WAIT", "300"))
RECORD_WAIT = float(os.environ.get("EDIT_UNDONE_RECORD_WAIT", "10"))
# (the record lands about 2 s after the start, the edits take about 3 s more)
RUN_SECS = 20
K = "src/kinds.rs"
BODY = ("pub fn body() -> u64 { 1 }", "pub fn body() -> u64 { 2 }")
BAD = ("pub fn mfile_val() -> u64 { 0 }", "pub fn mfile_val() -> u64 { (|x: u64| x)(0) + \"x\" }")
BROKE = ("panicked", "internal compiler error", "FATAL", "the watcher stopped")
RECORDED = "makepad-run collected: recorded "


def copy_pkg(dst):
    shutil.copytree(PKG, dst)
    # the copy is a workspace of its own: this checkout's widgets and [patch.crates-io] (live_kinds.py)
    manifest = os.path.join(dst, "Cargo.toml")
    text = open(manifest).read().replace('"../../../../widgets"', json.dumps(os.path.relpath(os.path.join(ROOT, "widgets"), dst)))
    patch, on = [], False
    for line in open(os.path.join(ROOT, "Cargo.toml")):
        if line.startswith("["):
            on = line.strip() == "[patch.crates-io]"
        elif on and 'path = "' in line:
            crate, rest = line.split("=", 1)
            path = rest.split('path = "', 1)[1].split('"', 1)[0]
            patch.append("%s = { path = %s }" % (crate.strip(), json.dumps(os.path.relpath(os.path.join(ROOT, path), dst))))
    open(manifest, "w").write(text + "\n[patch.crates-io]\n" + "\n".join(patch) + "\n")


def edit(pkg, old, new):
    p = os.path.join(pkg, K)
    s = open(p).read()
    assert s.count(old) == 1, "%r is in %s %d times" % (old, K, s.count(old))
    open(p, "w").write(s.replace(old, new))


class Run:
    """One run of the fixture: its stderr lines with their arrival times, and its probe lines."""
    def __init__(self, work, home, pkg, secs):
        self.err, self.out = [], []
        self.ready = os.path.join(work, "ready")
        if os.path.exists(self.ready):
            os.remove(self.ready)
        env = dict(os.environ, MAKEPAD_RUN_LIVE_READY=self.ready, MAKEPAD_RUN_HOME=home, MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"),
                   MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", SANDBOX_MUTE="1", MAKEPAD_MUTE="1",
                   MUSIC_MUTE="1", LIVE_KINDS_SECS=str(secs), MAKEPAD_RUN_LIVE_TRACE="1", MAKEPAD_RUN_BOOT_TRACE="1", MAKEPAD_REMOTE="1")
        self.p = subprocess.Popen(JIT + ["run", "--quiet", "--verbose", "--ahead=minimal", "relax-live-kinds"], cwd=pkg, env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace", start_new_session=True)
        threading.Thread(target=self.pump, args=(self.p.stdout, self.out), daemon=True).start()
        threading.Thread(target=self.pump, args=(self.p.stderr, self.err), daemon=True).start()

    @staticmethod
    def pump(f, acc):
        for line in f:
            acc.append((time.time(), line.rstrip("\n")))

    def lines(self, since=0):
        return [l for _, l in self.err[since:]]

    def has(self, s, since=0):
        return any(s in l for l in self.lines(since))

    def at(self, s):
        return next((t for t, l in self.err if s in l), None)

    def wait(self, pred, secs):
        t = time.time()
        while time.time() - t < secs:
            if pred():
                return True
            if self.p.poll() is not None:
                return pred()
            time.sleep(0.05)
        return False

    def up(self):
        ticks = lambda: sum(1 for _, l in self.out if l.startswith("out "))
        return self.wait(lambda: os.path.exists(self.ready) and ticks() >= 3 and self.has("watcher gate open"), WAIT)

    def live(self, pkg, old, new, verdict):
        """Saves an edit and waits for the JIT's verdict line on it ("NOT applied", ": patched (")."""
        n = len(self.err)
        edit(pkg, old, new)
        ok = self.wait(lambda: self.has(verdict, n), WAIT)
        time.sleep(1)
        return ok

    def end(self):
        try:
            self.p.wait(timeout=WAIT)
        except subprocess.TimeoutExpired:
            self.p.kill()
            self.p.wait()
            return "the run did not end"
        return next(("broke: " + l.strip()[:200] for l in self.lines() if any(b in l for b in BROKE)), None)

    def stats(self):
        """(cache misses, gandalf fns checked, manifest slots without cached code); None: not printed."""
        def num(pattern):
            m = next((re.search(pattern, l) for l in self.lines() if re.search(pattern, l)), None)
            return int(m.group(1)) if m else None
        return (num(r"makepad-run cache: \d+ hits, (\d+) misses"), num(r"gandalf: (\d+) fns checked"),
                num(r"startup manifest of \d+ slots \(\d+ before the first present, (\d+) without cached code\)"))


def show(tag, r):
    if os.environ.get("EDIT_UNDONE_TRACE"):
        for l in r.lines():
            if "makepad-run collected:" in l or "makepad-run cache: " in l and " hits, " in l or l.startswith("gandalf: ") \
                    or "startup manifest of" in l or ("makepad live:" in l and "watch round" not in l and "rewatch" not in l):
                print("  %s | %s" % (tag, l[:220]), flush=True)
        print("  %s | stats (misses, checked, uncached manifest slots): %s" % (tag, r.stats()), flush=True)


def case(root, name, base):
    """The case's failures, in the order they matter (empty: it passed), and its second run's stats."""
    work = os.path.join(root, name)
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(work)
    copy_pkg(pkg)
    r = Run(work, home, pkg, RUN_SECS)
    if not r.up():
        r.p.kill()
        return ["first run: not up: %s" % (r.lines()[-1][:160] if r.err else "")], None
    recorded_first = r.wait(lambda: r.has(RECORDED), RECORD_WAIT)
    steps = {"none": [], "refused": [(BAD, "NOT applied", "the type error was not refused"), ((BAD[1], BAD[0]), ": patched (", "putting the text back was not patched")],
             "undone": [(BODY, ": patched (", "the edit was not patched"), ((BODY[1], BODY[0]), ": patched (", "the undo was not patched")]}[name]
    for (old, new), verdict, why in steps:
        if not r.live(pkg, old, new, verdict):
            r.p.kill()
            return [why], None
    fails = []
    if name != "none" and not r.has("makepad live: not edited"):
        fails.append("the mark did not clear: the crate stayed edited")
    broke = r.end()
    # (the main loop's last tick: the record must come before it, not in the exit write)
    t_end = r.out[-1][0] if r.out else 0
    show(name + " 1", r)
    if broke:
        return fails + ["first run: " + broke], None
    t_rec = r.at(RECORDED)
    if t_rec is None or t_rec > t_end:
        fails.append("first run: the collected program was not recorded while it ran (%s)" % ("at its exit" if t_rec else "not at all"))
    r2 = Run(work, home, pkg, 4)
    r2.up()
    broke = r2.end()
    show(name + " 2", r2)
    if broke:
        fails.append("second run: " + broke)
    got = r2.stats()
    if base is not None:
        for what, b, g in zip(("code (cache misses)", "borrowck (fns checked)", "keys (manifest slots without cached code)"), base, got):
            if g is None or b is None:
                fails.append("second run: no %s count" % what)
            elif g > b:
                fails.append("second run: %s %d, the unedited baseline's %d" % (what, g, b))
    if recorded_first and not r2.has("makepad-run collected: restored "):
        why = [l for l in r2.lines() if "makepad-run collected: not restored" in l or "makepad-run collected: prefix" in l]
        fails.append("second run: the collected program was not restored (%s)" % (why[0].strip()[:160] if why else "no restore line"))
    if not recorded_first:
        print("%s: the record did not land before the edit in %.0f s: the restore is not checked" % (name, RECORD_WAIT), flush=True)
    return fails, got


args = sys.argv[1:]
only = [c for c in (args[args.index("--only") + 1] if "--only" in args else "").split(",") if c]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
root = tempfile.mkdtemp(prefix="edit-undone-", dir=os.path.join(ROOT, "target"))
fails = []
base = None
for name in ["none", "refused", "undone"]:
    if only and name not in only and name != "none":
        continue
    why, got = case(root, name, base)
    if name == "none":
        base = got
    print("%s: %s" % (name, "; ".join(why) or "ok"), flush=True)
    if why:
        fails.append("%s: %s" % (name, "; ".join(why)))
if fails:
    print("edit_undone_warm: FAIL %s (runs in %s)" % (" | ".join(fails), root))
    sys.exit(1)
shutil.rmtree(root)
print("edit_undone_warm: ok")
