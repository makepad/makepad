#!/usr/bin/env python3
"""Live edits next to an impl that matches `Self::Variant { .. }` struct variants.

usage: run.py --makepad <makepad binary>   (RELAX_STD in the environment)

Copies pkg/ to a temporary dir, runs it (a working tree: live) and edits src/main.rs while it runs.
ease.rs (never edited) has `match self { Self::Pow { begin, end } => .. }`; makepad's
animator.rx Ease::map has the same shape, and a Calculator layout edit was refused with
"E0412: cannot find type `Self::ExpDecay`" from it (E0425 since rustc 1.100).
  1. body edit of value()                              -> patched, output 4 becomes 5
  2a. add extra() returning 0 and call it in value()    -> patched in place, output 5 + 0 = 5;
      collecting the added item must resolve Self::Variant in the unchanged ease.rs
  2b. add field b to Pad, its initializer and its use  -> restart offered, naming main's frame;
      the previous code keeps printing 5, with no claim that Pad takes its new layout
  3. discard the pending field edit and remove extra  -> patched in the same process, output 5
  4. an invalid body edit is refused, then reverted   -> patched notification, output 5
  5. value() returns 999                              -> the program sees it and exits
No step may report an unresolved type (`cannot find type`, E0412/E0425).
Prints PASS/FAIL per step; exit 1 on any FAIL.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
relax = os.path.abspath(sys.argv[sys.argv.index("--makepad") + 1])
tmp = tempfile.mkdtemp(prefix="live-selfvariant-")
pkg = os.path.join(tmp, "pkg")
shutil.copytree(os.path.join(HERE, "pkg"), pkg)
main = os.path.join(pkg, "src", "main.rs")
base = open(main).read()
# the live driver writes this file once it watches the sources (RELAX_LIVE_READY)
ready = os.path.join(tmp, "live-ready")
env = dict(os.environ, RELAX_LIVE_READY=ready, RELAX_CACHE="0")
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

def no_unresolved_type(since):
    """Reject unresolved types regardless of error code so the Self::Variant regression stays visible."""
    return not any("cannot find type" in l for l in err[since:])

try:
    step("starts", wait(lambda: last_value() == 4 and os.path.exists(ready), 120) and no_unresolved_type(0))
    n = len(err)
    body = base.replace("p.a + ease(", "p.a + 1 + ease(")
    save(body)
    ok = wait(lambda: last_value() == 5)
    step("body edit applies", ok and no_unresolved_type(n))
    n = len(err)
    # (an added item makes the reload collect it: the step that reported the earlier unresolved types)
    item = body.replace("p.a + 1 + ease(", "p.a + 1 + extra() + ease(") + "\n/// Returns zero so collecting an added item preserves value()'s output.\nfn extra() -> u64 {\n    0\n}\n"
    m = len(out)
    save(item)
    ok = wait(lambda: len(out) > m and any("main.rs: patched" in l for l in err[n:]))
    step("added item next to a Self::Variant match patches in place", ok and last_value() == 5 and p.poll() is None
         and not any("a restart applies it; offered" in l for l in err[n:]) and no_unresolved_type(n))
    n = len(err)
    # main holds Pad for the whole run: a field added needs a restart, and none is requested here.
    layout = item.replace("    pub a: u64,\n", "    pub a: u64,\n    pub b: u64,\n").replace("Pad { a: 1 }", "Pad { a: 1, b: 8 }").replace("p.a + 1 + extra() + ease(", "p.a + 1 + p.b + extra() + ease(")
    save(layout)
    ok = wait(lambda: any("a restart applies it; offered" in l for l in err[n:]))
    m = len(out)
    time.sleep(0.5)
    report = err[n:]
    values = [int(l.split()[3]) for l in out[m:] if l.startswith("out tick ")]
    step("layout edit offers a restart naming main's frame and keeps the previous code", ok and p.poll() is None
         and bool(values) and all(v == 5 for v in values) and last_value() == 5 and no_unresolved_type(n)
         and any(l.startswith("relax live: main.rs:") and "struct `Pad` changed its layout" in l
                 and "::main` is running on thread" in l and "its frame may hold values of it in the old layout" in l
                 and "a restart applies it; offered" in l for l in report)
         and not any("takes its new layout" in l or "relax live: restarting" in l for l in report))
    n = len(err)
    save(body)
    ok = wait(lambda: last_value() == 5 and any("main.rs: patched" in l for l in err[n:]))
    step("discarding the pending field and removing the helper patches in the same process", ok and p.poll() is None and no_unresolved_type(n))
    n = len(err)
    save(body.replace("p.a + 1 + ease(", 'p.a + "invalid" + ease('))
    ok = wait(lambda: any("NOT applied" in l for l in err[n:]))
    step("invalid body is refused", ok and last_value() == 5 and no_unresolved_type(n))
    n = len(err)
    save(body)
    ok = wait(lambda: any("main.rs: patched" in l for l in err[n:]))
    step("restoring the running source reports a completed patch", ok and last_value() == 5 and no_unresolved_type(n))
    n = len(err)
    save(body.replace("    p.a + 1 + ease(&Ease::Pow { begin: 2, end: 1 }, 1u32) as u64\n", "    999\n"))
    ok = wait(lambda: p.poll() is not None)
    step("final edit applies, program exits", ok and last_value() == 999 and no_unresolved_type(n))
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
