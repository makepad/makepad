#!/usr/bin/env python3
"""LIVEICE: a refused edit then the applied edit of a fn with closures, inside `makepad run` (the live
driver of a working tree), with the closures and their generic instances first built afterwards on
another thread. The user's Amp parked its worker thread on "internal compiler error in
{closure#N}@S: side table: no entry" after exactly this sequence.

usage: run.py --makepad <makepad binary> [--warm]   (MAKEPAD_RUN_STD in the environment)

Copies pkg/ to a temporary dir, runs it and edits src/helper.rs while it runs:
  1. `parse` calls `first_end` before it exists      -> "NOT applied" (E0425), the app keeps running
  2. `first_end` added above `parse`, parse uses it  -> "patched"
  3. the file `go` makes the worker answer requests  -> "out song" lines with the v2 parse result
                                                       (closures of the replaced version built now),
                                                       no internal compiler error
--warm: the session runs from a warm cache (a first run fills it).
Prints PASS/FAIL per step; exit 1 on any FAIL.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
relax = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
warm = "--warm" in sys.argv
tmp = tempfile.mkdtemp(prefix="live-closure-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
helper = os.path.join(pkg, "src", "helper.rs")
base = open(helper).read()
FIRST_END = '''fn first_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in text.bytes().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

'''
v1 = base.replace("let end = answer.rfind('}')", "let end = first_end(answer, start)")
assert v1 != base
v2 = v1.replace("pub fn parse(", FIRST_END + "pub fn parse(", 1)
ANSWER = 'x {"title": "T%d", "steps": 8} {"title": "Z"}'
def expect(n, version):
    a = ANSWER % (n % 3)
    end = a.index("}") + 8 if version == 2 else a.rindex("}")
    return "out song %d T%d:%d" % (n, n % 3, end)

# minimal ahead: no speculative builds before the edits (the closures stay unbuilt until `go`, as
# they were in the user's Amp; balanced ahead builds them early and hides the bug)
base_env = dict(os.environ, MAKEPAD_RUN_AHEAD="minimal")
if warm:
    env = dict(base_env, MAKEPAD_RUN_CACHE_DIR=os.path.join(tmp, "cache"), MAKEPAD_RUN_TARGET_DIR=os.path.join(tmp, "target"))
    open(os.path.join(pkg, "go"), "w").close()
    open(os.path.join(pkg, "stop"), "w").close()
    fill = subprocess.run([relax, "run", "--quiet", "relax-live-closure-app"], cwd=pkg, env=env, capture_output=True, text=True, timeout=600)
    os.remove(os.path.join(pkg, "go")); os.remove(os.path.join(pkg, "stop"))
    print("%s warm-up run fills the cache (exit %d)" % ("PASS" if fill.returncode == 0 else "FAIL", fill.returncode))
    if fill.returncode != 0:
        print(fill.stderr[-3000:]); sys.exit(1)
else:
    env = dict(base_env, MAKEPAD_RUN_CACHE="0")
p = subprocess.Popen([relax, "run", "--verbose", "relax-live-closure-app"], cwd=pkg, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
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

def save(text):
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

def ticks():
    return sum(1 for l in out if l.startswith("out tick "))
step("starts", wait(lambda: ticks() > 2 and any("watching" in l for l in err), 180))
n_err = len(err)
save(v1)
ok = wait(lambda: any("NOT applied" in l for l in err[n_err:]))
step("call to a missing fn refused", ok)
n_err = len(err)
save(v2)
ok = wait(lambda: any("patched" in l for l in err[n_err:]))
step("fn added and used: applied", ok)
n_out = len(out)
open(os.path.join(pkg, "go"), "w").close()
ok = wait(lambda: sum(1 for l in out[n_out:] if l.startswith("out song ") or l.startswith("out failed ")) >= 5)
songs = [l for l in out[n_out:] if l.startswith("out song ") or l.startswith("out failed ")]
bad = [l for l in songs if l != expect(int(l.split()[2]), 2)]
step("worker runs the new parse (%d songs, %d wrong%s)" % (len(songs), len(bad), (": " + bad[0]) if bad else ""), ok and not bad)
ice = [l for l in err if "internal compiler error" in l or "side table" in l]
step("no internal compiler error%s" % ((": " + ice[0][:300]) if ice else ""), not ice)
open(os.path.join(pkg, "stop"), "w").close()
ok = wait(lambda: p.poll() is not None)
step("program exits", ok and p.returncode == 0)
if p.poll() is None:
    p.kill()
if fails:
    print("---- stderr"); print("\n".join(err[-60:]))
shutil.rmtree(tmp, ignore_errors=True)
print("live closure app: %d fail" % fails)
sys.exit(1 if fails else 0)
