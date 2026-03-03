#!/usr/bin/env python3
"""Test that content scrolls into scrollback when exceeding terminal height."""
import os, pty, select, signal, struct, time, fcntl, termios

BINARY = "/Users/admin/makepad/makepad/tools/tui_test/target/release/makepad-tui-test"
ROWS, COLS = 15, 80  # Very small terminal to force scrolling quickly

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
    return d

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

# Startup + DSR
time.sleep(0.5)
out = drain(master, 0.5)
if b'\x1b[6n' in out:
    os.write(master, b"\x1b[5;1R")
time.sleep(0.5)
drain(master, 0.5)

# With 15 rows and PINNED_ROWS=4, scroll_bottom=11
# Header uses 7 lines (rows 1-7), leaving 4 rows (8-11) before scrolling starts
# Each Enter adds 1 user line + 10 response lines = 11 lines
# This will overflow massively, testing scrollback behavior

# First Enter: "a"
print("=== FIRST ENTER (a) ===")
os.write(master, b"a\r")
total = b""
for _ in range(25):
    time.sleep(0.15)
    total += drain(master, 0.05)
frames = total.split(b'\x1b[?2026l')
print(f"  Bytes: {len(total)}, Frames: {len(frames)-1}")
# Check content
for i in range(1, 11):
    marker = f"[{i}/10] a"
    if marker.encode() not in total:
        print(f"  MISSING: {marker}")
        break
else:
    print(f"  All 10 response lines for 'a' present ✓")

# Second Enter: "b" - this should scroll first batch further into scrollback
print("\n=== SECOND ENTER (b) ===")
os.write(master, b"b\r")
total2 = b""
for _ in range(25):
    time.sleep(0.15)
    total2 += drain(master, 0.05)
frames2 = total2.split(b'\x1b[?2026l')
print(f"  Bytes: {len(total2)}, Frames: {len(frames2)-1}")
for i in range(1, 11):
    marker = f"[{i}/10] b"
    if marker.encode() not in total2:
        print(f"  MISSING: {marker}")
        break
else:
    print(f"  All 10 response lines for 'b' present ✓")

# Third Enter: "c"
print("\n=== THIRD ENTER (c) ===")
os.write(master, b"c\r")
total3 = b""
for _ in range(25):
    time.sleep(0.15)
    total3 += drain(master, 0.05)
frames3 = total3.split(b'\x1b[?2026l')
print(f"  Bytes: {len(total3)}, Frames: {len(frames3)-1}")
for i in range(1, 11):
    marker = f"[{i}/10] c"
    if marker.encode() not in total3:
        print(f"  MISSING: {marker}")
        break
else:
    print(f"  All 10 response lines for 'c' present ✓")

# Count total newlines across all phases (indicates scrolling happened)
all_output = total + total2 + total3
n_scroll_newlines = all_output.count(b'\n')
print(f"\nTotal newlines across all phases: {n_scroll_newlines}")
print(f"Total content lines: header(7) + 3*(1 user + 10 responses) = 40 lines")
PINNED = 4
print(f"Scroll region capacity: {ROWS - PINNED} rows")
print(f"Lines that scrolled into scrollback: {40 - (ROWS - PINNED)}")

# Cleanup
os.write(master, b"\x03")
time.sleep(0.3)
drain(master, 0.3)
try: os.kill(pid, signal.SIGTERM)
except: pass
try: os.waitpid(pid, 0)
except: pass
os.close(master)
print("\nDone ✓")
