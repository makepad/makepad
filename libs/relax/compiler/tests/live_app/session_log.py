#!/usr/bin/env python3
"""A run transcript survives abnormal exits and is deleted only after a clean exit."""
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="relax-session-log-") as tmp:
    root = Path(tmp)
    (root / "src").mkdir()
    (root / "Cargo.toml").write_text('[package]\nname="session-log"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
    (root / "src/main.rs").write_text('''
fn main() {
    eprintln!("session-log fixture ready");
    while !std::path::Path::new("go").exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "panic" => panic!("session-log fixture panic"),
        "exit" => std::process::exit(0),
        "error" => std::process::exit(7),
        _ => (),
    }
}
''')
    for mode in ["return", "exit", "error", "panic", "term", "int", "kill"]:
        (root / "go").unlink(missing_ok=True)
        output = root / (mode + ".log")
        with output.open("w") as log:
            p = subprocess.Popen([binary, "run", "session-log", "--", mode], cwd=root,
                                 env=dict(os.environ, MAKEPAD_RUN_CACHE="off"), stdout=log, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 120
                while "session-log fixture ready" not in output.read_text(errors="replace"):
                    assert p.poll() is None and time.monotonic() < deadline, output.read_text(errors="replace")
                    time.sleep(.05)
                text = output.read_text(errors="replace")
                match = re.search(r"makepad-run: session log (.+)", text)
                assert match, text
                transcript = Path(match[1].strip())
                assert "session-log fixture ready" in transcript.read_text(errors="replace")
                sig = {"term": signal.SIGTERM, "int": signal.SIGINT, "kill": signal.SIGKILL}.get(mode)
                if sig:
                    p.send_signal(sig)
                else:
                    (root / "go").touch()
                p.wait(timeout=30)
                if mode in ["return", "exit"]:
                    assert p.returncode == 0 and not transcript.exists(), (mode, output.read_text(errors="replace"))
                else:
                    assert p.returncode != 0 and transcript.exists(), (mode, output.read_text(errors="replace"))
                    assert "session-log fixture ready" in transcript.read_text(errors="replace")
                    if mode == "panic":
                        assert "session-log fixture panic" in transcript.read_text(errors="replace")
                    transcript.unlink()
                print("PASS session log", mode)
            finally:
                if p.poll() is None:
                    p.kill()
                    p.wait()
