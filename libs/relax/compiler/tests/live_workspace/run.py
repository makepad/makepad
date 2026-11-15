#!/usr/bin/env python3
"""A live test session in a workspace of several crates applies each save (relax issue 33):
run.py [/path/to/relax] (without one, from the workspace root: relax-runner's relax, built by cargo).

`relax test --live lw-app --test t` from the root of a copy of ws/ (its Cargo.toml says what is
in it). The copy is a git checkout (its .git written here, no git run) that a crashed git operation left
an index.lock in: the watcher read that as an operation under way and held every save for a
quiet window that never came, silently. Each save is written in place (truncate and write) and
brings a verdict:
1. the first one: both tests pass;
2. a body edit of crates/core, a member the test's crate uses: `adds` fails;
3. a body edit of crates/app's lib: `adds` passes again;
4. an edit of the test file: `adds` fails.
And the session reports no E0119: libs/state's impls, whose array lengths are a type-level
number's associated constant, do not overlap (the session that ignored its saves printed such
E0119s for a crate of that pattern).
Prints "live_workspace: ok" or "live_workspace: FAILED (...)"."""
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess as sp
import sys
import tempfile
import time

if len(sys.argv) > 1:
    jit = [str(Path(sys.argv[1]).resolve())]
else:
    # built once, here (a `cargo run` in the copy, outside the workspace, builds with other flags)
    sp.run(["cargo", "build", "--profile", "host", "-q", "-p", "relax-runner", "--bin", "relax"], check=True)
    jit = [str((Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "host" / "relax").resolve())]
here = Path(__file__).resolve().parent
VERDICT = re.compile(r"^testverdict group=(\d+) ran=\d+ passed=(\d+) failed=(\d+)", re.M)


def fail(why, log=None):
    print("live_workspace: FAILED (%s)" % why)
    if log is not None and log.exists():
        print("\n".join(log.read_text(errors="replace").splitlines()[-30:]))
    sys.exit(1)


with tempfile.TemporaryDirectory(prefix="live-workspace-") as tmp:
    root = Path(tmp)
    ws = root / "ws"
    shutil.copytree(here / "ws", ws)
    git = ws / ".git"
    (git / "refs" / "heads").mkdir(parents=True)
    (git / "HEAD").write_text("ref: refs/heads/main\n")
    (git / "refs" / "heads" / "main").write_text("1" * 40 + "\n")
    # a crashed operation's lock, from before the session
    (git / "index.lock").write_text("")
    hour_ago = time.time() - 3600
    os.utime(git / "index.lock", (hour_ago, hour_ago))
    env = dict(os.environ, RELAX_CACHE_DIR=str(root / "cache"), MAKEPAD_REMOTE_REGISTRY=str(root / "registry"), RELAX_PANIC_PARK="0")
    log = root / "live.log"
    with open(log, "w") as f:
        p = sp.Popen(jit + ["test", "--live", "lw-app", "--test", "t"], cwd=ws, env=env, stdout=f, stderr=sp.STDOUT, start_new_session=True)
    try:
        def verdict(seen, step):
            """The session's next verdict after the `seen` first: (passed, failed)."""
            end = time.monotonic() + 60
            while time.monotonic() < end:
                found = VERDICT.findall(log.read_text(errors="replace"))
                if len(found) > seen:
                    return int(found[seen][1]), int(found[seen][2])
                if p.poll() is not None:
                    fail("%s: the session ended (exit %s)" % (step, p.returncode), log)
                time.sleep(0.1)
            fail("%s: no verdict in 60 s" % step, log)

        def save(path, old, new):
            # mtimes of a coarse file system: the save is seen as one
            time.sleep(1.1)
            text = path.read_text()
            if old not in text:
                fail("%s holds no %r" % (path, old))
            with open(path, "r+") as f:
                f.truncate(0)
                f.write(text.replace(old, new))

        steps = [
            ("the first verdict", None, 0),
            ("an edit of crates/core", (ws / "crates/core/src/lib.rs", "x * 10", "x * 20"), 1),
            ("an edit of crates/app", (ws / "crates/app/src/lib.rs", "lw_core::base(x) + 1", "lw_core::base(x) - 9"), 0),
            ("an edit of the test file", (ws / "crates/app/tests/t.rs", "f(1), 11", "f(1), 12"), 1),
        ]
        for seen, (step, edit, want) in enumerate(steps):
            if edit is not None:
                save(*edit)
            passed, failed = verdict(seen, step)
            # (an edit's verdict counts the tests it ran: those the edit reaches)
            if failed != want or (edit is None and passed != 2):
                fail("%s: passed %d, failed %d; want %d failed" % (step, passed, failed, want), log)
        if "E0119" in log.read_text(errors="replace"):
            fail("the coherence check reported overlapping impls", log)
    finally:
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            p.wait(timeout=10)
        except sp.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
print("live_workspace: ok")
