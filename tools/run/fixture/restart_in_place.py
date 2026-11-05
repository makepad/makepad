#!/usr/bin/env python3
"""A restart-rung edit restarts the app in place: one process, the same window and GPU device, the
app's state kept, no leftover threads.

usage: restart_in_place.py [--visible]   (run from the workspace root; the JIT runs through
  `cargo run --profile host`; MAKEPAD_RUN_STD in the environment, default libs/relax/compiler/std)

Copies live_kinds/ to a temporary dir and runs it with its window (`--features window`: a HAL app,
hidden and muted unless --visible). Once it ticks, a layout edit of the app struct is saved: a field
added whose `#[rust(..)]` initial value is code (it may name the derive's `vm`, so running values
cannot take it: the Restart rung). It passes when:
  - the run ends in this process ("makepad-run: run end:") and no window is made for the next one;
  - the process is the same pid afterwards and is the only one the run left;
  - the app's state came back ("out restored <tick>" at or past the tick before the edit, the
    platform's "restart (..): kept .. app state kept") and the new code runs (later=123);
  - the restarted run has the edit's save time (handed over in the process, not through its
    environment): its save -> back on screen is in the session log (`restart_present`);
  - the restarted run has no more threads than the first one had at the same point;
  - no panic, internal compiler error or fault line appears.
The restart is offered (the Restart rung) and taken as a person would, by `session relaunch`.
Then the app panics for real and `resume P<n> restart` starts the next run: same process and window,
its state not kept. The same for a stale access. A failed hot load (a call of a fn whose build fails)
is a warm restart: same process and window, nothing parked, both on the ledger.
GPU work in flight at a run's end (a compute job of about a second) leaves the next run in the same
process and window with a GPU that does its work right, and the ended run's read unanswered.
Then ROUNDS more relaunches: what a run's end leaves in the process's statics
(MAKEPAD_RUN_LEFTOVERS: a heap that went stays unmapped, so a word still pointing there is a fault
too) must be a row of process_statics.tsv, and threads and resident memory must not grow.
Prints the restart's times (the run's end to the new run's first frame, the relaunch request to it)
and "restart_in_place: OK ..." or "restart_in_place: FAIL ..."; exit 1 on FAIL.
"""
import atexit, json, os, re, shutil, signal, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.abspath("Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
BAD = ("panicked", "panic parked", "stale access parked", "internal compiler error", "FATAL", "Segmentation", "SIGSEGV", "SIGBUS",
       "retaining mapping", "fault outside JIT code", "fault in host code")
os.environ.setdefault("MAKEPAD_RUN_STD", os.path.abspath("libs/relax/compiler/std"))
visible = "--visible" in sys.argv
ROUNDS = 8
# resident memory comes and goes by tens of MB with the driver's; a leak of a run's worth does not
ROUND_GROWTH_KB = 4 * 1024

ROOT = os.path.abspath(os.path.join(HERE, "../../.."))
# under the workspace, as lazy_typeck's copies: cargo reads .cargo/config.toml from the folder it
# runs in, and a copy outside it built makepad again without its rustflags (294 s of a 308 s run)
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
tmp = tempfile.mkdtemp(prefix="restart-in-place-", dir=os.path.join(ROOT, "target"))
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "live_kinds"), pkg)
manifest = os.path.join(pkg, "Cargo.toml")
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
           MAKEPAD_RUN_LIVE_TRACE="1", MAKEPAD_RUN_LIVE_LOG=os.path.join(tmp, "live.jsonl"))
if not visible:
    env["MAKEPAD_HIDE_WINDOWS"] = "1"
panic_file = os.path.join(tmp, "panic-now")
leftovers_file = os.path.join(tmp, "leftovers")
stale_file = os.path.join(tmp, "stale-now")
gpu_file = os.path.join(tmp, "gpu-now")
ledger_dir = os.path.join(tmp, "makepad", "devhub", "hot-outbox")
env["RESTART_PANIC_FILE"] = panic_file
env["RESTART_STALE_FILE"] = stale_file
env["RESTART_GPU_FILE"] = gpu_file
env["MAKEPAD_RUN_PANIC_PARK"] = "600"
env["MAKEPAD_RUN_STALE_PARK"] = "1"
env["MAKEPAD_RUN_MODULES_TRUSTED"] = "1"
env["MAKEPAD_RUN_LEFTOVERS"] = leftovers_file
env["MAKEPAD_RUN_TEST_HOT_BUILD_ERROR"] = "hot_fail_target"
env["MAKEPAD_HOME"] = os.path.join(tmp, "makepad")
def owned():
    """Linux: the run dies with this script (PR_SET_PDEATHSIG), so
    a script that is killed never leaves it holding the node."""
    if sys.platform.startswith("linux"):
        import ctypes
        ctypes.CDLL(None, use_errno=True).prctl(1, signal.SIGKILL, 0, 0, 0)
# makepad itself first: a fresh tree's build is not on the clock of the run below
subprocess.check_call(["cargo", "build", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad"], cwd=ROOT)
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
        print("restart_in_place: %s: %s" % (" ".join(words), ask(words).replace("\n", " | ")[:3000]))
    # the boot's threads, each stack in full
    import re
    for name, tid in re.findall(r"(run-compile|run-open-window|run-window) \((T\d+)\)", ask(["stacks"])):
        print("restart_in_place: stack %s %s: %s" % (name, tid, ask(["stacks", "thread=" + tid]).replace("\n", " | ")[:3000]))
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
            print("restart_in_place: thread x%d %s" % (len(v), k))
        status = open("/proc/%d/status" % pid).read().splitlines()
        print("restart_in_place: " + " ".join(l.replace("\t", "") for l in status if l.startswith(("SigBlk", "SigIgn"))))

def ask(pid, words):
    makepad = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "host", "makepad")
    return subprocess.run([makepad, "inspect", str(pid)] + words + ["as=json"], cwd=pkg, env=env, capture_output=True, text=True, timeout=60).stdout

def resident(pid):
    return int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip() or 0)

def demangled(name):
    """`_ZN14relax_compiler3jit4UNIT17h0123456789abcdefE` -> relax_compiler::jit::UNIT"""
    if not name.startswith("_ZN"):
        return name
    i, parts = 3, []
    while i < len(name) and name[i].isdigit():
        j = i
        while name[j].isdigit():
            j += 1
        n = int(name[i:j])
        parts.append(name[j:j + n])
        i = j + n
    if parts and re.fullmatch(r"h[0-9a-f]{16}", parts[-1]):
        parts.pop()
    return "::".join(parts)

def leftovers(path):
    """What the run's ends left that no row of process_statics.tsv names (the crates of this repo:
    the system's and the standard library's own statics are the process's)."""
    rows = {l.split("\t", 1)[0] for l in open(os.path.join(HERE, "process_statics.tsv")) if "\t" in l}
    found = set()
    try:
        lines = open(path).read().splitlines()
    except OSError:
        return ["no leftovers file: the runs' ends did not report"]
    for line in lines:
        if line.startswith("-> a heap that went: "):
            found.add("points into a heap that went: " + demangled(line.split()[-2]))
        elif line != "--":
            name = demangled(line.split()[0])
            if name.startswith(("relax_", "makepad_")) and name not in rows and not any(name.startswith(r + "::") for r in rows):
                found.add(name)
    return sorted(found)

def windows(pid):
    """How many native windows the process's HAL has made so far, and the ids it has them under (the
    WM's): a restart in the process adds none. (Hidden on a box with no display it has none at all.)"""
    r = subprocess.run([os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "host", "makepad"), "inspect", str(pid), "hal", "shell", "as=json"], cwd=pkg, env=env, capture_output=True, text=True, timeout=60)
    for line in r.stdout.splitlines():
        try:
            shell = json.loads(line)
        except ValueError:
            continue
        if "backend" in shell:
            return shell.get("windows_made"), sorted(w["window"] for w in shell.get("windows", []))
    return None

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
    windows0 = windows(pid)
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
            print("restart_in_place: session relaunch: " + (asked.stdout + asked.stderr).strip().replace("\n", " | ")[:300])
        except subprocess.TimeoutExpired:
            fails.append("relaunch-no-reply")
    restarting = lambda: next((t for t, l in err[n_err:] if "makepad live: restarting (" in l), None)
    if t_ask and not wait(lambda: restarting() is not None, 120):
        fails.append("no-restart")
    t_end = restarting()
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
    first = lambda: next((t for t, l in err[n_err:] + out[n_out:] if "first frame at" in l and t_end and t >= t_end), None)
    wait(lambda: first() is not None, 30)
    t_first = first()
    if t_first is None:
        fails.append("no-first-frame")
    time.sleep(2.0)
    try:
        presents = [l for l in open(env["MAKEPAD_RUN_LIVE_LOG"]) if '"event":"restart_present"' in l]
    except OSError:
        presents = []
    if not presents:
        fails.append("no-restart-present-logged")
    pid1 = app_pid()
    procs1 = len(descendants(p.pid)[0])
    threads1 = threads(pid1) if pid1 else 0
    if pid1 != pid:
        fails.append("pid-changed(%s->%s)" % (pid, pid1))
    if procs1 != procs0:
        fails.append("processes(%d->%d)" % (procs0, procs1))
    if threads1 > threads0 + 2:
        fails.append("threads(%d->%d)" % (threads0, threads1))
    if not any("makepad-run: run end:" in l for _, l in err[n_err:]):
        fails.append("no-run-end")
    windows1 = windows(pid1) if pid1 else None
    if not windows0 or windows1 != windows0:
        fails.append("window-changed(%s->%s)" % (windows0, windows1))
    bad = wrong(err[n_err:] + out[n_out:])
    if bad:
        fails.append("bad-line")
        print("restart_in_place: " + bad)
    if fails:
        # what the run said after the edit (backtrace frames left out)
        print("---- after the edit")
        print("\n".join([l[:300] for _, l in err[n_err:] if not l.startswith("  ") and "watch round" not in l][:60]))
    times = "run end->first frame %s ms, relaunch asked->first frame %s ms" % (
        "%.0f" % ((t_first - t_end) * 1e3) if t_first and t_end else "-", "%.0f" % ((t_first - t_ask) * 1e3) if t_first and t_ask else "-")
    print("restart_in_place: %s; threads %d -> %d; processes %d -> %d; %s" % (times, threads0, threads1, procs0, procs1, report() or "no report"))
    if visible:
        # the window system's side of the reopening (MAKEPAD_HAL_LOG): when the new window's first frame was on glass
        for t, l in [x for x in err[n_err:] if "[hal" in x[1]][:12]:
            print("restart_in_place: +%.0f ms %s" % ((t - t_end) * 1e3 if t_end else 0, l[:200]))
def stayed(stage, n_err):
    """A stage's restart was the next run in this process and window, and nothing broke."""
    if app_pid() != pid:
        fails.append(stage + "-pid-changed")
    if windows(pid) != windows0:
        fails.append(stage + "-window-changed")
    if not any("run's threads stayed" in l for _, l in err[n_err:]):
        fails.append(stage + "-no-run-end")
    # (each of these stages means to panic, or to fail a build)
    for _, l in err[n_err:]:
        if any(b in l for b in BAD[4:]):
            fails.append(stage + "-broke")
            print("restart_in_place: %s: %s" % (stage, l.strip()[:240]))
            print("\n".join(x[:260] for _, x in err[n_err:][-30:]))
            break

def parked():
    return next(iter(re.findall(r'"id":"(P\d+)"', ask(pid, ["panics"]))), None)

def park_and_restart(stage, trigger):
    """What parks (a panic, a stale access) and is told `resume P<n> restart`: the next run, its state fresh."""
    n_out, n_err = len(out), len(err)
    open(trigger, "w").close()
    wait(lambda: parked() is not None, 60)
    panic = parked()
    if panic is None:
        fails.append(stage + "-not-parked")
        print("restart_in_place: %s: panics: %s" % (stage, ask(pid, ["panics"])[:600]))
        print("\n".join(l[:300] for _, l in err[n_err:][-40:] if not l.startswith("makepad live: a change")))
        print("\n".join(l[:160] for _, l in out[n_out:][-3:]))
        return
    reply = ask(pid, ["resume", panic, "restart"])
    print("restart_in_place: %s: resume %s restart: %s" % (stage, panic, reply[:200]))
    if '"as":"restart"' not in reply or '"window_kept":true' not in reply or '"state_kept":false' not in reply:
        fails.append(stage + "-resume-reply")
    if not wait(lambda: any(tick_of(l) < 5 for _, l in ticks(n_out)), 120):
        fails.append(stage + "-no-next-run")
        stuck(pid)
    time.sleep(1.0)
    stayed(stage, n_err)
    if any(l.startswith("out restored") for _, l in out[n_out:]):
        fails.append(stage + "-state-kept")
    if parked() is not None:
        fails.append(stage + "-still-listed")

# a real panic of the app's, and a stale access (a call into a module gone): the next run, in the
# same process and window
if not fails:
    park_and_restart("panic", panic_file)
if not fails:
    park_and_restart("stale", stale_file)
# a failed hot load (the app's main thread calls a fn whose build fails, so it cannot hand over its
# state): the warm restart is the next run, in the same process and window; nothing parks; the ledger
# has the failure and its restart
if not fails:
    n_out, n_err = len(out), len(err)
    src = open(app_src).read()
    for a, b in (("                self.tick += 1;\n", "                self.tick += hot_fail_target();\n"),
                 ("impl AppMain for App {", "#[inline(never)]\nfn hot_fail_target() -> u64 { 1 }\n\nimpl AppMain for App {")):
        assert src.count(a) == 1, a
        src = src.replace(a, b)
    with open(app_src + ".new", "w") as f:
        f.write(src)
    os.rename(app_src + ".new", app_src)
    if not wait(lambda: any(tick_of(l) < 5 for _, l in ticks(n_out)), 120):
        fails.append("hot-fail-no-next-run")
        stuck(pid)
    def outcomes():
        found = []
        for name in os.listdir(ledger_dir) if os.path.isdir(ledger_dir) else []:
            try:
                found.append(json.load(open(os.path.join(ledger_dir, name))).get("outcome"))
            except ValueError:
                pass
        return found
    if not fails and not wait(lambda: "restarted" in outcomes() and "panic" in outcomes(), 60):
        fails.append("hot-fail-ledger(%s)" % outcomes())
    if not fails:
        stayed("hot-fail", n_err)
    if parked() is not None:
        fails.append("hot-fail-parked")
# GPU work in flight at the run's end (a compute job of seconds): the next run in the same process and
# window, its GPU work right; the ended run's read is never answered into it
GPU_LONG = 400_000_000
def gpu_cue(iters):
    with open(gpu_file + ".new", "w") as f:
        f.write(str(iters))
    os.rename(gpu_file + ".new", gpu_file)
gpu_lines = lambda since: [l for _, l in out[since:] if l.startswith("gpu: ")]
if not fails:
    n_out, n_err = len(out), len(err)
    gpu_cue(GPU_LONG)
    if not wait(lambda: gpu_lines(n_out), 60) or not gpu_lines(n_out)[0].startswith("gpu: in flight"):
        fails.append("gpu-not-started")
        print("restart_in_place: gpu: %s" % (gpu_lines(n_out) or "no answer"))
    else:
        time.sleep(0.3)
        ask(pid, ["session", "relaunch"])
        if not wait(lambda: any(l.startswith("out restored") for _, l in out[n_out:]), 120):
            fails.append("gpu-no-next-run")
            stuck(pid)
    if not fails:
        long_done = [l for l in gpu_lines(n_out) if l.startswith("gpu: done %d" % GPU_LONG)]
        if long_done:
            # (the job ended before the run did: nothing was in flight)
            fails.append("gpu-not-in-flight")
            print("restart_in_place: gpu: " + long_done[0])
        n_next = len(out)
        gpu_cue(16)
        answer = lambda: next((l for l in gpu_lines(n_next) if re.match(r"gpu: (done|failed|read failed) 16 ", l)), None)
        wait(lambda: answer() is not None, 60)
        short = answer()
        print("restart_in_place: gpu: after a run ended in a job: %s" % short)
        if short is None or not short.startswith("gpu: done 16 ok"):
            fails.append("gpu-next-run")
        stayed("gpu", n_err)
        lost = next((l for _, l in err[n_err:] if "device lost" in l.lower() or "device_lost" in l.lower()), None)
        if lost:
            fails.append("gpu-device-lost")
            print("restart_in_place: gpu: " + lost.strip()[:240])
# rounds of relaunches: nothing grows, nothing of a run stays in the process's statics
if not fails:
    time.sleep(1.0)
    rss, nthreads = [], []
    for round in range(ROUNDS):
        n_out, n_err = len(out), len(err)
        ask(pid, ["session", "relaunch"])
        if not wait(lambda: any(l.startswith("out restored") for _, l in out[n_out:]) and len(ticks(n_out)) > 5, 60):
            fails.append("round-%d-no-next-run" % round)
            stuck(pid)
            break
        time.sleep(0.5)
        rss.append(resident(pid))
        nthreads.append(threads(pid))
    print("restart_in_place: rounds: resident MB %s; threads %s" % ([r >> 10 for r in rss], nthreads))
    if nthreads and max(nthreads) > threads0 + 2:
        fails.append("threads-grew")
    if len(rss) > 2 and min(rss[-3:]) > min(rss[:3]) + ROUND_GROWTH_KB * ROUNDS:
        fails.append("resident-grew")
    # (MAKEPAD_RUN_LEFTOVERS is Linux's: run_end::leftovers)
    left = leftovers(leftovers_file) if sys.platform.startswith("linux") else []
    if left:
        fails.append("leftovers")
        print("restart_in_place: left of a run in statics no row of process_statics.tsv names:\n  " + "\n  ".join(left))
stop()
if fails:
    if "no-start" in fails:
        print("---- stderr tail")
        print("\n".join(l[:300] for _, l in err[-40:]))
    print("restart_in_place: FAIL %s (dir %s)" % (" ".join(fails), tmp))
    sys.exit(1)
shutil.rmtree(tmp, ignore_errors=True)
print("restart_in_place: OK")
