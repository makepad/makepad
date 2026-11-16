#!/usr/bin/env python3
"""A panic the program catches is the program's to handle: std's hook prints its message, as
rustc's does, and the JIT adds no report. One that ends a thread or a test keeps its
`  report: {..}` line, which a failed unit's summary (land_gate.sh's test_failures) and jit_misses
read.

usage: caught_report.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

A package is written to a fresh dir (its own RELAX_HOME):
  1. `makepad run`: main catches a panic with catch_unwind ("caught on purpose": its message, no
     report), and a thread it joins panics ("a thread ends": reported);
  2. `makepad test`: one test catches a panic ("caught in a test": no report), a #[should_panic]
     test panics as it expects ("expected by the test": no report), and one test fails ("fails the
     test": reported).
Red when a caught panic is reported: a suite whose tests catch panics on purpose filled its log
with reports rustc never prints (relax issue 231); before that, untagged, a summary took them for
failures (11-07).
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
    """The message of each report line (any `  report...: {..}` form)."""
    out = []
    for line in text.splitlines():
        m = re.match(r"^  report[^:]*: (\{.*)$", line)
        if m:
            msg = re.search(r'"message":"([^"]*)"', m.group(1))
            out.append(msg.group(1) if msg else "")
    return out


def compare(what, text, reported, quiet):
    """None when exactly the `reported` panics have a report and each `quiet` one printed std's
    message (`panicked at`) without one."""
    got = reports(text)
    for want in reported:
        if want not in got:
            return "%s: no report of %r (reports: %s)" % (what, want, got)
    for q in quiet:
        if q in got:
            return "%s: %r was caught, yet reported (reports: %s)" % (what, q, got)
        if not re.search(r"panicked at .*\n%s|panicked at .*%s" % (re.escape(q), re.escape(q)), text):
            return "%s: std's message of %r is missing" % (what, q)
    return None


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    open(os.path.join(pkg, "src", "main.rs"), "w").write(MAIN)
    env = dict(os.environ, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"),
               RELAX_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1")
    env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
    # 1. a run, unattended: its panics unwind at once (attended, as under devhub's observer, the first
    # parked for a decision nobody here makes, and the job paused, 11-14)
    p = subprocess.run(jit + ["run", "--quiet", "--unattended", "relax-caught-report"], cwd=pkg, env=env, capture_output=True, text=True, errors="replace")
    if "main done" not in p.stdout:
        return "the run did not finish (exit %s): %s" % (p.returncode, p.stderr.strip()[-400:])
    why = compare("the run", p.stderr, ["a thread ends"], ["caught on purpose"])
    if why:
        return why
    # 2. a test run
    log = os.path.join(work, "test.log")
    with open(log, "w") as f:
        p = subprocess.run(jit + ["test", "relax-caught-report"], cwd=pkg, env=env, stdout=f, stderr=subprocess.STDOUT, text=True)
    text = open(log, errors="replace").read()
    if p.returncode == 0 or "test tests::fails ... FAILED" not in text or "2 passed; 1 failed" not in text:
        return "the test run is not 2 passed, 1 failed (exit %s): %s" % (p.returncode, text.strip()[-400:])
    return compare("the test run", text, ["fails the test"], ["caught in a test", "expected by the test"])


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
