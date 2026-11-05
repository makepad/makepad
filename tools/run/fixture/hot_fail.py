#!/usr/bin/env python3
"""A failed hot load ends in a warm restart and a ledger report, never a parked panic.

usage: hot_fail.py   (run from the workspace root; the JIT runs through `cargo run --profile host`;
  MAKEPAD_RUN_STD in the environment, default libs/relax/compiler/std)

Copies hot_fail/ to a temporary dir and runs it, once per way a build can fail after a live edit.
Once it ticks, tick.rs is edited so that a worker thread calls a fn never built before. In a process
an edit ran in:
  - error: its build fails with an internal compiler error (MAKEPAD_RUN_TEST_HOT_BUILD_ERROR): the
    call would park at a compile error;
  - panic: the compiler panics in the call (MAKEPAD_RUN_TEST_HOT_COMPILE_PANIC): the call gets a stub
    that panics. Its builds fail too, so that nothing builds it ahead of the call.
Each passes when:
  - nothing parks and no panic waits for anyone;
  - the app's state came back ("out restored <tick>" at or past the tick before the edit) and the
    edit runs (value=2);
  - the ledger (the hot outbox under MAKEPAD_HOME) holds the failure and its restart, one load;
  - the restarted run answers `inspect` at its pid, the first run's (exec keeps it);
  - the run's exit code reaches whoever started it, in the working directory it was started in.
hot_fail.ps1 is the same on Windows, where the boxes have no Python and a restart is another process.
Prints "hot_fail: OK ..." or "hot_fail: FAIL ..."; exit 1 on any FAIL.
"""
import glob, json, os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
MAKEPAD = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.abspath("target")), "host", "makepad")
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.abspath("Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
BAD = ("parked", "paused on a panic", "stale access", "FATAL", "Segmentation")
os.environ.setdefault("MAKEPAD_RUN_STD", os.path.abspath("libs/relax/compiler/std"))


def case(name, hooks):
    tmp = tempfile.mkdtemp(prefix="hot-fail-")
    pkg = os.path.join(tmp, "pkg")
    shutil.copytree(os.path.join(HERE, "hot_fail"), pkg)
    home = os.path.join(tmp, "home")
    ready = os.path.join(tmp, "ready")
    env = dict(os.environ, MAKEPAD_RUN_LIVE_READY=ready, MAKEPAD_RUN_HOME=home, MAKEPAD_HOME=os.path.join(tmp, "makepad"),
               MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"), MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"),
               MAKEPAD_RUN_LIVE_TRACE="1", **{hook: "hot_fail_target" for hook in hooks})
    p = subprocess.Popen(JIT + ["run", "--ahead=minimal", "relax-hot-fail"], cwd=pkg, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    out, err = [], []
    def pump(f, acc):
        for line in f:
            acc.append(line.rstrip("\n"))
    for f, acc in ((p.stdout, out), (p.stderr, err)):
        threading.Thread(target=pump, args=(f, acc), daemon=True).start()

    def wait(pred):
        """Until `pred`, or the run has ended (the program stops by itself; its lines are all in by then)."""
        while not pred():
            if p.poll() is not None:
                time.sleep(0.5)
                return pred()
            time.sleep(0.02)
        return True

    def ticks(since=0):
        return [l for l in out[since:] if l.startswith("out ") and not l.startswith("out restored")]

    def pids():
        return [int(l.split()[1]) for l in out if l.startswith("pid ")]

    def ledger():
        records = []
        for path in glob.glob(os.path.join(tmp, "makepad", "devhub", "hot-outbox", "*.json")):
            try:
                records.append(json.load(open(path)))
            except ValueError:
                pass
        return records

    def outcomes():
        return sorted(r.get("outcome", "?") for r in ledger())

    fails = []
    restored = lambda: []
    before = -1
    try:
        if not wait(lambda: os.path.exists(ready) and len(ticks()) >= 20 and any("watcher gate open" in l for l in err)):
            fails.append("no-start")
        else:
            before = int(ticks()[-1].split()[1])
            n_out = len(out)
            source = os.path.join(pkg, "src", "tick.rs")
            text = open(source).read()
            with open(source + ".new", "w") as f:
                f.write(text.replace("pub fn value() -> u64 {\n    1\n}", "pub fn value() -> u64 {\n    hot_fail_target()\n}"))
            os.replace(source + ".new", source)
            restored = lambda: [int(l.split()[2]) for l in out[n_out:] if l.startswith("out restored ")]
            applied = lambda: any("value=2" in l for l in ticks(n_out))
            if not wait(lambda: restored() and applied()):
                fails.append("no-new-code" if restored() else "no-restored-state")
            elif restored()[-1] < before:
                fails.append("state-went-back")
            if not fails and not wait(lambda: "restarted" in outcomes()):
                fails.append("no-restart-record")
            if "panic" not in outcomes():
                fails.append("no-failure-record")
            loads = {r.get("load") for r in ledger() if r.get("outcome") in ("panic", "restarted")}
            if len(loads) != 1:
                fails.append("records-of-%d-loads" % len(loads))
            if not fails:
                now = pids()[-1]
                if len(pids()) != 2 or now != pids()[0]:
                    fails.append("pids-%s" % pids())
                answer = subprocess.run([MAKEPAD, "inspect", str(now), "stacks"], cwd=pkg, env=env, capture_output=True, text=True)
                if answer.returncode != 0:
                    fails.append("no-inspect: " + (answer.stdout + answer.stderr)[:200])
            if not fails:
                open(os.path.join(pkg, "exit-now"), "w").close()
                if p.wait() != 7:
                    fails.append("exit-code-%d" % p.returncode)
        wrong = next((l for l in err if any(b in l for b in BAD)), None)
        if wrong:
            fails.append("bad-line: " + wrong[:200])
    finally:
        p.kill()
        p.wait()
    if fails:
        print("hot_fail: %s: FAIL %s" % (name, " ".join(fails)))
        print("\n".join(err[-40:]))
        print("\n".join(out[-5:]))
        return False
    print("hot_fail: %s: OK restored at tick %d (edit at %d), ledger %s" % (name, restored()[-1], before, outcomes()))
    shutil.rmtree(tmp, ignore_errors=True)
    return True

results = [case("error", ["MAKEPAD_RUN_TEST_HOT_BUILD_ERROR"]), case("panic", ["MAKEPAD_RUN_TEST_HOT_BUILD_ERROR", "MAKEPAD_RUN_TEST_HOT_COMPILE_PANIC"])]
sys.exit(0 if all(results) else 1)
