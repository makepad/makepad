#!/usr/bin/env python3
"""A panic the program catches has its report line tagged: `  report (caught): {..}`. One that
ends a thread or a test keeps `  report: {..}`, and a failed unit's summary (land_gate.sh's
test_failures, which reads `  report: ` lines) and jit_misses take only those.

usage: caught_report.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A package is written to a fresh dir (its own RELAX_HOME):
  1. `makepad run`: main catches a panic with catch_unwind ("caught on purpose": tagged), and a
     thread it joins panics ("a thread ends": not tagged);
  2. `makepad test`: one test catches a panic ("caught in a test": tagged), a #[should_panic] test
     panics as it expects ("expected by the test": tagged), and one test fails ("fails the
     test": not tagged).
Red without the tag (jit.rs relax_catch_caught): every report says `report:`, so a summary takes
the deliberate panics for failures (a bucket's relax-compiler summary led with three, 11-07).
Prints `caught_report: ok` or `caught_report: FAIL <why>`; exit 1 on failure.
"""
import os, re, shutil, subprocess, sys, tempfile

ROOT = os.getcwd()
MAIN = """fn main() {
    let r = std::panic::catch_unwind(|| panic!("caught on purpose"));
    assert!(r.is_err());
    let h = std::thread::spawn(|| panic!("a thread ends"));
    assert!(h.join().is_err());
    println!("main done");
}

#[cfg(test)]
mod tests {
    #[test]
    fn catches() {
        assert!(std::panic::catch_unwind(|| panic!("caught in a test")).is_err());
    }

    #[test]
    #[should_panic(expected = "expected by the test")]
    fn expects() {
        panic!("expected by the test");
    }

    #[test]
    fn fails() {
        panic!("fails the test");
    }
}
"""
CARGO = """[package]
name = "relax-caught-report"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]
"""


def reports(text):
    """(tagged, message) of each report line."""
    out = []
    for line in text.splitlines():
        m = re.match(r"^  report( \(caught\))?: (\{.*)$", line)
        if m:
            msg = re.search(r'"message":"([^"]*)"', m.group(2))
            out.append((m.group(1) is not None, msg.group(1) if msg else ""))
    return out


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    open(os.path.join(pkg, "src", "main.rs"), "w").write(MAIN)
    env = dict(os.environ, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"),
               RELAX_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1")
    env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
    # 1. a run
    p = subprocess.run(jit + ["run", "--quiet", "relax-caught-report"], cwd=pkg, env=env, capture_output=True, text=True, errors="replace")
    if "main done" not in p.stdout:
        return "the run did not finish (exit %s): %s" % (p.returncode, p.stderr.strip()[-400:])
    got = reports(p.stderr)
    for want in [(True, "caught on purpose"), (False, "a thread ends")]:
        if want not in got:
            return "the run's reports %s lack %s" % (got, want)
    # 2. a test run
    log = os.path.join(work, "test.log")
    with open(log, "w") as f:
        p = subprocess.run(jit + ["test", "relax-caught-report"], cwd=pkg, env=env, stdout=f, stderr=subprocess.STDOUT, text=True)
    text = open(log, errors="replace").read()
    if p.returncode == 0 or "test tests::fails ... FAILED" not in text or "2 passed; 1 failed" not in text:
        return "the test run is not 2 passed, 1 failed (exit %s): %s" % (p.returncode, text.strip()[-400:])
    got = reports(text)
    for want in [(True, "caught in a test"), (True, "expected by the test"), (False, "fails the test")]:
        if want not in got:
            return "the test run's reports %s lack %s" % (got, want)
    return None


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="caught-report-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("caught_report: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("caught_report: ok")
