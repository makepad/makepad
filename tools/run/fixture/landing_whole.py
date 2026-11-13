#!/usr/bin/env python3
"""A landing that moves one of makepad's inputs and the program's own sources at once: the sources
patch now, and only makepad's input waits for the restart that is offered.

usage: landing_whole.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A program that never presents is written to a fresh dir (its own RELAX_HOME) with a build
script, and runs live under `makepad run --verbose --ahead=minimal`, printing
`out <landed::value()> <mine::side()>` when that changes:
  1. a landing writes build.rs (one of makepad's inputs: a build script) and src/landed.rs (value
     1 -> 2, and a new const SIDE = 7) in one batch: the restart is offered for the build script,
     and the program prints `out 2 0`;
  2. the person saves src/mine.rs to return landed::SIDE: it applies (`out 2 7`).
Red without the split of the batch (reload.rs: the program's sources of a batch the offer takes):
the whole batch went with the offer, so step 1 never prints `out 2 0`, and step 2 is refused with
E0425 for SIDE, which the program never took (an app, 11-07: a const and a method a landing added).
Prints `landing_whole: ok` or `landing_whole: FAIL <why>`; exit 1 on failure.
LANDING_WHOLE_TRACE=1 also prints the run's output.
"""
import os, shutil, subprocess, sys, tempfile, threading, time

ROOT = os.getcwd()
WAIT = float(os.environ.get("LANDING_WHOLE_WAIT", "300"))
MAIN = """mod landed;
mod mine;

use std::time::{Duration, Instant};

// prints the values when they change, until LANDING_WHOLE_STOP names a file that exists
fn main() {
    let stop = std::env::var("LANDING_WHOLE_STOP").unwrap_or_default();
    let start = Instant::now();
    let mut last = (u64::MAX, u64::MAX);
    while start.elapsed() < Duration::from_secs(600) && !std::path::Path::new(&stop).exists() {
        let v = (landed::value(), mine::side());
        if v != last {
            println!("out {} {}", v.0, v.1);
            last = v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("out end");
}
"""
LANDED = ("#[inline(never)]\npub fn value() -> u64 {\n    1\n}\n",
          "pub const SIDE: u64 = 7;\n\n#[inline(never)]\npub fn value() -> u64 {\n    2\n}\n")
MINE = ("#[inline(never)]\npub fn side() -> u64 {\n    0\n}\n", "#[inline(never)]\npub fn side() -> u64 {\n    crate::landed::SIDE\n}\n")
BUILD = ("fn main() {}\n", "// landed: the build script moved too\nfn main() {}\n")
CARGO = """[package]
name = "relax-landing-whole"
version = "0.1.0"
edition = "2021"
publish = false
build = "build.rs"

[workspace]
"""
BROKE = ("panicked", "internal compiler error", "FATAL", "the watcher stopped")


class Run:
    """The live run: its stdout and stderr lines as they arrive."""
    def __init__(self, jit, work, home, pkg):
        self.jit, self.out, self.err = jit, [], []
        self.ready, self.stop = os.path.join(work, "ready"), os.path.join(work, "stop")
        env = dict(os.environ, RELAX_LIVE_READY=self.ready, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"),
                   RELAX_TARGET_DIR=os.path.join(home, "target"), RELAX_LIVE_LOG=os.path.join(work, "live.jsonl"),
                   MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", LANDING_WHOLE_STOP=self.stop)
        env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
        self.p = subprocess.Popen(jit + ["run", "--quiet", "--verbose", "--ahead=minimal", "relax-landing-whole"], cwd=pkg, env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace", start_new_session=True)
        threading.Thread(target=self.pump, args=(self.p.stdout, self.out), daemon=True).start()
        threading.Thread(target=self.pump, args=(self.p.stderr, self.err), daemon=True).start()

    @staticmethod
    def pump(f, acc):
        for line in f:
            acc.append(line.rstrip("\n"))

    def has_err(self, s, since=0):
        return any(s in l for l in self.err[since:])

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

    def show(self):
        if os.environ.get("LANDING_WHOLE_TRACE"):
            for l in self.err + self.out:
                print("  | %s" % l[:240], flush=True)

    def said(self, since):
        return "; ".join(l.strip()[:200] for l in self.err[since:] if "relax live" in l or "error" in l)[:600]


def save(path, text):
    # a new mtime even within the same second: write, then bump the time
    with open(path, "w") as f:
        f.write(text)
    st = os.stat(path)
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))


def steps(r, build, landed, mine):
    if not r.wait(lambda: os.path.exists(r.ready) and "out 1 0" in r.out, WAIT):
        return "not up (%s)" % (r.err[-1][:160] if r.err else "no output")
    # 1. one landing: the build script and the program's source in one batch
    n = len(r.err)
    save(build, BUILD[1])
    save(landed, LANDED[1])
    if not r.wait(lambda: r.has_err("a build script changed", n), 30):
        return "no restart was offered for the build script (%s)" % r.said(n)
    if not r.wait(lambda: "out 2 0" in r.out, 30):
        return "the landing's program source did not patch with the build script in its batch (out: %s; %s)" % (", ".join(r.out[-3:]), r.said(n))
    # 2. a save that names what the landing added
    n = len(r.err)
    save(mine, MINE[1])
    if not r.wait(lambda: "out 2 7" in r.out, 30):
        return "the save naming the landing's new const did not apply (out: %s; %s)" % (", ".join(r.out[-3:]), r.said(n))
    return None


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    open(os.path.join(pkg, "src", "main.rs"), "w").write(MAIN)
    build, landed, mine = os.path.join(pkg, "build.rs"), os.path.join(pkg, "src", "landed.rs"), os.path.join(pkg, "src", "mine.rs")
    open(build, "w").write(BUILD[0])
    open(landed, "w").write(LANDED[0])
    open(mine, "w").write(MINE[0])
    r = Run(jit, work, home, pkg)
    why = steps(r, build, landed, mine)
    broke = r.end()
    r.show()
    return why or broke


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="landing-whole-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("landing_whole: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("landing_whole: ok")
