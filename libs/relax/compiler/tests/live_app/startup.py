#!/usr/bin/env python3
"""Live edits wait for present even across a compile-quiet startup interval.

Run with an absolute release Relax binary; MAKEPAD_RUN_STD names this checkout's std.
The fixture uses the platform crate name to exercise the GUI admission policy,
without creating a window or requiring a GPU.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix="relax-startup-") as tmp:
    root = Path(tmp)
    (root / "src").mkdir()
    (root / "Cargo.toml").write_text('[package]\nname="makepad-platform"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
    (root / "src/main.rs").write_text('''
mod helper;
extern "C" { fn relax_first_present(); }
fn main() {
    println!("waiting for present");
    while !std::path::Path::new("present").exists() {
        println!("value {}", std::hint::black_box(helper::value as fn() -> u32)());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    unsafe { relax_first_present(); }
    loop {
        let v = std::hint::black_box(helper::value as fn() -> u32)();
        println!("value {}", v);
        if v == 2 { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
''')
    helper = root / "src/helper.rs"
    helper.write_text('#[inline(never)] pub fn value() -> u32 { 1 }\n')
    log_path = root / "run.log"
    with log_path.open("w") as log:
        p = subprocess.Popen([binary, "run", "makepad-platform"], cwd=root,
                             env=dict(os.environ, MAKEPAD_RUN_CACHE="off", MAKEPAD_RUN_STARTUP_PHASE="0"),
                             stdout=log, stderr=subprocess.STDOUT)
        def text():
            return log_path.read_text(errors="replace")
        def wait_for(predicate):
            deadline = time.monotonic() + 120
            while not predicate():
                assert p.poll() is None, text()
                assert time.monotonic() < deadline, text()
                time.sleep(.05)
        try:
            wait_for(lambda: "value 1" in text() and "watching" in text())
            helper.write_text('#[inline(never)] pub fn value() -> u32 { 2 }\n')
            time.sleep(3)
            assert "patched" not in text() and "value 2" not in text(), text()
            (root / "present").touch()
            p.wait(timeout=120)
            assert p.returncode == 0 and "value 2" in text(), text()
            assert text().count("helper.rs: patched") == 1, text()
            print("PASS startup edit queued once until present; scheduling opt-out cannot bypass gate")
        finally:
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()
