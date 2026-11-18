#!/usr/bin/env python3
"""A struct that gains a private field never meets code built for its old layout through the slots
a startup manifest carried over from an earlier start.

usage: warm_manifest_layout.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A two-crate program is written to a fresh dir (its own RELAX_HOME). The dependency has a generic
queue whose closures move its jobs by value; the app's start a queues its own `Option<Engine>`
there and reads Engine's fields, start b queues numbers and only builds an Engine through `make`.
Each run presents (`relax_first_present`) and quits the way an app does (`relax_quit_started`: the
quit write records the startup manifest and its image). Three runs on one cache:
  1. start a on the first text: the manifest holds a's slots, keyed from that text;
  2. Engine gains a private field ahead of the ones a reads; start b on the new text keys its own
     slots fresh (`make` among them) and writes the union of both starts, stamped with this text;
  3. start a on the new text: `make` is the new layout's, so a's other slots (start_a, weigh, the
     dependency's closures over Option<Engine>) must be built for it too. A manifest that carried
     a's keys from run 1 under run 2's stamp hands back their old code: it moves and reads
     Engine at the old size and offsets (11-17: an app struct's new field met a strand's
     schedule closures built for the old one).
Runs 1 and 3 print what start a computes; run 3 must also have placed code from the startup image,
so the warm path is the one checked.
Prints `warm_manifest_layout: ok` or `warm_manifest_layout: FAIL <why>`; exit 1 on failure.
WARM_LAYOUT_TRACE=1 also prints each run's startup cache lines.
"""
import os, re, shutil, subprocess, sys, tempfile

ROOT = os.getcwd()
DEP = """pub struct Queue<T> {
    jobs: Vec<(u64, T)>,
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue { jobs: Vec::new() }
    }

    // the front job leaves the queue through closures that move it, and the caller's takes it
    #[inline(never)]
    pub fn run_front<R>(&mut self, mut f: impl FnMut(u64, T) -> R) -> Option<R> {
        let at = self.jobs.iter().map(|(k, _)| *k).min()?;
        let i = self.jobs.iter().position(|(k, _)| *k == at)?;
        let job = Some(self.jobs.remove(i)).map(|(k, job)| (k + 1, job));
        job.map(|(k, job)| f(k, job))
    }

    #[inline(never)]
    pub fn push(&mut self, k: u64, job: T) {
        self.jobs.push((k, job));
    }
}
"""
APP = """use std::hint::black_box;
use layout_dep::Queue;

extern "C" {
    fn relax_first_present();
    fn relax_quit_started();
}

pub struct Engine {
    id: u64,
%(field)s    name: String,
    steps: Vec<u32>,
}

#[inline(never)]
fn make(id: u64) -> Option<Engine> {
    Some(Engine { id, %(init)sname: format!("engine{}", id), steps: (0..id as u32).collect() })
}

#[inline(never)]
fn weigh(e: &Engine) -> u64 {
    e.id * 1000 + e.name.len() as u64 * 100 + e.steps.iter().map(|s| *s as u64).sum::<u64>()
}

#[inline(never)]
fn start_a() -> u64 {
    let mut q: Queue<Option<Engine>> = Queue::new();
    for id in [5u64, 3, 9] {
        q.push(id, make(black_box(id)));
    }
    let mut total = 0;
    while let Some(w) = q.run_front(|k, job| job.map_or(0, |e| k + weigh(&e))) {
        total += w;
    }
    total
}

#[inline(never)]
fn start_b() -> u64 {
    let mut q: Queue<u64> = Queue::new();
    for id in [2u64, 7] {
        q.push(id, id * 11);
    }
    let mut total = make(black_box(4)).map_or(0, |e| e.id * 7);
    while let Some(w) = q.run_front(|k, v| k + v) {
        total += w;
    }
    total
}

fn main() {
    let a = std::env::var("UNION_START").map_or(false, |s| s == "a");
    let v = if a { start_a() } else { start_b() };
    unsafe { relax_first_present() };
    println!("out {}", v);
    unsafe { relax_quit_started() };
}
"""
WORKSPACE = """[workspace]
members = ["app", "dep"]
resolver = "2"
"""
APP_CARGO = """[package]
name = "relax-warm-manifest-layout"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
layout_dep = { path = "../dep" }
"""
DEP_CARGO = """[package]
name = "layout_dep"
version = "0.1.0"
edition = "2021"
publish = false
"""
BROKE = ("panicked", "internal compiler error", "FATAL", "POISONED")


def want(start):
    """What start a or b prints, whatever Engine's layout: the queue pops by key, each job's key
    one up."""
    if start == "a":
        return sum(k + 1 + k * 1000 + len("engine%d" % k) * 100 + sum(range(k)) for k in (3, 5, 9))
    return 4 * 7 + sum(k + 1 + k * 11 for k in (2, 7))


def run(jit, pkg, home, start):
    env = dict(os.environ, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"), RELAX_TARGET_DIR=os.path.join(home, "target"),
               MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", UNION_START=start)
    env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
    p = subprocess.run(jit + ["run", "--quiet", "--verbose", "relax-warm-manifest-layout"], cwd=os.path.join(pkg, "app"), env=env,
                       capture_output=True, text=True, errors="replace")
    err = p.stderr.splitlines()
    if os.environ.get("WARM_LAYOUT_TRACE"):
        for l in err:
            if "relax cache: " in l and ("startup" in l or "prelinked" in l):
                print("  %s | %s" % (start, l[:240]), flush=True)
    return p, err


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    for d in ("app", "dep"):
        os.makedirs(os.path.join(pkg, d, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(WORKSPACE)
    open(os.path.join(pkg, "app", "Cargo.toml"), "w").write(APP_CARGO)
    open(os.path.join(pkg, "dep", "Cargo.toml"), "w").write(DEP_CARGO)
    open(os.path.join(pkg, "dep", "src", "lib.rs"), "w").write(DEP)
    for i, (start, edited) in enumerate([("a", False), ("b", True), ("a", True)]):
        text = APP % {"field": "    pad: [u64; 3],\n" if edited else "", "init": "pad: [1, 2, 3], " if edited else ""}
        open(os.path.join(pkg, "app", "src", "main.rs"), "w").write(text)
        p, err = run(jit, pkg, home, start)
        what = "run %d (start %s%s)" % (i + 1, start, ", Engine edited" if edited else "")
        broke = next((l.strip()[:200] for l in err if any(b in l for b in BROKE)), None)
        out = next((l for l in p.stdout.splitlines() if l.startswith("out ")), None)
        if p.returncode != 0 or broke or out is None:
            return "%s: %s" % (what, broke or "exit %s (%s)" % (p.returncode, err[-1][:160] if err else "no output"))
        if out != "out %d" % want(start):
            return "%s printed `%s`, not `out %d`: code built for Engine's other layout" % (what, out, want(start))
        if i == 2:
            m = next((re.search(r"prelinked (\d+) startup slots", l) for l in err if "prelinked " in l and " startup slots" in l), None)
            if not m or int(m.group(1)) == 0:
                return "%s: nothing was prelinked from the startup image, so the warm path went unchecked" % what
    return None


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="warm-manifest-layout-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("warm_manifest_layout: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("warm_manifest_layout: ok")
