#!/usr/bin/env python3
"""The terminal status keeps ticking while the cold app waits for first present."""
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
    (root / "Cargo.toml").write_text('[package]\nname="makepad-platform"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
    (root / "src/main.rs").write_text('''
fn main() {
    eprintln!("progress fixture ready");
    while !std::path::Path::new("done").exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
''')
    master, slave = pty.openpty()
    env = dict(os.environ, MAKEPAD_RUN_CACHE="off")
    for key in ["NO_COLOR", "MAKEPAD_RUN_REPORT_TO", "MAKEPAD_RUN_SCRIPT_DEPTH"]:
        env.pop(key, None)
    p = subprocess.Popen([binary, "run", "makepad-platform"], cwd=root, env=env, stdout=slave, stderr=slave)
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
        ticks = re.findall(r"compiling[^\r\n]*? · (\d+\.\d+) s", text)
        assert len(ticks) >= 12 and float(ticks[-1]) - float(ticks[0]) >= 2.5, text
        assert "queued" in text and "active" in text, text
        (root / "done").touch()
        deadline = time.monotonic() + 30
        while p.poll() is None:
            assert time.monotonic() < deadline, data.decode(errors="replace")
            drain()
        assert p.returncode == 0, data.decode(errors="replace")
        print("PASS progress: %d redraws over 3 seconds; elapsed, compiled, queued and active fields" % len(ticks))
    finally:
        if p.poll() is None:
            p.kill()
            p.wait()
        os.close(master)
