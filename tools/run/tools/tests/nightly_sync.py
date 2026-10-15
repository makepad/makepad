"""Targeted nightly_sync checks; run on node 165 with Python's standard library.

The fake git records read-only commands without creating or modifying a repository.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class NightlySync(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        checkout = self.root / "makepad tree"
        self.relax = checkout / "libs/relax"
        self.relax.mkdir(parents=True)
        self.script = checkout / "tools/run/tools/nightly_sync.sh"
        self.script.parent.mkdir(parents=True)
        shutil.copyfile(Path(__file__).parents[1] / "nightly_sync.sh", self.script)
        self.log = self.root / "git.log"
        fake_git = self.root / "git"
        fake_git.write_text('''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ["SYNC_GIT_LOG"], "a") as log:
    log.write(json.dumps(args) + "\\n")
if "rev-parse" in args:
    if args[-1].startswith("badbeef"):
        sys.exit(1)
    print("8d1a764" if args[-1] == "HEAD^{commit}" else "8bab26f")
elif "cat-file" in args:
    sys.exit(int(args[-1] == "8d1a764:compiler/gone.rs"))
elif "diff" in args:
    print("UPSTREAM DIFF " + args[-1])
    sys.exit(int(args[-1] == ":(literal)compiler/fail.rs"))
else:
    raise AssertionError(args)
''')
        fake_git.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.root) + os.pathsep + os.environ["PATH"],
                        SYNC_GIT_LOG=str(self.log))

    def run_sync(self, *args):
        return subprocess.run(["bash", str(self.script), *args], env=self.env,
                              text=True, capture_output=True)

    def port(self, name, headers):
        path = self.relax / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(headers)

    def test_multiple_sources_and_missing_path(self):
        self.port("src/a file.rs", "// ported from N:present.rs @8bab26f\n"
                  "// ported from compiler/gone.rs @8bab26f\n")
        result = self.run_sync(str(self.root / "nightly tree"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.count("=== src/a file.rs <-"), 2)
        self.assertIn("MISSING at 8d1a764: compiler/gone.rs", result.stdout)
        self.assertEqual(result.stdout.count("UPSTREAM DIFF"), 2)
        self.assertIn('"8bab26f..8d1a764", "--", ":(literal)compiler/present.rs"',
                      self.log.read_text())

    def test_no_pins_and_ignored_build_directory(self):
        self.port("target/generated.rs", "// ported from N:ignored.rs @8bab26f\n")
        result = self.run_sync(str(self.root))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("No port headers found.", result.stdout)

    def test_invalid_header_pin_and_path(self):
        for header in ("N:source.rs @not-a-sha", "N:source.rs @badbeef",
                       "../outside.rs @8bab26f", "/absolute.rs @8bab26f"):
            with self.subTest(header=header):
                self.port("src/a.rs", "// ported from " + header + "\n")
                result = self.run_sync(str(self.root))
                self.assertEqual(result.returncode, 1)
                self.assertNotIn("UPSTREAM DIFF", result.stdout)

    def test_diff_failure_is_reported(self):
        self.port("src/a.rs", "// ported from N:fail.rs @8bab26f\n")
        self.assertEqual(self.run_sync(str(self.root)).returncode, 1)

    def test_explicit_revision_and_usage(self):
        self.assertEqual(self.run_sync().returncode, 2)
        self.assertEqual(self.run_sync(str(self.root), "some-revision").returncode, 0)
        self.assertIn('"some-revision^{commit}"', self.log.read_text())


if __name__ == "__main__":
    unittest.main()
