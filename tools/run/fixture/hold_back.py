#!/usr/bin/env python3
"""An edit with an item the running program cannot take holds back only that item and what needs
it: the rest of the edit patches, and the held items wait for the restart that is offered.

usage: hold_back.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A program that never presents is written to a fresh dir (its own RELAX_HOME, a git
repository by hand: HEAD names refs/heads/main) and runs live under `makepad run --verbose
--ahead=minimal`, printing `out <parts::value()> <parts::reader()> <other::other()> <parts::count()>`
when that changes:
  0. `inspect live` says the commit the run loaded (loaded_sha: HEAD's commit A);
  1. one save of src/parts.rs changes the type of const LIMIT (u32 -> (u32, u32)) and of type alias
     Count (u32 -> u64), which take the restart, and the bodies of value (1 -> 2), count (7 -> 8)
     and reader (`LIMIT as u64` -> `LIMIT.1 as u64`, which needs LIMIT's new type): value and count
     patch (`out 2 3 0 8`), LIMIT, Count and reader keep their running text, and the restart is
     offered for LIMIT (`inspect live`: restart_pending, and pending_why names LIMIT);
  2. the person saves src/other.rs (0 -> 5): it applies (`out 2 3 5 8`).
Red without the hold-back (reload.rs hold_back): the save took the restart whole, so step 1 never
prints `out 2 3 0 8` (an app, 11-07: two consts, a type alias and a layout held nine hours of
landings back). Red without the loaded heads (reload_inputs.rs collect): step 0 has no
loaded_sha. Red without the pending edit's time (reload.rs restart_pending_since): step 1 has no
pending_why.
Prints `hold_back: ok` or `hold_back: FAIL <why>`; exit 1 on failure.
HOLD_BACK_TRACE=1 also prints the run's output.
"""
import json, os, re, shutil, subprocess, sys, tempfile, threading, time

ROOT = os.getcwd()
WAIT = float(os.environ.get("HOLD_BACK_WAIT", "300"))
MAIN = """mod other;
mod parts;

use std::time::{Duration, Instant};

// prints the values when they change, until HOLD_BACK_STOP names a file that exists
fn main() {
    let stop = std::env::var("HOLD_BACK_STOP").unwrap_or_default();
    let start = Instant::now();
    let mut last = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
    while start.elapsed() < Duration::from_secs(600) && !std::path::Path::new(&stop).exists() {
        let v = (parts::value(), parts::reader(), other::other(), parts::count() as u64);
        if v != last {
            println!("out {} {} {} {}", v.0, v.1, v.2, v.3);
            last = v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("out end");
}
"""
PARTS = ("""pub const LIMIT: u32 = 3;

pub type Count = u32;

#[inline(never)]
pub fn value() -> u64 {
    1
}

#[inline(never)]
pub fn reader() -> u64 {
    LIMIT as u64
}

#[inline(never)]
pub fn count() -> Count {
    7
}
""", """pub const LIMIT: (u32, u32) = (3, 4);

pub type Count = u64;

#[inline(never)]
pub fn value() -> u64 {
    2
}

#[inline(never)]
pub fn reader() -> u64 {
    LIMIT.1 as u64
}

#[inline(never)]
pub fn count() -> Count {
    8
}
""")
OTHER = ("#[inline(never)]\npub fn other() -> u64 {\n    0\n}\n", "#[inline(never)]\npub fn other() -> u64 {\n    5\n}\n")
CARGO = """[package]
name = "relax-hold-back"
version = "0.1.0"
edition = "2021"
publish = false

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
                   MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", HOLD_BACK_STOP=self.stop)
        env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
        self.p = subprocess.Popen(jit + ["run", "--quiet", "--verbose", "--ahead=minimal", "relax-hold-back"], cwd=pkg, env=env,
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

    def inspect(self, *words):
        m = next((re.search(r"makepad-remote: pid (\d+)", l) for l in self.err if "makepad-remote: pid " in l), None)
        pid = m.group(1) if m else str(self.p.pid)
        c = subprocess.run(self.jit + ["inspect", pid] + list(words), capture_output=True, text=True, errors="replace")
        return c.stdout + c.stderr

    def end(self):
        """Stops the program and waits for the run to end: a line saying it broke, or None."""
        open(self.stop, "w").close()
        self.p.wait()
        time.sleep(0.2)
        return next(("broke: " + l.strip()[:200] for l in self.err if any(b in l for b in BROKE)), None)

    def show(self):
        if os.environ.get("HOLD_BACK_TRACE"):
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


SHA = "a" * 40


def live(r):
    """`inspect live`'s row (its JSON answer)."""
    said = r.inspect("live", "as=json")
    return next((json.loads(l) for l in said.splitlines() if l.startswith("{") and '"loaded_sha"' in l), None), said


def steps(r, parts, other):
    if not r.wait(lambda: os.path.exists(r.ready) and "out 1 3 0 7" in r.out, WAIT):
        return "not up (%s)" % (r.err[-1][:160] if r.err else "no output")
    # 0. the commit the run loaded
    row, said = live(r)
    if not row or row.get("loaded_sha") != SHA or not row.get("loaded_ms"):
        return "`inspect live` does not say the run loaded commit A: %s" % said.strip()[:600]
    # 1. a save with items the running program cannot take
    n = len(r.err)
    save(parts, PARTS[1])
    if not r.wait(lambda: "out 2 3 0 8" in r.out, 60):
        return "the save's other items did not patch with LIMIT and Count held (out: %s; %s)" % (", ".join(r.out[-3:]), r.said(n))
    if not r.wait(lambda: r.has_err("the type of const `LIMIT` changed", n) and r.has_err("offered", n), 20):
        return "no restart was offered for LIMIT (%s)" % r.said(n)
    if any(o.startswith("out 2 4 ") for o in r.out):
        return "reader ran its new text against LIMIT's running value"
    # what the app does not run yet, in `live`: the oldest edit waiting for the restart
    row, said = live(r)
    if not row or row.get("restart_pending", 0) < 1 or "LIMIT" not in row.get("pending_why", "") or not row.get("pending_since_ms"):
        return "`inspect live` does not say the held edit waits for a restart: %s" % said.strip()[:300]
    # 2. a later save
    n = len(r.err)
    save(other, OTHER[1])
    if not r.wait(lambda: "out 2 3 5 8" in r.out, 30):
        return "a later save did not apply while LIMIT waited for the restart (out: %s; %s)" % (", ".join(r.out[-3:]), r.said(n))
    return None


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    open(os.path.join(pkg, "src", "main.rs"), "w").write(MAIN)
    parts, other = os.path.join(pkg, "src", "parts.rs"), os.path.join(pkg, "src", "other.rs")
    open(parts, "w").write(PARTS[0])
    open(other, "w").write(OTHER[0])
    os.makedirs(os.path.join(pkg, ".git", "refs", "heads"))
    open(os.path.join(pkg, ".git", "HEAD"), "w").write("ref: refs/heads/main\n")
    open(os.path.join(pkg, ".git", "refs", "heads", "main"), "w").write(SHA + "\n")
    r = Run(jit, work, home, pkg)
    why = steps(r, parts, other)
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
work = tempfile.mkdtemp(prefix="hold-back-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("hold_back: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("hold_back: ok")
