#!/usr/bin/env python3
"""Test reach through O3 body-word calls: run.py [/path/to/relax] (without one, from the workspace
root: relax-runner's relax, built by cargo).

An O3 call site with profile counts calls its callee's body word and skips the entry wrapper, so
the callee gets no mark of its own; the caller's note of the callee (at the O3 install, and at a
warm link from the disk cache) carries the reach. `makepad test --live live-reach --lib`, with a
low O3 threshold, found with `inspect list`:
0. `f` reaches O3 by the tier-up policy (its tests run again until it does: an inspect build is
   never stored, and step 4 needs the stored build), `f2` by `inspect tier`; `f`'s code calls `g`
   through the body word (the bypass is on, not silently off).
1. An edit of `h` to an equal value reaches `hot` and `hot_indirect` (passed, ran), not `other`.
   `f` runs its O3 code only, so `g` has no mark of its own.
2. (a) An edit of `g` fails `hot` and reruns it; `other` does not rerun. `direct` calls `g` through
   its wrapper, so the edit is reached by a test and the session selects by reach: without the
   O3 install's note `hot` keeps its stale pass.
3. (c) An edit of `g2`, called through a fn pointer, fails `hot_indirect` (the indirect call marks).
4. (b) A second session on the same disk cache links `f`'s O3 code from it (checked): an edit of
   `g` back to its first value passes `hot` and reruns it (only the warm link's note reaches `g`).
Prints "live_reach: ok" or "live_reach: FAILED (...)"."""
import itertools
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess as sp
import sys
import tempfile
import time

if len(sys.argv) > 1:
    jit = [str(Path(sys.argv[1]).resolve())]
else:
    sp.run(["cargo", "build", "--profile", "host", "-q", "-p", "relax-runner", "--bin", "relax"], check=True)
    jit = [str((Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "host" / "relax").resolve())]
here = Path(__file__).resolve().parent


def fail(why, log=None):
    print("live_reach: FAILED (%s)" % why)
    if log is not None and log.exists():
        print("\n".join(log.read_text(errors="replace").splitlines()[-30:]))
    sys.exit(1)


class Session:
    """One `relax test --live` session on the shared cache; `close()` ends it and waits for the exit."""

    def __init__(self, root, pkg, name):
        self.pkg = pkg
        self.log = root / (name + ".log")
        env = dict(os.environ, RELAX_CACHE_DIR=str(root / "cache"), MAKEPAD_REMOTE_REGISTRY=str(root / "registry"),
                   RELAX_PANIC_PARK="0", RELAX_O3_CALLS="50")
        self.env = env
        with open(self.log, "w") as f:
            self.p = sp.Popen(jit + ["test", "--live", "live-reach", "--lib"], cwd=pkg, env=env,
                              stdout=f, stderr=sp.STDOUT, start_new_session=True)
        self.pid = None
        while self.pid is None:
            if self.p.poll() is not None:
                fail("the session ended before it could be inspected", self.log)
            apps = [a for a in self.inspect("list") if a.get("app") == "live-reach"]
            if apps:
                self.pid = str(apps[0]["pid"])
            else:
                time.sleep(0.2)

    def inspect(self, *words, check=True, json_out=True):
        r = sp.run(jit + ["inspect", *words] + (["--json"] if json_out else []), cwd=self.pkg, env=self.env,
                   stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
        if not json_out:
            return r.stdout
        rows = [json.loads(line) for line in r.stdout.splitlines() if line.startswith("{")]
        if check and r.returncode != 0:
            fail("inspect %s: exit %d: %s" % (" ".join(words), r.returncode, r.stdout.strip()), self.log)
        return rows

    def verdict(self, since):
        for e in self.inspect(self.pid, "watch", "on=testverdict", "since=%d" % since):
            if e.get("event") == "testverdict":
                return e
        fail("no testverdict after seq %d" % since, self.log)

    def results(self, v, step):
        if v.get("results") != "test-results group=%d" % v["group"]:
            fail("%s: the verdict names no results query: %s" % (step, v), self.log)
        rows = self.inspect(self.pid, *v["results"].split())
        return {r["name"]: (r["status"], r["ran"]) for r in rows if "name" in r}

    def expect(self, v, step, want):
        got = self.results(v, step)
        if got != want:
            fail("%s: %s, want %s" % (step, got, want), self.log)

    def edit(self, old, new):
        lib = self.pkg / "src" / "lib.rs"
        text = lib.read_text()
        if text.count(old) != 1:
            fail("edit: %r found %d times in lib.rs" % (old, text.count(old)))
        before = lib.stat()
        lib.write_text(text.replace(old, new))
        after = lib.stat()
        # the session sees a save by its stamp (mtime, size): on a file system with coarse times
        # an equal-size save can keep it, so it is saved again once the time has moved
        if (after.st_mtime_ns, after.st_size) == (before.st_mtime_ns, before.st_size):
            time.sleep(1.1)
            lib.write_text(text.replace(old, new))

    def close(self):
        try:
            os.killpg(self.p.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            self.p.wait(timeout=10)
        except sp.TimeoutExpired:
            os.killpg(self.p.pid, signal.SIGKILL)
            self.p.wait()
            fail("the session did not exit on SIGTERM", self.log)


def o3_installed(s, name, origins=None):
    """`name` runs installed O3 code (of one of `origins`, when given: fresh, warm-cache, ...)."""
    for line in s.inspect(s.pid, "versions", name, json_out=False).splitlines():
        cells = [c.strip() for c in line.split("|")]
        if len(cells) > 4 and cells[1] == "O3" and cells[3] == "installed" and (origins is None or cells[2] in origins):
            return True
    return False


PASS = ("passed", True)
KEPT = ("passed", False)
sessions = []
with tempfile.TemporaryDirectory(prefix="live-reach-") as tmp:
    root = Path(tmp)
    pkg = root / "pkg"
    pkg.mkdir()
    shutil.copy(here / "Cargo.toml", pkg / "Cargo.toml")
    shutil.copytree(here / "src", pkg / "src")
    (root / "registry").mkdir()
    try:
        s = Session(root, pkg, "first")
        sessions.append(s)
        first = s.verdict(0)
        s.expect(first, "first verdict", {"hot": PASS, "hot_indirect": PASS, "direct": PASS, "other": PASS})
        # equal-value edits of `h` reach `hot` and `hot_indirect` (not `other`)
        last = first
        forms = itertools.cycle(["5 + 5", "6 + 4", "7 + 3", "8 + 2", "9 + 1", "4 + 6", "3 + 7", "2 + 8", "1 + 9", "0 + 10"])
        cur = ["10"]

        def edit_h(step):
            nxt = next(forms)
            s.edit("pub fn h() -> u32 { %s }" % cur[0], "pub fn h() -> u32 { %s }" % nxt)
            cur[0] = nxt
            v = s.verdict(last["seq"])
            s.expect(v, step, {"hot": PASS, "hot_indirect": PASS, "direct": KEPT, "other": KEPT})
            return v

        # f's O3 build by the tier-up policy, which stores it for step (b) (an inspect build is
        # never stored): the tests run again until f's counted code has been built at O3
        # (its counting window is wall time: a bounded wait, 60 s, in runs of its test)
        runs, deadline = 0, time.monotonic() + 60
        while not o3_installed(s, "f", ("fresh",)):
            if time.monotonic() > deadline:
                fail("f reached no O3 code in %d runs of its test (60 s): %s" % (runs, s.inspect(s.pid, "versions", "f", json_out=False)), s.log)
            last = edit_h("an h edit while f tiers up")
            runs += 1
        print("live_reach: f reached O3 by the policy after %d more runs of its test" % runs)
        # f2's O3 build, now and here (`tier <fn> o3 fresh=1` builds before it answers; a build the
        # tier-up policy beat to it is dropped as outdated: the state check below is the test)
        s.inspect(s.pid, "tier", "f2", "o3", "fresh=1")
        if not o3_installed(s, "f2"):
            fail("f2 has no installed O3 version: %s" % s.inspect(s.pid, "versions", "f2", json_out=False), s.log)
        # the O3 code of f calls g through its body word: a `call [r11]` (aarch64: ldr x16, [x16]; blr x16),
        # where a wrapper call is a `call rel32`
        asm = s.inspect(s.pid, "asm", "f", "view=listing", json_out=False)
        if "call qword ptr [r11]" not in asm and not ("ldr x16, [x16]" in asm and "blr x16" in asm):
            fail("f's O3 code has no body-word call to g (the bypass is off?): %s" % asm, s.log)
        # step 3: f runs its O3 code only, so g has no mark of its own
        last = edit_h("the h edit with f in O3")
        # (a) an edit of g, reached through f's body-word call only
        s.edit("wrapping_add(1)", "wrapping_add(2)")
        last = s.verdict(last["seq"])
        s.expect(last, "(a) the g edit", {"hot": ("failed", True), "hot_indirect": KEPT, "direct": ("failed", True), "other": KEPT})
        # (c) an edit of g2, called through a fn pointer
        s.edit("wrapping_add(7)", "wrapping_add(8)")
        last = s.verdict(last["seq"])
        s.expect(last, "(c) the g2 edit", {"hot": ("failed", False), "hot_indirect": ("failed", True), "direct": ("failed", False), "other": KEPT})
        s.close()
        sessions.clear()
        # (b) a second session on the same cache links f's O3 code from it
        s = Session(root, pkg, "second")
        sessions.append(s)
        warm = s.verdict(0)
        s.expect(warm, "the second session's first verdict", {"hot": ("failed", True), "hot_indirect": ("failed", True), "direct": ("failed", True), "other": PASS})
        if not o3_installed(s, "f", ("warm-cache", "warm-image")):
            fail("the second session runs no O3 code of f linked from the cache: %s" % s.inspect(s.pid, "versions", "f", json_out=False), s.log)
        s.edit("wrapping_add(2)", "wrapping_add(1)")
        v = s.verdict(warm["seq"])
        s.expect(v, "(b) the g edit back in the second session", {"hot": PASS, "hot_indirect": ("failed", False), "direct": PASS, "other": KEPT})
    finally:
        for s in sessions:
            s.close()
print("live_reach: ok")
