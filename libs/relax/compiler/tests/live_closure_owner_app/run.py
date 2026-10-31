#!/usr/bin/env python3
"""JITPATCH: a live patch of main.rs while handlers of the replaced version are kept in a registry
(as Fab's script_mod! design keeps its closures). The handlers run only after the patch: they are
the old version's closures and build closures of OTHER old-version fns (`adder`, `scaled`) whose
bodies were never lowered in this process (warm: their code came from the disk cache). Fab's
`makepad run` panicked there: "the environment of a closure of a replaced version of main.rs
(expression N) is unknown: no body of that version was lowered in this process".

usage: run.py --makepad <makepad binary> [--warm]   (MAKEPAD_RUN_STD in the environment)

  1. main.rs: a fn added above `adder` (ids shift), `main` prints with it -> "patched"
  2. the file `go`: the stored v0 handlers run -> "out val <n> <v>" with v0's values, no ICE
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
relax = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
warm = "--warm" in sys.argv
tmp = tempfile.mkdtemp(prefix="live-closure-owner-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
main_rs = os.path.join(pkg, "src", "main.rs")
base = open(main_rs).read()
v1 = base.replace("fn adder(", "fn shift(v: u64) -> u64 {\n    let s = |a: u64| a.wrapping_mul(3);\n    s(v) + 1\n}\n\nfn adder(", 1)
v1 = v1.replace('println!("out tick {}", n);', 'println!("out tick {} {}", n, shift(1));', 1)
assert v1.count("fn shift") == 1 and "shift(1)" in v1

def expect(n):
    a = (n % 5) * 10 + 1 + 7
    b = ((n % 3) * 10 + 7) * 2
    return "out val %d %d" % (n, a + b)

base_env = dict(os.environ, MAKEPAD_RUN_AHEAD="minimal")
if warm:
    env = dict(base_env, MAKEPAD_RUN_CACHE_DIR=os.path.join(tmp, "cache"), MAKEPAD_RUN_TARGET_DIR=os.path.join(tmp, "target"))
    open(os.path.join(pkg, "go"), "w").close()
    open(os.path.join(pkg, "stop"), "w").close()
    fill = subprocess.run([relax, "run", "--quiet", "relax-live-closure-owner-app"], cwd=pkg, env=env, capture_output=True, text=True, timeout=600)
    os.remove(os.path.join(pkg, "go")); os.remove(os.path.join(pkg, "stop"))
    print("%s warm-up run fills the cache (exit %d)" % ("PASS" if fill.returncode == 0 else "FAIL", fill.returncode))
    if fill.returncode != 0:
        print(fill.stderr[-3000:]); sys.exit(1)
else:
    env = dict(base_env, MAKEPAD_RUN_CACHE="0")
p = subprocess.Popen([relax, "run", "--verbose", "relax-live-closure-owner-app"], cwd=pkg, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
out, err = [], []
def pump(f, acc):
    for line in f:
        acc.append(line.rstrip("\n"))
threading.Thread(target=pump, args=(p.stdout, out), daemon=True).start()
threading.Thread(target=pump, args=(p.stderr, err), daemon=True).start()

def wait(pred, secs=30):
    t = time.time()
    while time.time() - t < secs:
        if pred():
            return True
        time.sleep(0.05)
    return False

fails = 0
def step(name, ok):
    global fails
    print("%s %s" % ("PASS" if ok else "FAIL", name))
    if not ok:
        fails += 1

step("starts", wait(lambda: sum(1 for l in out if l.startswith("out tick ")) > 2 and any("watching" in l for l in err), 180))
n_err = len(err)
with open(main_rs, "w") as f:
    f.write(v1)
st = os.stat(main_rs)
os.utime(main_rs, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))
step("main.rs patched", wait(lambda: any("patched" in l for l in err[n_err:])))
n_out = len(out)
open(os.path.join(pkg, "go"), "w").close()
ok = wait(lambda: sum(1 for l in out[n_out:] if l.startswith("out val ")) >= 5)
vals = [l for l in out[n_out:] if l.startswith("out val ")]
bad = [l for l in vals if l != expect(int(l.split()[2]))]
step("the v0 handlers run after the patch (%d values, %d wrong%s)" % (len(vals), len(bad), (": " + bad[0]) if bad else ""), ok and not bad)
ice = [l for l in err if "internal compiler error" in l or "is unknown" in l or "panicked" in l]
step("no internal compiler error%s" % ((": " + ice[0][:300]) if ice else ""), not ice)
open(os.path.join(pkg, "stop"), "w").close()
ok = wait(lambda: p.poll() is not None)
step("program exits", ok and p.returncode == 0)
if p.poll() is None:
    p.kill()
if fails:
    print("---- stderr"); print("\n".join(err[-60:]))
shutil.rmtree(tmp, ignore_errors=True)
print("live closure owner app: %d fail" % fails)
sys.exit(1 if fails else 0)
