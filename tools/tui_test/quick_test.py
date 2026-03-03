#!/usr/bin/env python3
"""PTY test for scroll-region tui_test."""
import os, pty, select, signal, struct, time, fcntl, termios

BINARY = "/Users/admin/makepad/makepad/tools/tui_test/target/release/makepad-tui-test"
ROWS, COLS = 20, 80  # Small terminal to test scrolling

def set_winsize(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))

def drain(fd, t=0.5):
    d = b""
    end = time.time() + t
    
    while True:
        rem = max(0, end - time.time())
        r, _, _ = select.select([fd], [], [], rem)
        if r:
            try:
                c = os.read(fd, 65536)
                if not c: break
                d += c
            except: break
        else: break
    return dno th

def extract_texts(raw):
    texts = []
    i = 0
    while i < len(raw):
        if raw[i] == 0x1b:
            i += 1
            if i < len(raw) and raw[i] == ord('['):
                i += 1
                while i < len(raw) and 0x20 <= raw[i] <= 0x3f: i += 1
                while i < len(raw) and 0x20 <= raw[i] <= 0x2f: i += 1
                if i < len(raw): i += 1
            elif i < len(raw): i += 1
        elif raw[i] >= 0x20:
            ts = i
            while i < len(raw) and raw[i] >= 0x20 and raw[i] != 0x1b: i += 1
            t = raw[ts:i].decode('utf-8', errors='replace').strip()
            if t: texts.append(t)
        else: i += 1
    return texts

def find_seqs(raw, prefix):
    """Find all CSI sequences matching a prefix."""
    results = []
    i = 0
    while i < len(raw):
        if raw[i] == 0x1b and i+1 < len(raw) and raw[i+1] == ord('['):
            start = i
            i += 2
            seq = b'\x1b['
            while i < len(raw) and 0x20 <= raw[i] <= 0x3f:
                seq += bytes([raw[i]])
                i += 1
            if i < len(raw) and 0x40 <= raw[i] <= 0x7e:
                seq += bytes([raw[i]])
                i += 1
            if prefix.encode() in seq:
                results.append(seq.decode('ascii', errors='replace'))
        else:
            i += 1
    return results

master, slave = pty.openpty()
set_winsize(slave, ROWS, COLS)

pid = os.fork()
if pid == 0:
    os.setsid()
    os.dup2(slave, 0)
    os.dup2(slave, 1)
    os.dup2(slave, 2)
    os.close(master)
    os.close(slave)
    os.environ['TERM'] = 'xterm-256color'
    os.chdir('/Users/admin/makepad/makepad')
    os.execv(BINARY, [BINARY])

os.close(slave)

# Startup
time.sleep(0.5)
out = drain(master, 0.5)
print(f"Startup: {len(out)} bytes")
if b'\x1b[6n' in out:
    os.write(master, b"\x1b[5;1R")

# Initial render
time.sleep(0.5)
init = drain(master, 0.5)
print(f"Initial render: {len(init)} bytes")

# Check for scroll region setup
scroll_seqs = find_seqs(init, 'r')
print(f"Scroll region commands: {scroll_seqs}")

texts = extract_texts(init)
print(f"\nInitial visible content:")
for t in texts:
    print(f"  {t}")

# Type "hi" + Enter
os.write(master, b"hi")
time.sleep(0.3)
drain(master, 0.3)

os.write(master, b"\r")
total = b""
for _ in range(25):
    time.sleep(0.15)
    total += drain(master, 0.05)

# Count sync frames
frames = total.split(b'\x1b[?2026l')
n_frames = len(frames) - 1
print(f"\nAfter Enter: {len(total)} bytes, {n_frames} sync frames")

# Check for scroll region and LF (natural scrolling)
scroll_rgn = find_seqs(total, 'r')
n_newlines = total.count(b'\n')
print(f"Scroll region commands in output: {len(scroll_rgn)}")
print(f"Newlines in output: {n_newlines}")

# Show last frame content
if n_frames > 0:
    last = frames[-2] + b'\x1b[?2026l'
    texts = extract_texts(last)
    print(f"\nFinal frame content:")
    for t in texts:
        print(f"  {t}")

# Verify all 10 responses present
for i in range(1, 11):
    marker = f"[{i}/10] hi"
    if marker.encode() in total:
        print(f"  \u2713 {marker}")
    else:
        print(f"  \u2717 MISSING: {marker}")

# Check pinned area (prompt + status should be at bottom rows)
prompt_at_bottom = b'\x1b[18;1H' in total or b'\x1b[18;' in total  # row 18 = ROWS-2
status_at_bottom = b'\x1b[20;1H' in total or b'\x1b[20;' in total  # row 20 = ROWS
print(f"\nPrompt pinned at row {ROWS-2}: {'yes' if prompt_at_bottom else 'no'}")
print(f"Status pinned at row {ROWS}: {'yes' if status_at_bottom else 'no'}")

# Cleanup
os.write(master, b"\x03")
time.sleep(0.3)
cleanup = drain(master, 0.3)
print(f"\nCleanup: {len(cleanup)} bytes")
# Check scroll region reset
if b'\x1b[r' in cleanup:
    print("  Scroll region reset on exit: yes")

try: os.kill(pid, signal.SIGTERM)
except: pass
try: os.waitpid(pid, 0)
except: pass
os.close(master)
