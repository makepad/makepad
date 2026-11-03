#!/usr/bin/env python3
"""A restart-rung edit restarts the app's process: exec, one process, the app's state kept, no
leftover threads.

usage: restart_exec.py [--visible]   (run from the workspace root; the JIT runs through
  `cargo run --profile host`; MAKEPAD_RUN_STD in the environment, default libs/relax/compiler/std)

Copies live_kinds/ to a temporary dir and runs it with its window (`--features window`: a HAL app,
hidden and muted unless --visible). Once it ticks, a layout edit of the app struct is saved: a field
added whose `#[rust(..)]` initial value is code (it may name the derive's `vm`, so running values
cannot take it: the Restart rung). It passes when:
  - the restart is the exec restart ("makepad live: restarting"), never an in-process relaunch;
  - the process is the same pid afterwards (exec keeps it) and is the only one the run left;
  - the app's state came back ("out restored <tick>" at or past the tick before the edit, the
    platform's "restart (..): kept .. app state kept") and the new code runs (later=123);
  - the restarted process has no more threads than the first one had at the same point;
  - no panic, internal compiler error or fault line appears.
The restart is offered (the Restart rung) and taken as a person would, by `session relaunch`.
Prints the restart's times (the exec to the new run's first frame, the relaunch request to it) and
"restart_exec: OK ..." or "restart_exec: FAIL ..."; exit 1 on FAIL.
"""
import atexit, json, os, shutil, signal, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.abspath("Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
BAD = ("panicked", "panic parked", "stale access parked", "internal compiler error", "FATAL", "Segmentation", "SIGSEGV", "SIGBUS",
       "retaining mapping", "domain relaunch", "to first frame (window kept)")
os.environ.setdefault("MAKEPAD_RUN_STD", os.path.abspath("libs/relax/compiler/std"))
visible = "--visible" in sys.argv

tmp = tempfile.mkdtemp(prefix="restart-exec-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "live_kinds"), pkg)
manifest = os.path.join(pkg, "Cargo.toml")
ROOT = os.path.abspath(os.path.join(HERE, "../../.."))
text = open(manifest).read().replace('"../../../../widgets"', json.dumps(os.path.relpath(os.path.join(ROOT, "widgets"), pkg)))
# the workspace's [patch.crates-io] (third-party crates carried in libs/): the copy is a workspace
# of its own, and without them its window's dependencies would be looked up in the registry
# (nodes run offline)
patch, on = [], False
for line in open(os.path.join(ROOT, "Cargo.toml")):
    if line.startswith("["):
        on = line.strip() == "[patch.crates-io]"
    elif on and 'path = "' in line:
        name, rest = line.split("=", 1)
        path = rest.split('path = "', 1)[1].split('"', 1)[0]
        patch.append("%s = { path = %s }" % (name.strip(), json.dumps(os.path.relpath(os.path.join(ROOT, path), pkg))))
open(manifest, "w").write(text + "\n[patch.crates-io]\n" + "\n".join(patch) + "\n")
app_src = os.path.join(pkg, "src", "window.rs")
ready = os.path.join(tmp, "ready")
home = os.path.join(tmp, "home")
env = dict(os.environ, MAKEPAD_RUN_LIVE_READY=ready, MAKEPAD_RUN_HOME=home, MAKEPAD_RUN_CACHE_DIR=os.path.join(home, "cache"),
           MAKEPAD_RUN_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_MUTE="1", SANDBOX_MUTE="1", MUSIC_MUTE="1",
           MAKEPAD_RUN_LIVE_TRACE="1")
if not visible:
    env["MAKEPAD_HIDE_WINDOWS"] = "1"
def owned():
    """Linux: the run dies with this script (PR_SET_PDEATHSIG, kept across the exec restart), so
    a script that is killed never leaves it holding the node."""
    if sys.platform.startswith("linux"):
        import ctypes
        ctypes.CDLL(None, use_errno=True).prctl(1, signal.SIGKILL, 0, 0, 0)
p = subprocess.Popen(JIT + ["run", "--ahead=minimal", "relax-live-kinds", "--features", "window"], cwd=pkg, env=env,
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True, preexec_fn=owned)

def stop():
    """The run ends with the script, however it ends (an app left running holds the node's updates)."""
    try:
        os.killpg(p.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        p.wait(20)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        p.wait()
atexit.register(stop)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(1))
out, err = [], []
def pump(f, acc):
    for line in f:
        acc.append((time.time(), line.rstrip("\n")))
for f, acc in ((p.stdout, out), (p.stderr, err)):
    threading.Thread(target=pump, args=(f, acc), daemon=True).start()

def wait(pred, secs):
    t = time.time()
    while time.time() - t < secs:
        if pred():
            return True
        if p.poll() is not None:
            return pred()
        time.sleep(0.02)
    return False

def ticks(since=0):
    return [(t, l) for t, l in out[since:] if l.startswith("out ") and not l.startswith("out restored")]

def tick_of(line):
    return int(line.split()[1])

def processes():
    """{pid: (ppid, command name)} of every process (ps: Linux and macOS)."""
    rows = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,comm="], capture_output=True, text=True).stdout.splitlines()
    procs = {}
    for r in rows:
        w = r.split(None, 2)
        if len(w) == 3:
            procs[int(w[0])] = (int(w[1]), w[2])
    return procs

def descendants(root):
    procs = processes()
    out, todo = [], [root]
    while todo:
        q = todo.pop()
        for pid, (ppid, _) in procs.items():
            if ppid == q:
                out.append(pid)
                todo.append(pid)
    return out, procs

def app_pid():
    """The makepad process: `cargo run` itself once it execs the binary (unix), else its child."""
    kids, procs = descendants(p.pid)
    return next((k for k in [p.pid] + kids if k in procs and "makepad" in os.path.basename(procs[k][1])), None)

def threads(pid):
    if os.path.isdir("/proc/%d/task" % pid):
        return len(os.listdir("/proc/%d/task" % pid))
    rows = subprocess.run(["ps", "-M", "-p", str(pid)], capture_output=True, text=True).stdout.splitlines()
    return max(len(rows) - 1, 0)

def wrong(lines):
    return next((l.strip()[:240] for _, l in lines if any(b in l for b in BAD)), None)

def stuck(pid):
    """What a run that does not come back is doing: its own answer (`why-hung`) and, on Linux,
    each thread's state and kernel wait."""
    makepad = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "host", "makepad")
    def ask(words):
        try:
            r = subprocess.run([makepad, "inspect", str(pid)] + words, cwd=pkg, env=env, capture_output=True, text=True, timeout=60)
            return (r.stdout + r.stderr).strip()
        except subprocess.TimeoutExpired:
            return "no reply"
    for words in (["why-hung"], ["session", "view"]):
        print("restart_exec: %s: %s" % (" ".join(words), ask(words).replace("\n", " | ")[:3000]))
    # the boot's threads, each stack in full
    import re
    for name, tid in re.findall(r"(run-compile|run-open-window|run-window) \((T\d+)\)", ask(["stacks"])):
        print("restart_exec: stack %s %s: %s" % (name, tid, ask(["stacks", "thread=" + tid]).replace("\n", " | ")[:3000]))
    tasks = "/proc/%d/task" % pid
    if os.path.isdir(tasks):
        rows = {}
        for t in sorted(os.listdir(tasks), key=int):
            try:
                comm = open("%s/%s/comm" % (tasks, t)).read().strip()
                state = open("%s/%s/stat" % (tasks, t)).read().rsplit(")", 1)[1].split()[0]
                wchan = open("%s/%s/wchan" % (tasks, t)).read().strip()
            except OSError:
                continue
            rows.setdefault("%s %s %s" % (comm, state, wchan), []).append(t)
        for k, v in sorted(rows.items()):
            print("restart_exec: thread x%d %s" % (len(v), k))
        status = open("/proc/%d/status" % pid).read().splitlines()
        print("restart_exec: " + " ".join(l.replace("\t", "") for l in status if l.startswith(("SigBlk", "SigIgn"))))

fails = []
started = wait(lambda: os.path.exists(ready) and len(ticks()) >= 20 and any("watcher gate open" in l for _, l in err), 600)
if not started:
    fails.append("no-start")
pid = app_pid()
if started and pid is None:
    fails.append("no-app-process")
if not fails:
    time.sleep(2.0)
    threads0 = threads(pid)
    procs0 = len(descendants(p.pid)[0])
    before = tick_of(ticks()[-1][1])
    n_out, n_err = len(out), len(err)
    src = open(app_src).read()
    for a, b in (("    #[rust]\n    state: Option<crate::kinds::State>,\n", "    #[rust]\n    state: Option<crate::kinds::State>,\n    #[rust(later_of(vm))]\n    later: u64,\n"),
                 ("impl AppMain for App {", "fn later_of(_vm: &mut ScriptVm) -> u64 { 123 }\n\nimpl AppMain for App {"),
                 ('println!("out {} main_loop={} inlined={} {} {}", self.tick, 1,', 'println!("out {} main_loop={} later={} inlined={} {} {}", self.tick, 1, self.later,')):
        assert src.count(a) == 1, a
        src = src.replace(a, b)
    with open(app_src + ".new", "w") as f:
        f.write(src)
    os.rename(app_src + ".new", app_src)
    # the Restart rung offers the restart; the person takes it (the ring's Update ready, the WM's
    # toast, or what this does: `session relaunch`)
    offered = lambda: any("a restart applies it; offered" in l for _, l in err[n_err:])
    t_ask = None
    if not wait(offered, 120):
        fails.append("no-restart-offered")
    else:
        t_ask = time.time()
        # the built makepad (the run's own binary), not another cargo run
        makepad = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "host", "makepad")
        try:
            asked = subprocess.run([makepad, "inspect", str(pid), "session", "relaunch"], cwd=pkg, env=env, capture_output=True, text=True, timeout=60)
            print("restart_exec: session relaunch: " + (asked.stdout + asked.stderr).strip().replace("\n", " | ")[:300])
        except subprocess.TimeoutExpired:
            fails.append("relaunch-no-reply")
    restarting = lambda: next((t for t, l in err[n_err:] if "makepad live: restarting (" in l), None)
    if t_ask and not wait(lambda: restarting() is not None, 120):
        fails.append("no-exec-restart")
    t_exec = restarting()
    restored = lambda: [(t, int(l.split()[2])) for t, l in out[n_out:] if l.startswith("out restored ")]
    new_code = lambda: any("later=123" in l for _, l in ticks(n_out))
    if not wait(lambda: restored() and new_code(), 300):
        fails.append("no-new-code" if restored() else "no-restored-state")
        stuck(pid)
    elif restored()[-1][1] < before:
        fails.append("state-lost(%d<%d)" % (restored()[-1][1], before))
    report = lambda: next((l for _, l in err[n_err:] + out[n_out:] if "restart (" in l and ": kept " in l), None)
    wait(lambda: report() is not None, 30)
    if report() is None or "app state kept" not in report():
        fails.append("no-restart-report")
    # the app's first tick can come before its first present
    first = lambda: next((t for t, l in err[n_err:] + out[n_out:] if "first frame at" in l and t_exec and t >= t_exec), None)
    wait(lambda: first() is not None, 30)
    t_first = first()
    if t_first is None:
        fails.append("no-first-frame")
    time.sleep(2.0)
    pid1 = app_pid()
    procs1 = len(descendants(p.pid)[0])
    threads1 = threads(pid1) if pid1 else 0
    if pid1 != pid:
        fails.append("pid-changed(%s->%s)" % (pid, pid1))
    if procs1 != procs0:
        fails.append("processes(%d->%d)" % (procs0, procs1))
    if threads1 > threads0 + 2:
        fails.append("threads(%d->%d)" % (threads0, threads1))
    bad = wrong(err[n_err:] + out[n_out:])
    if bad:
        fails.append("bad-line")
        print("restart_exec: " + bad)
    if fails:
        # what the run said after the edit (backtrace frames left out)
        print("---- after the edit")
        print("\n".join([l[:300] for _, l in err[n_err:] if not l.startswith("  ") and "watch round" not in l][:60]))
    times = "exec->first frame %s ms, relaunch asked->first frame %s ms" % (
        "%.0f" % ((t_first - t_exec) * 1e3) if t_first and t_exec else "-", "%.0f" % ((t_first - t_ask) * 1e3) if t_first and t_ask else "-")
    print("restart_exec: %s; threads %d -> %d; processes %d -> %d; %s" % (times, threads0, threads1, procs0, procs1, report() or "no report"))
    if visible:
        # the window system's side of the reopening (MAKEPAD_HAL_LOG): when the new window's first frame was on glass
        for t, l in [x for x in err[n_err:] if "[hal" in x[1]][:12]:
            print("restart_exec: +%.0f ms %s" % ((t - t_exec) * 1e3 if t_exec else 0, l[:200]))
stop()
if fails:
    if "no-start" in fails:
        print("---- stderr tail")
        print("\n".join(l[:300] for _, l in err[-40:]))
    print("restart_exec: FAIL %s (dir %s)" % (" ".join(fails), tmp))
    sys.exit(1)
shutil.rmtree(tmp, ignore_errors=True)
print("restart_exec: OK")
