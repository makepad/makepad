#!/usr/bin/env python3
"""A patch reaches O1 code the startup image linked: the code that inlined the patched body unguarded
is built again.

usage: warm_image_patch.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A program that never presents is written to a fresh dir (its own RELAX_HOME) and runs twice
under `makepad run --verbose --ahead=minimal` with O3 and startup baseline code off (RELAX_O3=0,
RELAX_O1=0), so the code it runs is O1:
  - `leaf` is inlined into `host` with no call and no loop before the site, so the copy runs
    unguarded (opt::unguard_prefix): a patch of `leaf` builds `host` again (reload: the slots that
    inlined it, `inlined_into`);
  - the first run boots cold with `--check-first` (a body is checked before its first code, so
    none is built as unverified baseline code), ends on its own, and its exit writes the startup
    image;
  - the second run (no `--check-first`: that places nothing from the image) links `host`'s code
    from the image, `leaf` is patched (1 -> 2), and the program prints 2 at `host`'s next call.
`makepad inspect <pid> versions` must show `host`'s O1 code holding `leaf`'s copy in the first run
and `host` running that code from the image in the second.
Red without the image install's inlining record below O3 (jit install_prelinked_slot): `host` was
never built again and printed 1 until it ended.
Prints `warm_image_patch: ok` or `warm_image_patch: FAIL <why>`; exit 1 on failure.
WARM_IMAGE_TRACE=1 also prints both runs' output.
"""
import os, re, shutil, subprocess, sys, tempfile, threading, time

ROOT = os.getcwd()
WAIT = float(os.environ.get("WARM_IMAGE_WAIT", "300"))
LEAF = ("fn leaf() -> u64 {\n    1\n}", "fn leaf() -> u64 {\n    2\n}")
MAIN = """use std::time::{Duration, Instant};

// inlined into `host` before any call or loop: an unguarded copy
%s

#[inline(never)]
fn host(x: u64) -> u64 {
    leaf() + x
}

// prints `host`'s value when it changes, until WARM_IMAGE_SECS pass or the file WARM_IMAGE_STOP names
// exists
fn main() {
    let secs: u64 = std::env::var("WARM_IMAGE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let stop = std::env::var("WARM_IMAGE_STOP").unwrap_or_default();
    let start = Instant::now();
    let mut last = u64::MAX;
    while start.elapsed() < Duration::from_secs(secs) && !std::path::Path::new(&stop).exists() {
        let v = host(std::hint::black_box(0));
        if v != last {
            println!("out {}", v);
            last = v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("out end");
}
""" % LEAF[0]
CARGO = """[package]
name = "relax-warm-image-patch"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]
"""
BROKE = ("panicked", "internal compiler error", "FATAL", "the watcher stopped")


class Run:
    """One run: its stdout and stderr lines as they arrive."""
    def __init__(self, jit, work, home, pkg, secs, args):
        self.jit, self.out, self.err = jit, [], []
        self.ready, self.stop = os.path.join(work, "ready"), os.path.join(work, "stop")
        for f in (self.ready, self.stop):
            if os.path.exists(f):
                os.remove(f)
        env = dict(os.environ, RELAX_LIVE_READY=self.ready, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"),
                   RELAX_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", RELAX_O3="0",
                   RELAX_O1="0", WARM_IMAGE_SECS=str(secs), WARM_IMAGE_STOP=self.stop)
        env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
        self.p = subprocess.Popen(jit + ["run", "--quiet", "--verbose", "--ahead=minimal"] + args + ["relax-warm-image-patch"], cwd=pkg, env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace", start_new_session=True)
        threading.Thread(target=self.pump, args=(self.p.stdout, self.out), daemon=True).start()
        threading.Thread(target=self.pump, args=(self.p.stderr, self.err), daemon=True).start()

    @staticmethod
    def pump(f, acc):
        for line in f:
            acc.append(line.rstrip("\n"))

    def has_err(self, s, since=0):
        return any(s in l for l in self.err[since:])

    def has_out(self, s):
        return s in self.out

    def wait(self, pred, secs):
        t = time.time()
        while time.time() - t < secs:
            if pred():
                return True
            if self.p.poll() is not None:
                time.sleep(0.2)
                return pred()
            time.sleep(0.05)
        return False

    def end(self):
        """Stops the program and waits for the run to end: a line saying it broke, or None."""
        open(self.stop, "w").close()
        self.p.wait()
        time.sleep(0.2)
        return next(("broke: " + l.strip()[:200] for l in self.err if any(b in l for b in BROKE)), None)

    def placed(self):
        m = next((re.search(r"prelinked (\d+) startup slots", l) for l in self.err if "prelinked " in l and " startup slots" in l), None)
        return int(m.group(1)) if m else 0

    def versions(self, f, when):
        """`makepad inspect <pid> versions <f>`: the fn's code versions and the slots holding its copies."""
        m = next((re.search(r"makepad-remote: pid (\d+)", l) for l in self.err if "makepad-remote: pid " in l), None)
        pid = m.group(1) if m else str(self.p.pid)
        c = subprocess.run(self.jit + ["inspect", pid, "versions", f], capture_output=True, text=True, errors="replace")
        if os.environ.get("WARM_IMAGE_TRACE"):
            print("  inspect %s, versions %s:\n%s%s" % (when, f, c.stdout, c.stderr), flush=True)
        return c.stdout

    def show(self, tag):
        if os.environ.get("WARM_IMAGE_TRACE"):
            for l in self.err + self.out:
                print("  %s | %s" % (tag, l[:240]), flush=True)


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    main = os.path.join(pkg, "src", "main.rs")
    open(main, "w").write(MAIN)
    r = Run(jit, work, home, pkg, 120, ["--check-first"])
    leaf = r.versions("leaf", "in the first run") if r.wait(lambda: r.has_out("out 1"), WAIT) else ""
    copy = next((l for l in leaf.splitlines() if "its code is inlined into" in l), "")
    broke = r.end()
    r.show("cold")
    if broke or not r.has_out("out 1") or not r.has_out("out end"):
        return "first run: %s" % (broke or "it did not print 1 and end (%s)" % (r.err[-1][:160] if r.err else "no output"))
    if "host (S" not in copy or "O1 installed" not in copy:
        return "first run: host's O1 code does not hold leaf's copy (%s)" % (copy[:160] or leaf[:160] or "no answer")
    if not r.has_err("startup image of "):
        return "first run: no startup image was written"
    r = Run(jit, work, home, pkg, 120, [])
    up = r.wait(lambda: os.path.exists(r.ready) and r.has_out("out 1"), WAIT)
    host = r.versions("host", "before the patch").split("\n", 1)[0] if up else ""
    if not up or "runs O1 warm-image code" not in host:
        r.end()
        r.show("warm")
        return "second run: %s" % ("host does not run the image's O1 code (%s)" % host[:160] if up else "not up")
    n = len(r.err)
    s = open(main).read()
    open(main, "w").write(s.replace(LEAF[0], LEAF[1]))
    patched = r.wait(lambda: r.has_err(": patched (", n), WAIT)
    reached = patched and r.wait(lambda: r.has_out("out 2"), 20)
    r.versions("host", "after the patch")
    broke = r.end()
    r.show("warm")
    if broke:
        return "second run: " + broke
    if not r.placed():
        return "second run: no slot was placed from the startup image"
    if not patched:
        return "second run: the edit of leaf was not patched"
    if not reached:
        return "second run: host still ran its old copy of leaf after the patch (out: %s)" % ", ".join(r.out[-4:])
    return None


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="warm-image-patch-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("warm_image_patch: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("warm_image_patch: ok")
