#!/bin/bash
# Usage: bash run.sh /absolute/path/to/makepad
# Force the real lazy loader to defer a module used by a static's fn pointers.
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
        env = dict(os.environ, MAKEPAD_RUN_HOME=str(state),
                   MAKEPAD_RUN_CACHE_DIR=str(state / "cache"),
                   MAKEPAD_RUN_TARGET_DIR=str(state / "target"),
                   MAKEPAD_RUN_STD=str(fixture.parent.parent / "std"),
                   MAKEPAD_RUN_OFFLINE="1", MAKEPAD_RUN_CTFE=mode,
                   MAKEPAD_RUN_LAZY_DEFER="/payload.rs", MAKEPAD_RUN_DEFERRED_TRACE="1")
        for launch in ("cold", "warm"):
            result = subprocess.run([relax, "run", "ctfe-deferred"],
                                    cwd=fixture, env=env, text=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            output = result.stdout
            assert result.returncode == 0, f"CTFE={mode} {launch}: {output}"
            assert "ctfe_deferred: complete catalogue" in output, output
            assert "CTFE MISMATCH" not in output, output
            if launch == "cold":
                assert "makepad-run deferred: demand loaded 1 module(s)" in output, output
            print(f"ctfe_deferred: CTFE={mode} {launch} passed", flush=True)
PY
