#!/usr/bin/env python3
"""Invalidated code never runs again (INVALID, invalid.rs), end to end in `makepad run`: the code of
an edit the borrow checker rejects after it ran, with a thread inside it.

usage: run.py --makepad <makepad binary>   (MAKEPAD_RUN_STD in the environment)

Copies pkg/ to a temporary dir, runs it live with MAKEPAD_RUN_STALE_PARK=1 and edits src/work.rs:
  1. an edit the borrow checker rejects after it ran (SPEC-V runs it before its verdict), the worker
     blocked inside it                -> REVERTED: its code is invalidated (PROT_NONE, never reused),
                                         `stale` and `inspect code` say so
  2. free the worker                  -> it returns into the invalidated code: parked, the app pauses;
                                         `stale` says RETURNED, the reason and every stack (the
                                         INVALIDATED frame); `panics` lists it
  3. resume stop                      -> the process ends by the signal
(A layout edit with a thread in the code it rebuilds is offered as a restart and invalidates nothing:
tools/run/fixture/restart_in_place.py.)
Prints PASS/FAIL per step and its wall time; exit 1 on any FAIL.
"""
import os, re, resource, shutil, subprocess, sys, tempfile, threading, time

t_start = time.time()
HERE = os.path.dirname(os.path.abspath(__file__))
mk = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
tmp = tempfile.mkdtemp(prefix="live-invalid-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
work = os.path.join(pkg, "src", "work.rs")
base = open(work).read()
env = dict(os.environ, MAKEPAD_RUN_CACHE="0", MAKEPAD_RUN_STALE_PARK="1")
out, err = [], []
def pump(f, acc):
    for line in f:
        acc.append(line.rstrip("\n"))
# no core file: `resume stop` ends the process by the fault's signal, and writing a core of the JIT's
# reserved address space took 15-30 s
def no_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
p = subprocess.Popen([mk, "run", "--quiet", "relax-live-invalid"], cwd=pkg, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, preexec_fn=no_core)
threading.Thread(target=pump, args=(p.stdout, out), daemon=True).start()
threading.Thread(target=pump, args=(p.stderr, err), daemon=True).start()

def wait(pred, secs=30):
    t = time.time()
    while time.time() - t < secs:
        if pred():
            return True
        time.sleep(0.05)
    return False

def save(text):
    with open(work, "w") as f:
        f.write(text)
    st = os.stat(work)
    os.utime(work, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))

def send(cmd):
    p.stdin.write(cmd + "\n")
    p.stdin.flush()

def inspect(*args):
    r = subprocess.run([mk, "inspect", str(p.pid), *args], capture_output=True, text=True, timeout=90)
    return r.stdout + r.stderr

def since(acc, n, pat):
    return any(re.search(pat, l) for l in acc[n:])

def parked_id(n):
    for l in err[n:]:
        m = re.search(r"stale access parked as (P\d+)", l)
        if m:
            return m.group(1)
    return None

fails = 0
def step(name, ok, detail=""):
    global fails
    print("%s %s (t=%.1f s)" % ("PASS" if ok else "FAIL", name, time.time() - t_start))
    if not ok:
        fails += 1
        if detail:
            # (the whole answer: the gate keeps this output, and a rare failure needs all of it)
            print("  " + detail[:20000].replace("\n", "\n  "))

try:
    step("starts", wait(lambda: any(l.startswith("out tick 0 10") for l in out), 180))
    # 1. an edit the borrow checker rejects after it ran, the worker blocked inside it
    n_out, n_err = len(out), len(err)
    rejected = base.replace("""    s.a = s.a.wrapping_add(n & 1);
    10""", """    let g = crate::park_here(n + 1000000);
    let mut v = vec![s.a, g];
    let r = &v[0];
    v.push(s.a);
    *r + 40""")
    save(rejected)
    ok = wait(lambda: since(out, n_out, r"^out parked 1\d{6}$") and since(err, n_err, "REVERTED"), 60)
    step("the rejected edit ran (the worker inside it) and was REVERTED", ok, "\n".join(out[n_out:][-10:] + err[n_err:][-30:]))
    told = inspect("stale")
    step("stale: its code is invalidated (the borrow checker)", "rolled back: the borrow checker rejected it" in told and "work::step" in told, told)
    code = inspect("code")
    step("inspect code counts the invalidated group", re.search(r"invalidated \(PROT_NONE, never reused\): [1-9]\d* groups", code) is not None, code)
    # 2. the worker returns into it
    n_err = len(err)
    send("free")
    ok = wait(lambda: parked_id(n_err) is not None)
    pid = parked_id(n_err)
    told = inspect("stale")
    step("the return into the invalidated code parks the thread (%s)" % pid, ok and "RETURNED into" in told and "the borrow checker rejected it" in told and "INVALIDATED" in told, told + "\n".join(err[n_err:][-30:]))
    listed = inspect("panics")
    step("listed as a parked stale access", (pid or "?") in listed and "stale access" in listed, listed)
    time.sleep(0.5)
    step("the process waits for a decision", p.poll() is None)
    # 3. stop
    n_err = len(err)
    r = inspect("resume", pid or "P0", "stop")
    ok = wait(lambda: p.poll() is not None, 30)
    step("resume stop ends the process (status %s)" % p.poll(), ok and p.poll() != 0, r + "\n".join(err[n_err:][-20:]))
finally:
    if p.poll() is None:
        p.kill()
if fails:
    print("---- stderr"); print("\n".join(err[-60:]))
    print("---- stdout"); print("\n".join(out[-20:]))
shutil.rmtree(tmp, ignore_errors=True)
print("live invalid: %d fail (%.1f s)" % (fails, time.time() - t_start))
sys.exit(1 if fails else 0)
