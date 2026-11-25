#!/bin/bash
# Usage: bash run.sh /absolute/path/to/makepad
# Force the real lazy loader to defer a module used by a static's fn pointers: a dependency's, outside the
# app's project tree (app/; the app's own crates, its tree's, are type-checked whole before main, which
# loads every file of theirs), with the background check off (a headless program's would load the
# module itself, before the static asks).
set -eu
python3 - "$1" "$(cd "$(dirname "$0")" && pwd)" <<'PY'
import os
from pathlib import Path
import subprocess
import sys
import tempfile

relax = str(Path(sys.argv[1]).resolve())
fixture = Path(sys.argv[2])
with tempfile.TemporaryDirectory(prefix="relax-ctfe-deferred-") as scratch:
    for mode in ("1", "0", "check"):
        state = Path(scratch) / mode
        env = dict(os.environ, RELAX_HOME=str(state),
                   RELAX_CACHE_DIR=str(state / "cache"),
                   RELAX_TARGET_DIR=str(state / "target"),
                   RELAX_STD=str(fixture.parent.parent / "std"),
                   RELAX_OFFLINE="1", RELAX_CTFE=mode,
                   RELAX_LAZY_DEFER="/payload.rs", RELAX_DEFERRED_TRACE="1",
                   RELAX_BG_TYPECK="0")
        for launch in ("cold", "warm"):
            result = subprocess.run([relax, "run", "ctfe-deferred"],
                                    cwd=fixture / "app", env=env, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            output = result.stdout
            assert result.returncode == 0, f"CTFE={mode} {launch}: {output}"
            assert "ctfe_deferred: complete catalogue" in output, output
            assert "CTFE MISMATCH" not in output, output
            if launch == "cold":
                assert "relax deferred: demand loaded 1 module(s)" in output, output
            print(f"ctfe_deferred: CTFE={mode} {launch} passed", flush=True)
PY
