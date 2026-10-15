#!/usr/bin/env python3
"""Live edits next to an impl that matches `Self::Variant { .. }` struct variants.

usage: run.py --makepad <makepad binary>   (MAKEPAD_RUN_STD in the environment)

Copies pkg/ to a temporary dir, runs it (a working tree: live) and edits src/main.rs while it runs.
ease.rs (never edited) has `match self { Self::Pow { begin, end } => .. }`; makepad's
animator.rx Ease::map has the same shape, and a Calculator layout edit was refused with
"E0412: cannot find type `Self::ExpDecay`" from it.
  1. body edit of value()                               -> patched, new output (4)
  2. a field added to Pad and a fn added (in place)     -> patched, output 4 (the running Pad's
     new field reads its default)
  3. the field and helper are removed                 -> patched, output 5
  4. an invalid body edit is refused, then reverted   -> patched notification, output 5
  5. value() returns 999                              -> the program sees it and exits
No step may report E0412. Prints PASS/FAIL per step; exit 1 on any FAIL.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
relax = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
tmp = tempfile.mkdtemp(prefix="live-selfvariant-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
main = os.path.join(pkg, "src", "main.rs")
base = open(main).read()
env = dict(os.environ, MAKEPAD_RUN_CACHE="0")
p = subprocess.Popen([relax, "run", "--quiet", "relax-live-selfvariant"], cwd=pkg, env=env, stdout=subprocess.PIPE,
                     stderr=subprocess.PIPE, text=True, start_new_session=True)
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

def last_value():
    for l in reversed(out):
        if l.startswith("out tick "):
            return int(l.split()[3])
    return None

def save(text):
    with open(main, "w") as f:
        f.write(text)
    st = os.stat(main)
    os.utime(main, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))

fails = 0
def step(name, ok):
    global fails
    print("%s %s" % ("PASS" if ok else "FAIL", name))
    if not ok:
        fails += 1

def no_e0412(since):
    return not any("E0412" in l for l in err[since:])

try:
    step("starts", wait(lambda: last_value() == 4 and any("watching" in l for l in err), 120))
    n = len(err)
    body = base.replace("p.a + ease(", "p.a + 1 + ease(")
    save(body)
    ok = wait(lambda: last_value() == 5)
    step("body edit applies", ok and no_e0412(n))
    n = len(err)
    # (an added item makes the reload collect it: the step that reported the earlier E0412s)
    layout = body.replace("    pub a: u64,\n", "    pub a: u64,\n    pub b: u64,\n").replace("Pad { a: 1 }", "Pad { a: 1, b: 8 }").replace("p.a + 1 + ease(", "p.a + p.b + extra() + ease(") + "\nfn extra() -> u64 {\n    0\n}\n"
    save(layout)
    # (the running Pad's new field reads its default 0: 1 + 0 + 0 + 3)
    ok = wait(lambda: any("makepad live: main.rs" in l for l in err[n:]))
    time.sleep(0.5)
    step("layout edit next to a Self::Variant match: applied, no E0412 (%s)" % (last_value(),), ok and last_value() == 4 and no_e0412(n))
    n = len(err)
    save(body)
    ok = wait(lambda: last_value() == 5 and any("main.rs: patched" in l for l in err[n:]))
    step("field and helper removal applies in the same process", ok and p.poll() is None and no_e0412(n))
    n = len(err)
    save(body.replace("p.a + 1 + ease(", 'p.a + "invalid" + ease('))
    ok = wait(lambda: any("NOT applied" in l for l in err[n:]))
    step("invalid body is refused", ok and last_value() == 5)
    n = len(err)
    save(body)
    ok = wait(lambda: any("main.rs: patched" in l for l in err[n:]))
    step("restoring the running source reports a completed patch", ok and last_value() == 5)
    n = len(err)
    save(body.replace("    p.a + 1 + ease(&Ease::Pow { begin: 2, end: 1 }, 1u32) as u64\n", "    999\n"))
    ok = wait(lambda: p.poll() is not None)
    step("final edit applies, program exits", ok and last_value() == 999 and no_e0412(n))
finally:
    if p.poll() is None:
        os.killpg(p.pid, 15)
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, 9)
if fails:
    print("---- stderr"); print("\n".join(err[-40:]))
shutil.rmtree(tmp, ignore_errors=True)
print("live selfvariant: %d fail" % fails)
sys.exit(1 if fails else 0)
