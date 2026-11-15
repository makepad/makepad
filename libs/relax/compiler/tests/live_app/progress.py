#!/usr/bin/env python3
"""The terminal status keeps ticking while the cold app waits for first present, and an idle
compiler reads "running", not "compiling" (a program that never presents read as compiling forever)."""
import os
from pathlib import Path
import pty
import re
import select
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="relax-progress-") as tmp:
    root = Path(tmp)
    (root / "src").mkdir()
    (root / "Cargo.toml").write_text('[package]\nname="presents-progress"\nversion="0.0.0"\nedition="2021"\n[package.metadata.relax]\npresents = true\n[workspace]\n')
    (root / "src/main.rs").write_text('''
fn main() {
    eprintln!("progress fixture ready");
    while !std::path::Path::new("done").exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
''')
    master, slave = pty.openpty()
    env = dict(os.environ, RELAX_CACHE="off")
    for key in ["NO_COLOR", "RELAX_REPORT_TO", "RELAX_SCRIPT_DEPTH"]:
        env.pop(key, None)
    p = subprocess.Popen([binary, "run", "presents-progress"], cwd=root, env=env, stdout=slave, stderr=slave)
    os.close(slave)
    data = bytearray()
    def drain():
        if select.select([master], [], [], .02)[0]:
            try:
                data.extend(os.read(master, 65536))
            except OSError:
                pass
    try:
        deadline = time.monotonic() + 120
        while b"progress fixture ready" not in data:
            assert p.poll() is None and time.monotonic() < deadline, data.decode(errors="replace")
            drain()
        begin = len(data)
        start = time.monotonic()
        while time.monotonic() - start < 3:
            drain()
        text = re.sub(r"\x1b\[[0-9;]*m", "", data[begin:].decode(errors="replace"))
        ticks = re.findall(r"(?:compiling|running)[^\r\n]*? · (\d+\.\d+) s", text)
        assert len(ticks) >= 12 and float(ticks[-1]) - float(ticks[0]) >= 2.5, text
        # the fixture's main waits on a file: nothing compiles, and the last redraw says so
        assert re.search(r"running +\d+ fns compiled · \d+\.\d+ s", text.rsplit("\x1b[2K", 1)[-1]), text
        (root / "done").touch()
        deadline = time.monotonic() + 30
        while p.poll() is None:
            assert time.monotonic() < deadline, data.decode(errors="replace")
            drain()
        assert p.returncode == 0, data.decode(errors="replace")
        print("PASS progress: %d redraws over 3 seconds; an idle compiler reads running" % len(ticks))
    finally:
        if p.poll() is None:
            p.kill()
            p.wait()
        os.close(master)
