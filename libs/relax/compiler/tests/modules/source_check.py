"""The module source check under `relax run`: a file's fn bodies are parsed on demand there."""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="module-source-check-") as tmp:
    pkg = pathlib.Path(tmp) / "pkg"
    shutil.copytree(root / "source_check", pkg, ignore=shutil.ignore_patterns("target*"))
    p = subprocess.run(
        [os.path.abspath(sys.argv[1]), "run", "--quiet", "module-source-check"],
        cwd=pkg,
        env=dict(os.environ, RELAX_CACHE="0"),
        capture_output=True, text=True,
    )
got = "\n".join(s for s in p.stdout.splitlines() if s.startswith("out "))
want = (root / "expect-source-check.out").read_text().strip()
if p.returncode != 0 or got != want:
    print(got)
    print(p.stderr[-3000:])
    sys.exit("modules source check in a program: FAIL")
print("modules source check in a program: PASS")
