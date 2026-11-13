#!/usr/bin/env python3
"""`makepad test` prints the warnings of the code under test, as `cargo test` does: run.py
[/path/to/makepad] (without one, from the workspace root: `cargo run --profile host` of makepad).

src/t.rs's test has an unused import, an unused `let` and an unused `Result`: `cargo test` prints
3 warnings (the JIT printed none), and none for src/a.rs, whose imports its fn's body and signature
use (a test run once called them unused: it recorded no import uses).
1. A first run prints the 3.
2. An unchanged rerun (its check is stored whole) prints them again, as cargo replays a fresh
   crate's warnings.
3. A run after an edit elsewhere in the crate checks it again and prints them.
4. `makepad test --live`: the first verdict's check prints them, and a save that adds an unused
   `let` to the test prints the test's warnings with its patch line, the new one among them.
5. With the warnings fixed, a run prints "relax test: 0 warnings" (a clean run reads differently
   from one that did not check), and so does its unchanged rerun (the stored clean verdict).
6. `makepad test <file.rs>` (a single file, as `rustc --test`): an import its code uses is not
   reported (that form once recorded no import uses and called every import unused), one it
   does not use is.
7. A package of several test targets, each with an import that only a fn's `use Matrix as M`
   reads, and one also with an import nothing reads: cold, unchanged and after an edit of the lib,
   only that one is reported. (Each target's process loaded only the graph's root again and took
   the others from the collected image, whose import resolution records no uses: every target but
   the root called those imports unused.)
8. A test target whose imports of a dependency's macros (one direct, one through a re-export)
   only a `#[test]` fn's macro calls read, and an import nothing reads: only that one is reported.
   (The lint's scopes left `#[test]` fns out in a test build too: an app's
   `use ..::script_eval;`, read only by a test's `script_eval!(..)`, was called unused.)
Prints "test_warnings: ok" or "test_warnings: FAILED (...)"."""
import os
from pathlib import Path
import shutil
import signal
import subprocess as sp
import sys
import tempfile
import time

if len(sys.argv) > 1:
    jit = [str(Path(sys.argv[1]).resolve())]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", str(Path.cwd() / "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
here = Path(__file__).resolve().parent
WANT = ["warning: unused import: `std::collections::HashMap`", "warning: unused variable: `unused`",
         "warning: unused `Result` that must be used", "relax test: 3 warnings"]


def fail(why, out=""):
    print("test_warnings: FAILED (%s)" % why)
    print("\n".join(out.splitlines()[-30:]))
    sys.exit(1)


def missing(out, want):
    return [w for w in want if not any(w in line for line in out.splitlines())]


with tempfile.TemporaryDirectory(prefix="test-warnings-") as tmp:
    pkg = Path(tmp) / "pkg"
    pkg.mkdir()
    shutil.copy(here / "Cargo.toml", pkg / "Cargo.toml")
    shutil.copytree(here / "src", pkg / "src")
    env = dict(os.environ, RELAX_CACHE_DIR=str(Path(tmp) / "cache"))

    def run(step):
        p = sp.run(jit + ["test", "test-warnings"], cwd=pkg, env=env, stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
        out = p.stdout
        if p.returncode != 0 or "test result: ok. 1 passed" not in out:
            fail("%s: the test did not pass" % step, out)
        if missing(out, WANT):
            fail("%s: no %s" % (step, ", ".join(missing(out, WANT))), out)
        n = sum(1 for line in out.splitlines() if ": warning: " in line)
        if n != 3:
            fail("%s: %d warnings, want 3" % (step, n), out)

    run("first run")
    run("unchanged rerun")
    lib = pkg / "src" / "lib.rs"
    lib.write_text(lib.read_text().replace("x / 2", "x >> 1"))
    run("run after an edit of lib.rs")

    log = Path(tmp) / "live.log"
    with open(log, "w") as f:
        p = sp.Popen(jit + ["test", "--live", "test-warnings"], cwd=pkg, env=env, stdout=f, stderr=sp.STDOUT, start_new_session=True)
    try:
        def wait(cond, secs):
            end = time.time() + secs
            while time.time() < end:
                if cond(log.read_text(errors="replace")):
                    return True
                if p.poll() is not None:
                    return cond(log.read_text(errors="replace"))
                time.sleep(0.2)
            return False
        if not wait(lambda out: any(line.startswith("testverdict group=0") for line in out.splitlines()), 300):
            fail("--live: no first verdict", log.read_text(errors="replace"))
        out = log.read_text(errors="replace")
        if missing(out, WANT):
            fail("--live: the first check printed no %s" % ", ".join(missing(out, WANT)), out)
        t = pkg / "src" / "t.rs"
        time.sleep(1.1)
        t.write_text(t.read_text().replace("    let unused = 3;\n", "    let unused = 3;\n    let unused2 = 4;\n"))
        if not wait(lambda out: "t.rs: patched" in out and "unused variable: `unused2`" in out, 60):
            fail("--live: the save's patch line came without the new `let`'s warning", log.read_text(errors="replace"))
        lines = log.read_text(errors="replace").splitlines()
        at = next(i for i, line in enumerate(lines) if "t.rs: patched" in line)
        gone = missing("\n".join(lines[at + 1:at + 6]), ["warning: unused variable: `unused2`", "warning: unused variable: `unused`", "warning: unused `Result` that must be used"])
        if gone:
            fail("--live: after the patch line, no %s" % ", ".join(gone), "\n".join(lines))
    finally:
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            p.wait(timeout=10)
        except sp.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()

    t = pkg / "src" / "t.rs"
    t.write_text("#[test]\nfn a() {\n    let _ = super::parse(\"4\");\n    assert_eq!(super::half(4), 2);\n}\n")
    for step in ["a clean run", "its unchanged rerun"]:
        p = sp.run(jit + ["test", "test-warnings"], cwd=pkg, env=env, stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
        out = p.stdout
        if p.returncode != 0 or "test result: ok. 1 passed" not in out:
            fail("%s: the test did not pass" % step, out)
        if not any("relax test: 0 warnings" in line for line in out.splitlines()):
            fail("%s: no \"relax test: 0 warnings\" line" % step, out)
        if any(": warning: " in line for line in out.splitlines()):
            fail("%s: a warning printed" % step, out)
    one = Path(tmp) / "one.rs"
    one.write_text("use std::hint::black_box as bb;\nuse std::collections::BTreeMap;\n\n#[test]\nfn a() {\n    assert_eq!(bb(2) + 2, 4);\n}\n")
    p = sp.run(jit + ["test", str(one)], cwd=tmp, env=env, stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
    out = p.stdout
    if p.returncode != 0 or "test result: 1 passed; 0 failed" not in out:
        fail("a single file: the test did not pass", out)
    if missing(out, ["warning: unused import: `std::collections::BTreeMap`", "relax test: 1 warning"]):
        fail("a single file: no %s" % ", ".join(missing(out, ["warning: unused import: `std::collections::BTreeMap`", "relax test: 1 warning"])), out)
    if any("black_box" in line and "warning" in line for line in out.splitlines()):
        fail("a single file: its used import reported unused", out)

    multi = Path(tmp) / "multi"
    (multi / "src").mkdir(parents=True)
    (multi / "tests").mkdir()
    (multi / "Cargo.toml").write_text('[package]\nname = "test-warnings-multi"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
    (multi / "src" / "lib.rs").write_text("pub mod video {\n    pub enum Matrix { A, B }\n    pub enum Range { Full, Limited }\n"
                                          "    pub fn g(m: Matrix, r: Range) -> u32 { m as u32 * 2 + r as u32 }\n}\n")
    target = ("use test_warnings_multi::video::{g, Matrix, Range};\n%s\nfn pick() -> u32 {\n    use Matrix as M;\n    use Range as R;\n"
              "    g(M::B, R::Full)\n}\n\n#[test]\nfn t() {\n    assert_eq!(pick(), 2);\n}\n")
    for name, extra in [("a", ""), ("b", "use std::collections::BTreeSet;"), ("c", "")]:
        (multi / "tests" / ("%s.rs" % name)).write_text(target % extra)
    want = ["warning: unused import: `std::collections::BTreeSet`"]
    for step in ["several targets: a first run", "several targets: its unchanged rerun", "several targets: a run after an edit of lib.rs"]:
        if step.endswith("lib.rs"):
            lib = multi / "src" / "lib.rs"
            lib.write_text(lib.read_text().replace("m as u32 * 2", "(m as u32) << 1"))
        p = sp.run(jit + ["test", "test-warnings-multi"], cwd=multi, env=env, stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
        out = p.stdout
        if p.returncode != 0 or sum(1 for line in out.splitlines() if "test result: ok. 1 passed" in line) != 3:
            fail("%s: the 3 targets' tests did not pass" % step, out)
        warnings = [line for line in out.splitlines() if ": warning: " in line]
        if missing(out, want) or len(warnings) != 1:
            fail("%s: %d warnings, want only %s" % (step, len(warnings), want[0]), out)

    mac = Path(tmp) / "mac"
    (mac / "src").mkdir(parents=True)
    (mac / "tests").mkdir()
    (mac / "Cargo.toml").write_text('[package]\nname = "test-warnings-mac"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
    (mac / "src" / "lib.rs").write_text("#[macro_export]\nmacro_rules! twice {\n    ($e:expr) => { $e * 2 };\n}\n\n"
                                        "#[macro_export]\nmacro_rules! thrice {\n    ($e:expr) => { $e * 3 };\n}\n\n"
                                        "pub mod kit {\n    pub use crate::thrice;\n}\n")
    (mac / "tests" / "m.rs").write_text("use std::collections::BTreeSet;\nuse test_warnings_mac::kit::thrice;\nuse test_warnings_mac::twice;\n\n"
                                        "#[test]\nfn t() {\n    let f = |x: u32| twice!(x) + thrice!(x);\n    assert_eq!(f(1), 5);\n}\n")
    p = sp.run(jit + ["test", "test-warnings-mac"], cwd=mac, env=env, stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
    out = p.stdout
    if p.returncode != 0 or "test result: ok. 1 passed" not in out:
        fail("macros read by a test: its test did not pass", out)
    warnings = [line for line in out.splitlines() if ": warning: " in line]
    if missing(out, ["warning: unused import: `std::collections::BTreeSet`"]) or len(warnings) != 1:
        fail("macros read by a test: %d warnings, want only BTreeSet's" % len(warnings), out)
print("test_warnings: ok")
