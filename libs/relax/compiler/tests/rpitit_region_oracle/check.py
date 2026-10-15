# Targeted rustc oracle; invoke only through nbuild.sh on node 165.
import pathlib
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parent
print(subprocess.check_output(["rustc", "--version"], text=True).strip())
with tempfile.TemporaryDirectory(prefix="arch-n01-rpitit-") as out:
    for name, success in [("valid", True), ("invalid", False)]:
        run = subprocess.run(["rustc", "--edition=2021", "--crate-type=lib", "--emit=metadata", str(root / (name + ".rs")), "-o", str(pathlib.Path(out) / (name + ".rmeta"))], text=True, capture_output=True)
        print(f"rpitit-region-oracle {name}: exit {run.returncode}")
        print(run.stderr.strip())
        assert (run.returncode == 0) == success
        if not success:
            assert "E0477" in run.stderr or "lifetime may not live long enough" in run.stderr
