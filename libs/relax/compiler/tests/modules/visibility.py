"""Assert the actual reload body lists, including transitive crate visibility."""
import ast
import os
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent
p = subprocess.run(
    [os.path.abspath(sys.argv[1]), "test", str(root / "visibility.rs")],
    env=dict(os.environ, MAKEPAD_RUN_CACHE="0", MAKEPAD_RUN_RELOAD_TRACE="1"),
    capture_output=True, text=True, timeout=120,
)
try:
    assert p.returncode == 0, f"exit {p.returncode}"
    assert "out crate visibility: true" in p.stdout
    for phase, expected in [
        ("base", {"own_collision", "direct_collision", "transitive_collision"}),
        ("leaf", {"transitive_collision"}),
    ]:
        trace = p.stderr.split(f"visibility: {phase} begin\n", 1)[1].split(
            f"visibility: {phase} end\n", 1
        )[0]
        lists = re.findall(r"\(bodies (\[.*?\])\)", trace)
        assert len(lists) == 1, f"{phase}: expected one check phase, got {lists}"
        bodies = ast.literal_eval(lists[0])
        assert sorted(bodies) == sorted(expected), f"{phase}: expected {expected}, got {bodies}"
        print(f"modules visibility {phase}: PASS (bodies {bodies})")
except (AssertionError, IndexError) as error:
    print(p.stdout)
    print(p.stderr)
    sys.exit(f"modules visibility: FAIL ({error})")
