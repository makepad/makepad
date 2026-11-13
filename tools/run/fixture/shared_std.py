#!/usr/bin/env python3
"""Two programs on one cache share the std code they have in common: a build script's run and the
test of its package, which records the lints' uses as it loads.

usage: shared_std.py   (run from the workspace root; the JIT runs through `cargo run --profile host`;
                        RELAX_STD defaults to libs/relax/compiler/std)

The package's build script and its test call the same code, which goes through io::Error's
bitpacked representation (std/io/error/repr_bitpacked.rs: `decode_repr` and the closures its
callers pass it). Each process writes its cache trace (RELAX_CACHE_TRACE):
  1. `makepad test`, cold: the build script runs first (a run's load: it keeps no front images and
     parses std itself) and records its code; then the test's load parses std again, recording lint
     uses as it goes;
  2. `makepad test` after an edit of build.rs: the build script runs again, on the test's entries.
Each process, against what the processes before it recorded:
  - every std function it looks up that an earlier process recorded is a hit, never a miss (by
    name; a closure's name holds its expression id, so closures are compared with that id masked:
    the same closure under another id is the case this guards);
  - no std entry fails to link;
  - no internal compiler error, no "outside the parsed tree", no "side table: no entry".
The test must hit repr_bitpacked's decode_repr instances and their closures, as the build script
recorded them.
Red: a load that records lint uses once numbered the file another way (the trailing-macro
look-ahead left its nodes in the tree: decode_repr's closures were expressions 771 and 802, a run's
420 and 451). The test then missed them and failed to link the callers it took from the build
script; before entries carried the file's numbering, a program decoded the other numbering's
`{closure#802}` (an internal compiler error in three programs in one day). And a data object (a
constant, a vtable) went to the store of the crate whose code named it first: the build script's,
which no other program opens, so the test failed to link core::fmt::write, fmt_u64 and four more
std entries that name it.
Prints `shared_std: ok` or `shared_std: FAIL ...`; exit 1 on failure."""
import os, re, shutil, subprocess, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", str(ROOT / "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.environ.setdefault("RELAX_STD", str(ROOT / "libs/relax/compiler/std"))
STD = {"std", "core", "alloc"}
BROKE = ("internal compiler error", "outside the parsed tree", "side table: no entry")
PROBE = '''pub fn probe() -> u32 {
    use std::io::{Error, ErrorKind};
    let custom = Error::new(ErrorKind::Other, "probe");
    let os = Error::from_raw_os_error(2);
    let simple = Error::from(ErrorKind::NotFound);
    let text = format!("{custom} {os:?} {simple}");
    matches!(custom.kind(), ErrorKind::Other) as u32 + matches!(simple.kind(), ErrorKind::NotFound) as u32 + text.len() as u32
}
'''


def masked(name):
    # a closure's expression id, in its own name and in the type of an instance it is an argument of
    name = re.sub(r"closure#\d+#[0-9a-f]+", "closure#_", name)
    return re.sub(r"(\.rs):\d+@", r"\1:_@", name)


def main():
    fails = []
    tmp = Path(tempfile.mkdtemp(prefix="shared-std-"))
    pkg = tmp / "pkg"
    (pkg / "src").mkdir(parents=True)
    (pkg / "Cargo.toml").write_text('[package]\nname = "shared-std-probe"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n\n[workspace]\n')
    (pkg / "src" / "probe.rs").write_text(PROBE)
    (pkg / "build.rs").write_text('include!("src/probe.rs");\nfn main() {\n    println!("cargo:rustc-env=PROBE_N={}", probe());\n}\n')
    (pkg / "src" / "lib.rs").write_text('include!("probe.rs");\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn same_as_the_build_script() {\n'
                                        '        assert_eq!(super::probe(), env!("PROBE_N").parse::<u32>().unwrap());\n    }\n}\n')
    env = dict(os.environ, RELAX_CACHE_DIR=str(tmp / "cache"), RELAX_HOME=str(tmp / "home"), TMPDIR=str(tmp),
               MAKEPAD_MUTE="1")
    procs = []
    for step in ["cold", "after a build.rs edit"]:
        if step != "cold":
            b = pkg / "build.rs"
            b.write_text(b.read_text() + "// edited: the build script runs again\n")
        d = tmp / ("traces-%d" % len(procs))
        d.mkdir()
        p = subprocess.run(JIT + ["test", "shared-std-probe"], cwd=pkg, env=dict(env, RELAX_CACHE_TRACE=str(d / "{pid}.tsv")),
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        if p.returncode != 0 or "test result: ok. 1 passed" not in p.stdout:
            fails.append("%s: the test did not pass (exit %d)" % (step, p.returncode))
        broke = [l for l in p.stdout.splitlines() if any(b in l for b in BROKE)]
        if broke:
            fails.append("%s: %s" % (step, broke[0][:300]))
        if fails:
            print("\n".join(p.stdout.splitlines()[-20:]))
            break
        for f in sorted(d.glob("*.tsv"), key=lambda f: f.stat().st_mtime):
            rows = [r for r in (l.split("\t") for l in f.read_text().splitlines()) if len(r) >= 8]
            build = any(r[3] == "build_script_build" for r in rows)
            procs.append(("%s %s %s" % (step, "build script" if build else "test", f.stem), build, rows))
    if not fails:
        recorded = set()
        builds = [b for _, b, _ in procs]
        if builds[:1] != [True] or builds.count(True) != 2:
            fails.append("want the build script's run first in each step: %s" % [l for l, _, _ in procs])
        # decode_repr's instances and their closures, as the cold build script recorded them
        want = None
        took = []
        for label, build, rows in procs:
            hit = {masked(r[5]) for r in rows if r[3] in STD and r[1] == "hit"}
            missed = sorted({r[5] for r in rows if r[3] in STD and r[1] == "miss" and masked(r[5]) in recorded and masked(r[5]) not in hit})
            linkfail = sorted({r[5] for r in rows if r[3] in STD and r[1] == "linkfail"})
            if missed:
                fails.append("%s: %d std functions an earlier program recorded missed: %s" % (label, len(missed), "; ".join(n[:140] for n in missed[:4])))
            if linkfail:
                fails.append("%s: %d std entries failed to link: %s" % (label, len(linkfail), "; ".join(n[:140] for n in linkfail[:4])))
            looked = {masked(r[5]) for r in rows if r[3] in STD and r[1] in ("hit", "miss")}
            if want and not build and label.startswith("cold") and want & looked:
                took.append((label, len(want & hit)))
            recorded |= {masked(r[5]) for r in rows if r[3] in STD and r[1].startswith("rec:")}
            if want is None and build:
                want = {n for n in recorded if "repr_bitpacked" in n and ("decode_repr" in n or "repr_bitpacked.rs:" in n)}
        if not want or len(want) < 4:
            fails.append("the build script recorded %d of decode_repr's instances and closures, want 4" % len(want or ()))
        elif not any(n == len(want) for _, n in took):
            fails.append("no test process hit all %d of decode_repr's instances and closures the build script recorded: %s" % (len(want), took))
    if fails:
        print("shared_std: FAIL " + " | ".join(fails))
        sys.exit(1)
    shutil.rmtree(tmp, ignore_errors=True)
    print("shared_std: ok")


main()
