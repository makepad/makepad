#!/usr/bin/env python3
"""Starts that alternate between two startup sets (a suite's tests, an app opened on one tab or
another) converge on one startup manifest and image, and then write neither again; after a
struct's layout changes, the slots the manifest carries for the other start get code for the new
layout.

usage: warm_image_union.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A program that presents is written to a fresh dir (its own RELAX_HOME). Each run calls a common
set of fns and one of two sets of its own (UNION_START=a or b, 200 slots each), presents
(`relax_first_present`), ends three frames (`relax_frame_done`) of which the first two call 200
new fns each (through a vtable) and the last none, then quits the way an app does (`relax_quit_started`: the quit
write records the startup manifest and its image) and ends. It runs a, b, a, b, a, b under
`makepad run --verbose`.
  - the first run is cold and writes the manifest and the image of start a;
  - the second, of start b, finds b's slots new and a's not called, and writes the union of both;
  - from the third on, each start is held by the loaded manifest and image: every run prelinks
    from the image and none writes the manifest or the image again.
  - from the second on, no fn the frames after the present call is linked on the main thread
    after the present (RELAX_POSTFRAME_LOG): the startup image holds them all.
Red before the union of recent startups (cache.rs record_manifest): every run rewrote both for
its own start, which the next run rewrote back. Red before the startup image's window stayed open
until a frame in which main reached no new code (postframe.rs image_window_open): it closed at the
frame after the present, and every start linked the second frame's 200 fns on main again.
Then the struct start a's slots read gains a field before the one they read, and it runs b, a:
  - b, the first start on the new source, records a's slots it did not run with the manifest it
    writes, stamped with the new sources;
  - a must run code made for the new layout. Red before a carried slot kept its key only when this
    run would trust it (cache.rs record_manifest, recorded_key): the slot kept the key of the
    earlier layout's code under the new stamp, the next start trusted it and ran code that read
    the field at its old offset (a wrong sum).
Every run's sum is checked.
Prints `warm_image_union: ok` or `warm_image_union: FAIL <why>`; exit 1 on failure.
WARM_UNION_TRACE=1 also prints each run's cache lines.
"""
import os, re, shutil, subprocess, sys, tempfile

ROOT = os.getcwd()
DIGITS = " ".join(str(n) for n in range(50))
MAIN = """use std::hint::black_box;

extern "C" {
    fn relax_first_present();
    fn relax_frame_done();
    fn relax_quit_started();
}

#[repr(C)]
struct Shape {
    %(pad)sb: u64,
}

#[inline(never)]
fn common<const N: usize>(_s: &Shape) -> u64 {
    black_box(N as u64 + 1)
}

#[inline(never)]
fn only_a<const N: usize>(s: &Shape) -> u64 {
    black_box(N as u64 * 3 + s.b)
}

#[inline(never)]
fn only_b<const N: usize>(_s: &Shape) -> u64 {
    black_box(N as u64 * 5)
}

#[inline(never)]
fn first_frame<const N: usize>(_s: &Shape) -> u64 {
    black_box(N as u64 * 7)
}

#[inline(never)]
fn next_frame<const N: usize>(_s: &Shape) -> u64 {
    black_box(N as u64 * 11)
}

// 200 slots of `f`: one per const argument
macro_rules! each {
    ($f:ident, $s:ident; $($n:literal)*) => { 0 $(+ $f::<{ $n }>($s) + $f::<{ 100 + $n }>($s) + $f::<{ 200 + $n }>($s) + $f::<{ 300 + $n }>($s))* };
}

#[inline(never)]
fn start_common(s: &Shape) -> u64 {
    each!(common, s; %(d)s)
}

#[inline(never)]
fn start_a(s: &Shape) -> u64 {
    each!(only_a, s; %(d)s)
}

#[inline(never)]
fn start_b(s: &Shape) -> u64 {
    each!(only_b, s; %(d)s)
}

// the frames after the present reach their code through a vtable, as an app's event handlers do:
// no call from a startup slot's code names it (the image takes such callees in by its calls)
trait Frame {
    fn run(&self, s: &Shape) -> u64;
}

struct One;
struct Two;

impl Frame for One {
    #[inline(never)]
    fn run(&self, s: &Shape) -> u64 {
        each!(first_frame, s; %(d)s)
    }
}

impl Frame for Two {
    #[inline(never)]
    fn run(&self, s: &Shape) -> u64 {
        each!(next_frame, s; %(d)s)
    }
}

fn main() {
    let a = std::env::var("UNION_START").map_or(false, |s| s == "a");
    let s = black_box(Shape { %(init)sb: B });
    let v = start_common(&s) + if a { start_a(&s) } else { start_b(&s) };
    let frames: [&dyn Frame; 2] = black_box([&One, &Two]);
    unsafe { relax_first_present() };
    // the present's frame ends; the next two reach new code, the last none
    unsafe { relax_frame_done() };
    let v = v + frames[0].run(&s);
    unsafe { relax_frame_done() };
    let v = v + frames[1].run(&s);
    unsafe { relax_frame_done() };
    unsafe { relax_frame_done() };
    println!("out {}", v);
    unsafe { relax_quit_started() };
}
"""
# the field start a's slots read, and the one the second layout puts before it (code made for the
# first layout reads it instead: a different sum)
B = 7
PAD = 1000
NS = [n + k for n in range(50) for k in (0, 100, 200, 300)]


def source(layout):
    pad, init = ("pad: u64,\n    ", "pad: %d, " % PAD) if layout else ("", "")
    return MAIN.replace("b: B", "b: %d" % B) % {"d": DIGITS, "pad": pad, "init": init}


def expected(start):
    return sum(n + 1 + n * 7 + n * 11 for n in NS) + sum(n * 3 + B if start == "a" else n * 5 for n in NS)
CARGO = """[package]
name = "relax-warm-image-union"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]
"""
BROKE = ("panicked", "internal compiler error", "FATAL")
WROTE = ("startup image of ", "startup manifest rewritten")


def run(jit, pkg, home, start, frames_log):
    env = dict(os.environ, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"), RELAX_TARGET_DIR=os.path.join(home, "target"),
               MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", UNION_START=start, RELAX_POSTFRAME_LOG=frames_log)
    env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
    p = subprocess.run(jit + ["run", "--quiet", "--verbose", "relax-warm-image-union"], cwd=pkg, env=env, capture_output=True, text=True, errors="replace")
    err = p.stderr.splitlines()
    if os.environ.get("WARM_UNION_TRACE"):
        for l in err:
            if "relax cache: " in l and ("startup" in l or "prelinked" in l):
                print("  %s | %s" % (start, l[:240]), flush=True)
    return p, err


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    open(os.path.join(pkg, "src", "main.rs"), "w").write(source(0))
    for i, start in enumerate("ababab" + "ba"):
        if i == 6:
            open(os.path.join(pkg, "src", "main.rs"), "w").write(source(1))
        frames_log = os.path.join(work, "postframe.%d" % (i + 1))
        p, err = run(jit, pkg, home, start, frames_log)
        broke = next((l.strip()[:200] for l in err if any(b in l for b in BROKE)), None)
        out = next((l for l in p.stdout.splitlines() if l.startswith("out ")), None)
        if p.returncode != 0 or broke or not out:
            return "run %d (start %s): %s" % (i + 1, start, broke or "exit %s (%s)" % (p.returncode, err[-1][:160] if err else "no output"))
        if out != "out %d" % expected(start):
            return "run %d (start %s%s): %s, expected out %d" % (i + 1, start, ", new layout" if i >= 6 else "", out, expected(start))
        if i >= 6:
            continue
        if i >= 1:
            linked = [l for l in (open(frames_log, errors="replace").read().splitlines() if os.path.exists(frames_log) else []) if "first_frame" in l or "next_frame" in l]
            if linked:
                return "run %d (start %s): %d fns of the frames after the present linked on main after it, not prelinked (first: %s)" % (i + 1, start, len(linked), linked[0][:160])
        wrote = [w.strip() for w in WROTE if any(w in l for l in err)]
        if i == 0 and "startup image of" not in wrote:
            return "run 1 (start a): no startup image was written"
        if i >= 1:
            m = next((re.search(r"prelinked (\d+) startup slots", l) for l in err if "prelinked " in l and " startup slots" in l), None)
            if not m or int(m.group(1)) == 0:
                return "run %d (start %s): nothing was prelinked from the startup image" % (i + 1, start)
        if i >= 2 and wrote:
            return "run %d (start %s): wrote again past the union of both starts (%s)" % (i + 1, start, "; ".join(wrote))
    return None


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="warm-image-union-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("warm_image_union: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("warm_image_union: ok")
