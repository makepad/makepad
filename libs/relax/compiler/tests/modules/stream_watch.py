"""Exercise watcher draining and delayed module diagnostics without a GUI."""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="module-stream-watch-") as tmp:
    pkg = pathlib.Path(tmp) / "pkg"
    shutil.copytree(root / "stream_watch", pkg, ignore=shutil.ignore_patterns("target*"))
    p = subprocess.run(
        [os.path.abspath(sys.argv[1]), "run", "--quiet", "module-stream-watch"],
        cwd=pkg,
        env=dict(os.environ, MAKEPAD_RUN_CACHE="0"),
        capture_output=True, text=True, timeout=120,
    )
lines = p.stderr.splitlines()
quiet = next((i for i, s in enumerate(lines) if s == "stream-watch: quiet window ended"), None)
errors = [i for i, s in enumerate(lines) if s.startswith("makepad live:") and "memory/watch-stream.rs:" in s]
ok = p.returncode == 0 and "out module stream watcher: true" in p.stdout and quiet is not None and errors and min(errors) > quiet
if not ok:
    print(p.stdout)
    print(p.stderr)
    sys.exit("modules stream watcher: FAIL")
print("modules stream watcher: PASS")
