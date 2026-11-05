#!/usr/bin/env python3
"""A failed hot load ends in a warm restart and a ledger report, never a parked panic; the restarted
run takes the next edit, a panic of the program's there is the program's, and an error of the JIT's
there is the JIT's.

usage: hot_fail.py [case ...]   (run from the workspace root; the JIT runs through `cargo run
  --profile host`; MAKEPAD_RUN_STD in the environment, default libs/relax/compiler/std; every case
  when none is named)

Copies hot_fail/ to a temporary dir and runs it once per case. Once it ticks, tick.rs is edited so
that a worker thread calls a fn never built before. In a process an edit ran in, that build fails:
  - error, program-panic, jit-error: with an internal compiler error (MAKEPAD_RUN_TEST_HOT_BUILD_ERROR):
    the call would park at a compile error;
  - panic: the compiler panics in the call (MAKEPAD_RUN_TEST_HOT_COMPILE_PANIC): the call gets a stub
    that panics. Its builds fail too, so that nothing builds it ahead of the call.
Then, in the restarted run (a terminal program: it never presents a frame):
  - error, panic: the next edit;
  - program-panic: a thread of the program's panics ("panic-now");
  - jit-error: that build fails again with no edit in the run (MAKEPAD_RUN_TEST_REPLACEMENT_BUILD_ERROR);
    the worker calls the fn again every tick, so calls meet the error after the run reported it too.
Each passes when:
  - the app's state came back ("out restored <tick>" at or past the tick before the edit) and, but
    for jit-error, the edit runs (value=2);
  - the ledger (the hot outbox under MAKEPAD_HOME) holds the failure and its restart, one load;
  - error, panic: the next edit is a patch (value=3, no further restart): the restore ended the
    recovery, though the program never presents a frame;
  - program-panic: the panic is the program's: the run's output and the flight recorder say it as a
    panic, no JIT error is said (no record, no verdict), and the program runs on;
  - jit-error: the error is the JIT's: a restart_failed record of the same load, the run's JIT error
    verdict line and exit code, a `jiterror` in the flight recorder, and no panic said or parked (the
    calls after the report included);
  - nothing else parks and no panic waits for anyone;
  - the restarted run answers `inspect` at its pid, the first run's (exec keeps it);
  - the run's exit code (7, or the JIT error's) reaches whoever started it, in the working directory
    it was started in.
Every wait ends on what the run does (an output line, a ledger record, its end), not on a clock (the
program stops by itself after 300 s); a failure says what came instead.
hot_fail.ps1 is the error and panic cases on Windows, where the boxes have no Python and a restart is
another process.
Prints "hot_fail: OK ..." or "hot_fail: FAIL ..."; exit 1 on any FAIL.
"""
import glob, json, os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
MAKEPAD = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.abspath("target")), "host", "makepad")
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.abspath("Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
BAD = ("parked", "paused on a panic", "stale access", "FATAL", "Segmentation")
# libs/remote hot.rs JIT_ERROR_EXIT, Verdict::line
JIT_ERROR_EXIT = 86
JIT_ERROR_LINE = "verdict: jit-error (not your change) record="
OWN_PANIC = "hot_fail: the program's own panic"
os.environ.setdefault("MAKEPAD_RUN_STD", os.path.abspath("libs/relax/compiler/std"))


def case(name, hooks, after):
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
        """Until `pred`, or the run has ended (its lines are all in by then)."""
        while not pred():
            if p.poll() is not None:
                time.sleep(0.5)
                return pred()
            time.sleep(0.02)
        return True

    def ended():
        return "the run ended (exit %d)" % p.returncode if p.poll() is not None else "the run goes on"

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

    def edit(old, new):
        source = os.path.join(pkg, "src", "tick.rs")
        text = open(source).read()
        assert text.count(old) == 1, old
        with open(source + ".new", "w") as f:
            f.write(text.replace(old, new))
        os.replace(source + ".new", source)

    def recorded(cat):
        """The flight recorder's events of kind `cat` (its recording of the whole run, saved)."""
        path = os.path.join(tmp, "rec-%s.mkpf" % cat)
        save = subprocess.run([MAKEPAD, "inspect", str(pids()[-1]), "rec", "save", "last=all", "path=" + path], cwd=pkg, env=env, capture_output=True, text=True)
        if save.returncode != 0:
            return "no recording: " + (save.stdout + save.stderr)[:200]
        events = subprocess.run([MAKEPAD, "inspect", "file=" + path, "pf", "events", "cat=" + cat, "n=100"], cwd=pkg, env=env, capture_output=True, text=True)
        return events.stdout + events.stderr

    def none_of(rec):
        """The recording holds no event of the kind asked (prof_stream ask.rs events)."""
        return "no events match" in rec or "no events recorded" in rec

    fails = []
    restored = lambda: []
    before = -1
    exit_code = 7
    allowed = None
    try:
        if not wait(lambda: os.path.exists(ready) and len(ticks()) >= 20 and any("watcher gate open" in l for l in err)):
            fails.append("no-start")
        else:
            before = int(ticks()[-1].split()[1])
            n_out = len(out)
            edit("pub fn value() -> u64 {\n    1\n}", "pub fn value() -> u64 {\n    hot_fail_target()\n}")
            restored = lambda: [int(l.split()[2]) for l in out[n_out:] if l.startswith("out restored ")]
            applied = lambda: any("value=2" in l for l in ticks(n_out))
            if after == "jit-error":
                # the restarted run's own build of the target fails: its call never returns a value
                wait(lambda: restored() and "restart_failed" in outcomes())
                if not restored():
                    fails.append("no-restored-state(%s; %s)" % (outcomes(), ended()))
                elif "restart_failed" not in outcomes():
                    fails.append("no-jit-error-record(%s; %s)" % (outcomes(), ended()))
            elif not wait(lambda: restored() and applied()):
                fails.append("no-new-code" if restored() else "no-restored-state")
            if not fails and restored()[-1] < before:
                fails.append("state-went-back")
            if not fails and after == "edit":
                # the restarted run's watcher takes the next edit (a recovery its restore never
                # ended parks the watcher, and every later edit is dropped): until its value shows,
                # a record or refusal of it comes, or the run ends
                ids = {r.get("id") for r in ledger()}
                n_later, e_later = len(out), len(err)
                edit("pub fn value() -> u64 {\n    hot_fail_target()\n}", "pub fn value() -> u64 {\n    hot_fail_target() + 1\n}")
                patched = lambda: any("value=3" in l for l in ticks(n_later))
                came = lambda: [r for r in ledger() if r.get("id") not in ids]
                refused = lambda: [l for l in err[e_later:] if "NOT applied" in l]
                verified_at = []
                def settled():
                    if patched() or refused() or any(r.get("outcome") != "verified" for r in came()):
                        return True
                    if came():
                        # verified: its value is in the next ticks
                        if not verified_at:
                            verified_at.append(len(ticks()))
                        return len(ticks()) >= verified_at[0] + 10
                    return False
                wait(settled)
                if not patched():
                    fails.append("later-edit-not-patched(records %s; %s; %s)" % (sorted(r.get("outcome") for r in came()), (refused() or ["no refusal"])[0][:160], ended()))
                elif len(pids()) != 2:
                    fails.append("later-edit-restarted-%s" % pids())
            if not fails and after == "program-panic":
                # the program's own panic after the restore: the program's, as in any run
                allowed = OWN_PANIC
                e_panic = len(err)
                open(os.path.join(pkg, "panic-now"), "w").close()
                wait(lambda: any(OWN_PANIC in l for l in err[e_panic:]))
                n_on = len(out)
                if not any(OWN_PANIC in l for l in err[e_panic:]):
                    fails.append("program-panic-not-said(%s)" % ended())
                elif not wait(lambda: len(ticks(n_on)) >= 10):
                    fails.append("program-stopped-at-its-panic(%s)" % ended())
                if "restart_failed" in outcomes():
                    fails.append("program-panic-recorded-as-the-jits(%s)" % outcomes())
                if any(JIT_ERROR_LINE in l for l in err):
                    fails.append("program-panic-said-as-a-jit-error")
                if not fails:
                    rec = recorded("panic")
                    if OWN_PANIC not in rec:
                        fails.append("program-panic-not-in-the-flight-recorder: " + rec[-300:])
                    rec = recorded("jiterror")
                    if not none_of(rec):
                        fails.append("a-jiterror-in-the-flight-recorder: " + rec[-300:])
            if not fails:
                wait(lambda: "restarted" in outcomes() or "restart_failed" in outcomes())
                if "restarted" not in outcomes():
                    fails.append("no-restart-record(%s; %s)" % (outcomes(), ended()))
            if "panic" not in outcomes():
                fails.append("no-failure-record")
            loads = {r.get("load") for r in ledger() if r.get("outcome") in ("panic", "restarted", "restart_failed")}
            if len(loads) != 1:
                fails.append("records-of-%d-loads" % len(loads))
            if not fails and after == "jit-error":
                failed = [r for r in ledger() if r.get("outcome") == "restart_failed"]
                record = failed[0].get("id", "?")
                if "MAKEPAD_RUN_TEST_REPLACEMENT_BUILD_ERROR" not in failed[0].get("reason", ""):
                    fails.append("jit-error-record-of-another-error: " + failed[0].get("reason", "")[:200])
                wait(lambda: any(JIT_ERROR_LINE + record in l for l in err))
                if not any(JIT_ERROR_LINE + record in l for l in err):
                    fails.append("no-jit-error-verdict-line(%s)" % ended())
                # the worker's calls after the report: each meets the same error, the JIT's too
                n_on = len(ticks())
                if not wait(lambda: len(ticks()) >= n_on + 10):
                    fails.append("no-ticks-after-the-jit-error(%s)" % ended())
                rec = recorded("jiterror")
                if record not in rec:
                    fails.append("jit-error-not-in-the-flight-recorder: " + rec[-300:])
                rec = recorded("panic")
                if not none_of(rec):
                    fails.append("the-jits-error-said-as-a-panic: " + rec[-300:])
                exit_code = JIT_ERROR_EXIT
            if not fails:
                now = pids()[-1]
                if len(pids()) != 2 or now != pids()[0]:
                    fails.append("pids-%s" % pids())
                answer = subprocess.run([MAKEPAD, "inspect", str(now), "stacks"], cwd=pkg, env=env, capture_output=True, text=True)
                if answer.returncode != 0:
                    fails.append("no-inspect: " + (answer.stdout + answer.stderr)[:200])
            if not fails:
                open(os.path.join(pkg, "exit-now"), "w").close()
                if p.wait() != exit_code:
                    fails.append("exit-code-%d" % p.returncode)
        wrong = next((l for l in err if any(b in l for b in BAD) and not (allowed and allowed in l)), None)
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

ERROR, PANIC, AGAIN = "MAKEPAD_RUN_TEST_HOT_BUILD_ERROR", "MAKEPAD_RUN_TEST_HOT_COMPILE_PANIC", "MAKEPAD_RUN_TEST_REPLACEMENT_BUILD_ERROR"
CASES = [("error", [ERROR], "edit"), ("panic", [ERROR, PANIC], "edit"),
         ("program-panic", [ERROR], "program-panic"), ("jit-error", [ERROR, AGAIN], "jit-error")]
asked = sys.argv[1:]
unknown = [a for a in asked if a not in [c[0] for c in CASES]]
if unknown:
    sys.exit("hot_fail: no case %s (cases: %s)" % (", ".join(unknown), ", ".join(c[0] for c in CASES)))
results = [case(*c) for c in CASES if not asked or c[0] in asked]
sys.exit(0 if all(results) else 1)
