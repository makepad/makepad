#!/usr/bin/env python3
"""The live driver inside `makepad run` (a working tree runs live, reload.rs start_watcher), end to end.

usage: run.py --makepad <makepad binary> [--warm]   (MAKEPAD_RUN_STD in the environment)

Copies pkg/ to a temporary dir, runs it (a working tree: live) and edits src/helper.rs while it runs:
  1. body edit: value() returns 2                  -> stdout shows 2, stderr "patched"
  2. type error in value()                          -> stderr "NOT applied" + the error; output stays 2
  3. struct field added to Conf (a Copy struct)     -> a state-preserving restart: the program
                                                     restarts, gets its tick count back and runs
                                                     the new code (value 1 + extra 5 = 6)
  4. the struct back + value() returns 999          -> the program sees 999 and exits
Each step waits up to 20 s. Prints PASS/FAIL per step; exit 1 on any FAIL.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
relax = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
tmp = tempfile.mkdtemp(prefix="live-app-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
helper = os.path.join(pkg, "src", "helper.rs")
base = open(helper).read()
# --warm: a live session from a warm cache. A first live run (one tick, no edit) fills a fresh
# cache; the edits below then run on a program restored from it (collected graph), not parsed.
warm = "--warm" in sys.argv
if warm:
    env = dict(os.environ, MAKEPAD_RUN_CACHE_DIR=os.path.join(tmp, "cache"), MAKEPAD_RUN_TARGET_DIR=os.path.join(tmp, "target"))
    fill = subprocess.run([relax, "run", "--quiet", "relax-live-app"], cwd=pkg, env=dict(env, LIVE_APP_TICKS="1"), capture_output=True, text=True, timeout=600)
    print("%s warm-up run fills the cache (exit %d)" % ("PASS" if fill.returncode == 0 else "FAIL", fill.returncode))
    if fill.returncode != 0:
        print(fill.stderr[-3000:]); sys.exit(1)
else:
    env = dict(os.environ, MAKEPAD_RUN_CACHE="0")
p = subprocess.Popen([relax, "run", "--verbose" if warm else "--quiet", "relax-live-app"], cwd=pkg, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
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

fails = 0
def step(name, ok):
    global fails
    print("%s %s" % ("PASS" if ok else "FAIL", name))
    if not ok:
        fails += 1

step("starts", wait(lambda: last_value() == 1 and any("watching" in l for l in err), 120))
if warm:
    step("live start restored the collected graph", any(l.startswith("makepad-run collected: restored") for l in err))
t = time.time()
save(base.replace("conf().base\n", "conf().base + 1\n"))
ok = wait(lambda: last_value() == 2)
step("body edit applies (%.0f ms save -> new output)" % ((time.time() - t) * 1e3), ok and any("patched" in l for l in err))
n_err = len(err)
save(base.replace("conf().base\n", "conf().base + \"x\"\n"))
ok = wait(lambda: any("NOT applied" in l for l in err[n_err:]))
time.sleep(0.3)
step("type error refused, app keeps running the previous code", ok and last_value() == 2)
n_err = len(err)
n_before = max((int(l.split()[2]) for l in out if l.startswith("out tick ")), default=0)
layout = base.replace("    pub base: u64,\n", "    pub base: u64,\n    pub extra: u8,\n").replace("Conf { base: 1 }", "Conf { base: 1, extra: 5 }").replace("conf().base\n", "conf().base + conf().extra as u64\n")
save(layout)
ok = wait(lambda: last_value() == 6)
restored = [int(l.split()[2]) for l in out if l.startswith("out restored ")]
step("layout edit: state-preserving restart (restored tick %s, before %d), new code runs" % (restored[-1] if restored else None, n_before), ok and bool(restored) and restored[-1] >= n_before)
save(layout.replace("conf().base + conf().extra as u64\n", "999\n"))
ok = wait(lambda: p.poll() is not None)
step("final edit applies, program exits", ok and last_value() == 999)
if p.poll() is None:
    p.kill()
if fails:
    print("---- stderr"); print("\n".join(err[-40:]))
shutil.rmtree(tmp, ignore_errors=True)
print("live app: %d fail" % fails)
sys.exit(1 if fails else 0)
