#!/usr/bin/env python3
"""A slot installed from the disk cache's prelinked startup image is replaced by a hot reload, and
its caller -- also prelinked, not edited -- calls the new code.

usage: run.py --makepad <makepad binary>   (MAKEPAD_RUN_STD in the environment)

Copies pkg/ to a temporary dir with a fresh disk cache; every run is a live session (MAKEPAD_RUN_LIVE=1):
  1. fill: run once (PRELINK_ONCE=1): the cold run records the startup manifest and its image
  2. warm: run once again: the start is prelinked (verbose "prelinked N startup slots")
  3. live: run, and while it prints "out tick <n> 11" (caller = value() * 10 + 1, value() = 1):
     a. edit value() to return 2   -> the caller prints 21, stderr "patched"
     b. edit value() to return 999 -> the caller prints 9991, the program exits
     The cache trace (MAKEPAD_RUN_CACHE_TRACE) must show value() and caller() as prelinked in run 3.
Each step waits up to 20 s. Prints PASS/FAIL per step; exit 1 on any FAIL.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
relax = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
tmp = tempfile.mkdtemp(prefix="prelink-reload-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
helper = os.path.join(pkg, "src", "helper.rs")
base = open(helper).read()
env = dict(os.environ, MAKEPAD_RUN_LIVE="1", MAKEPAD_RUN_CACHE_DIR=os.path.join(tmp, "cache"))
for k in ("MAKEPAD_RUN_CACHE", "MAKEPAD_RUN_PRELINK", "MAKEPAD_RUN_HINTS_RECORD"):
    env.pop(k, None)
cmd = [relax, "run", "--verbose", "cache-prelink-reload"]

fails = 0
def step(name, ok):
    global fails
    print("%s %s" % ("PASS" if ok else "FAIL", name))
    if not ok:
        fails += 1

def once(tag):
    p = subprocess.run(cmd, cwd=pkg, env=dict(env, PRELINK_ONCE="1"), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=600)
    open(os.path.join(tmp, tag + ".log"), "w").write(p.stdout + p.stderr)
    return p

p1 = once("fill")
step("fill run prints the result", p1.returncode == 0 and "out tick 0 11" in p1.stdout)
p2 = once("warm")
step("warm run prelinks its start", p2.returncode == 0 and "out tick 0 11" in p2.stdout and "prelinked" in p2.stderr)

trace = os.path.join(tmp, "live.trace")
p = subprocess.Popen(cmd, cwd=pkg, env=dict(env, MAKEPAD_RUN_CACHE_TRACE=trace), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
out, err = [], []
def pump(f, acc):
    for line in f:
        acc.append(line.rstrip("\n"))
threading.Thread(target=pump, args=(p.stdout, out), daemon=True).start()
threading.Thread(target=pump, args=(p.stderr, err), daemon=True).start()

def wait(pred, secs=20):
    t = time.time()
    while time.time() - t < secs:
        if pred():
            return True
        time.sleep(0.05)
    return False

def last_value():
    for l in reversed(out):
        if l.startswith("out tick "):
            return int(l.split()[3])
    return None

def save(text):
    # a new mtime even within the same second: write, then bump the time
    with open(helper, "w") as f:
        f.write(text)
    st = os.stat(helper)
    os.utime(helper, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))

step("live run starts prelinked", wait(lambda: last_value() == 11 and any("watching" in l for l in err), 120) and any("prelinked" in l for l in err))
save(base.replace("    1\n", "    2\n"))
ok = wait(lambda: last_value() == 21)
step("edit of the prelinked value(): the prelinked caller calls the new code", ok and any("patched" in l for l in err))
save(base.replace("    1\n", "    999\n"))
ok = wait(lambda: p.poll() is not None)
step("second edit applies, program exits", ok and last_value() == 9991 and p.returncode == 0)
if p.poll() is None:
    p.kill()
    p.wait()

prelinked = set()
try:
    for line in open(trace, errors="replace"):
        f = line.rstrip("\n").split("\t")
        if len(f) >= 6 and f[1] == "prelink":
            prelinked.add(f[5])
except FileNotFoundError:
    pass
named = lambda n: any(s == n or s.endswith("::" + n) for s in prelinked)
step("value() and caller() came from the startup image (%d prelinked slots traced)" % len(prelinked), named("value") and named("caller"))
if fails:
    print("---- stderr"); print("\n".join(err[-40:]))
    print("---- kept", tmp)
else:
    shutil.rmtree(tmp, ignore_errors=True)
print("cache prelink reload: %d fail" % fails)
sys.exit(1 if fails else 0)
