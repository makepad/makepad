"""A module's load links std's twins from the disk cache: the warm_cache package twice on one
fresh cache. The second load links them, and the module's own code turns hot and reaches O3 as
in the first run, taking in no fewer callees."""
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parent
runs = []
with tempfile.TemporaryDirectory(prefix="module-warm-cache-") as tmp:
    pkg = pathlib.Path(tmp) / "pkg"
    shutil.copytree(root / "warm_cache", pkg, ignore=shutil.ignore_patterns("target*"))
    for _ in range(2):
        runs.append(subprocess.run(
            [os.path.abspath(sys.argv[1]), "run", "--quiet", "module-warm-cache"],
            cwd=pkg,
            env=dict(os.environ, RELAX_MODULE_TRACE="1", RELAX_O3="1", RELAX_O3_CALLS="100", RELAX_O3_TRACE="1"),
            capture_output=True, text=True,
        ))


def linked(p):
    m = re.search(r"^relax module: warm loaded .*, (\d+) linked from the disk cache\)$", p.stderr, re.M)
    return int(m.group(1)) if m else None


def inlined(p):
    n = [int(x) for x in re.findall(r"^tier1: sandbox:warm::mix installed \(.*?, (\d+) callees inlined", p.stdout + p.stderr, re.M)]
    return max(n) if n else None


try:
    for k, p in enumerate(runs):
        assert p.returncode == 0, f"run {k + 1}: exit {p.returncode}"
        out = [l for l in p.stdout.splitlines() if l.startswith("out the module")]
        assert out == ["out the module, before the tier-up: true", "out the module, after it: true"], f"run {k + 1}: {out}"
    host = [[l for l in p.stdout.splitlines() if l.startswith("out as host code")] for p in runs]
    assert host[0] == host[1] and len(host[0]) == 1, f"host results {host}"
    assert linked(runs[0]) == 0, f"the first run linked {linked(runs[0])} twins from a fresh cache"
    assert (linked(runs[1]) or 0) >= 50, f"the second run linked {linked(runs[1])} twins"
    cold, warm = inlined(runs[0]), inlined(runs[1])
    assert cold is not None and warm is not None and warm >= cold, f"`mix` at O3 took in {cold} callees, on the warm cache {warm}"
    print(f"modules warm cache: PASS ({linked(runs[1])} twins linked, mix at O3 in both runs)")
except AssertionError as error:
    for p in runs:
        print(p.stdout[-2000:])
        print(p.stderr[-4000:])
    sys.exit(f"modules warm cache: FAIL ({error})")
