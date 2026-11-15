#!/usr/bin/env python3
"""A live test session answers each test's outcome, not only counts and failing names: run.py
[/path/to/relax] (without one, from the workspace root: relax-runner's relax, built by cargo).
Relax issue 131 (ported from makepad/relax PR 134).

`makepad test --live live-test-results --lib -- --skip filtered`, found with `inspect list`:
1. The first verdict names its query, `test-results group=<g>`; the query answers the group and
   test count, then each test: both plain tests passed and ran, `ignored` ignored, `filtered`
   filtered (neither ran).
2. An edit of value() fails value_is_one: the next verdict's query has it failed and run, and
   `independent`, which the edit does not reach, passed from the earlier run (ran false).
3. The first verdict's group is refused now: its results are gone, the error names the group kept.
4. An edit whose patched value() faults (a read of an unmapped address): its verdict says
   `faulted` with its group, the JIT undoes it, and the run after that says `reverted` with the
   group, in its verdict and its results' header: it tested the code from before the edit, which
   is no verdict on the saved source (a session read 77/77 after a revert as one, 11-14).
Prints "live_test_results: ok" or "live_test_results: FAILED (...)"."""
import json
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
    # built once, here: a `cargo run` per call ran in the test's package, outside the workspace,
    # without its .cargo/config.toml, so the first call built relax again with other flags (100 s)
    sp.run(["cargo", "build", "--profile", "host", "-q", "-p", "relax-runner", "--bin", "relax"], check=True)
    jit = [str((Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "host" / "relax").resolve())]
here = Path(__file__).resolve().parent


def fail(why, log=None):
    print("live_test_results: FAILED (%s)" % why)
    if log is not None and log.exists():
        print("\n".join(log.read_text(errors="replace").splitlines()[-30:]))
    sys.exit(1)


with tempfile.TemporaryDirectory(prefix="live-test-results-") as tmp:
    root = Path(tmp)
    pkg = root / "pkg"
    pkg.mkdir()
    shutil.copy(here / "Cargo.toml", pkg / "Cargo.toml")
    shutil.copytree(here / "src", pkg / "src")
    registry = root / "registry"
    registry.mkdir()
    # its own discovery registry: `inspect list` names this session only
    env = dict(os.environ, RELAX_CACHE_DIR=str(root / "cache"), MAKEPAD_REMOTE_REGISTRY=str(registry), RELAX_PANIC_PARK="0")
    log = root / "live.log"
    with open(log, "w") as f:
        p = sp.Popen(jit + ["test", "--live", "live-test-results", "--lib", "--", "--skip", "filtered"], cwd=pkg, env=env,
                     stdout=f, stderr=sp.STDOUT, start_new_session=True)
    try:
        def inspect(*words, check=True):
            r = sp.run(jit + ["inspect", *words, "--json"], cwd=pkg, env=env, stdout=sp.PIPE, stderr=sp.STDOUT, text=True)
            rows = [json.loads(line) for line in r.stdout.splitlines() if line.startswith("{")]
            if check and r.returncode != 0:
                fail("inspect %s: exit %d: %s" % (" ".join(words), r.returncode, r.stdout.strip()), log)
            return rows

        pid = None
        while pid is None:
            if p.poll() is not None:
                fail("the session ended before it could be inspected", log)
            apps = [a for a in inspect("list") if a.get("app") == "live-test-results"]
            if apps:
                pid = str(apps[0]["pid"])
            else:
                time.sleep(0.2)

        def verdict(since):
            for e in inspect(pid, "watch", "on=testverdict", "since=%d" % since):
                if e.get("event") == "testverdict":
                    return e
            fail("no testverdict after seq %d" % since, log)

        def results(v, step):
            if v.get("results") != "test-results group=%d" % v["group"]:
                fail("%s: the verdict names no results query: %s" % (step, v), log)
            rows = inspect(pid, *v["results"].split())
            head = [r for r in rows if "test_count" in r]
            tests = {r["name"]: (r["status"], r["ran"]) for r in rows if "name" in r}
            if head != [{"group": v["group"], "test_count": 4}] or len(tests) != 4:
                fail("%s: header %s, %d rows: %s" % (step, head, len(tests), rows), log)
            return tests

        first = verdict(0)
        got = results(first, "first verdict")
        want = {"value_is_one": ("passed", True), "independent": ("passed", True), "ignored": ("ignored", False), "filtered": ("filtered", False)}
        if got != want:
            fail("first verdict: %s, want %s" % (got, want), log)
        lib = pkg / "src" / "lib.rs"
        time.sleep(1.1)
        lib.write_text(lib.read_text().replace("{ 1 }", "{ 2 }"))
        edited = verdict(first["seq"])
        if edited["group"] <= first["group"] or edited.get("failures") != ["value_is_one"]:
            fail("the edit's verdict: %s" % edited, log)
        got = results(edited, "the edit's verdict")
        want = {"value_is_one": ("failed", True), "independent": ("passed", False), "ignored": ("ignored", False), "filtered": ("filtered", False)}
        if got != want:
            fail("the edit's verdict: %s, want %s" % (got, want), log)
        stale = inspect(pid, "test-results", "group=%d" % first["group"], check=False)
        if any("name" in r for r in stale) or not any(("group %d" % edited["group"]) in r.get("err", "") for r in stale):
            fail("the first group's results after the edit: %s" % stale, log)
        time.sleep(1.1)
        lib.write_text(lib.read_text().replace("{ 2 }", "{ unsafe { (16usize as *const u32).read_volatile() } }"))
        faulted = verdict(edited["seq"])
        g = faulted["group"]
        if g <= edited["group"] or faulted.get("faulted") != [g] or "reverted" in faulted:
            fail("the faulting edit's verdict: %s" % faulted, log)
        undone = verdict(faulted["seq"])
        if undone["group"] != g or undone.get("reverted") != [g] or "faulted" in undone:
            fail("the verdict after the revert: %s" % undone, log)
        head = [r for r in inspect(pid, *undone["results"].split()) if "test_count" in r]
        if head != [{"group": g, "test_count": 4, "reverted": [g]}]:
            fail("the results after the revert: %s" % head, log)
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
print("live_test_results: ok")
