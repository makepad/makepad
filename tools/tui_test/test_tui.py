#!/usr/bin/env python3
"""Run tui_test in a PTY and capture its terminal output for verification."""
import os, pty, select, signal, struct, sys, time, fcntl, termios

BINARY = "/Users/admin/makepad/makepad/tools/tui_test/target/release/makepad-tui-test"
COLS, ROWS = 120, 40

def set_winsize(fd, rows, cols):
    s = struct.pack('HHHH', rows, cols, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, s)

def read_available(fd, timeout=0.5):
    data = b""
    end = time.time() + timeout
    while True:
        remaining = max(0, end - time.time())
        r, _, _ = select.select([fd], [], [], remaining)
        if r:
            try:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                data += chunk
            except OSError:
                break
        else:
            break
    return data

def describe_output(data):
    """Summarize escape sequences found in output."""
    lines = []
    i = 0
    while i < len(data):
        b = data[i]
        if b == 0x1b:
            seq_start = i
            i += 1
            if i < len(data):
                nb = data[i]
                if nb == ord('['):
                    i += 1
                    while i < len(data) and 0x20 <= data[i] <= 0x3f:
                        i += 1
                    while i < len(data) and 0x20 <= data[i] <= 0x2f:
                        i += 1
                    if i < len(data):
                        i += 1
                    seq = data[seq_start:i]
                    lines.append(f"  CSI: {seq!r}")
                elif nb == ord(']'):
                    i += 1
                    while i < len(data) and data[i] not in (0x07, 0x1b):
                        i += 1
                    if i < len(data) and data[i] == 0x07:
                        i += 1
                    elif i + 1 < len(data) and data[i] == 0x1b and data[i+1] == ord('\\'):
                        i += 2
                    seq = data[seq_start:i]
                    lines.append(f"  OSC: {seq!r}")
                elif nb == ord('M'):
                    i += 1
                    lines.append(f"  ESC M (Reverse Index)")
                else:
                    i += 1
                    lines.append(f"  ESC {chr(nb)}")
        elif b == 0x0d:
            lines.append(f"  CR")
            i += 1
        elif b == 0x0a:
            lines.append(f"  LF")
            i += 1
        elif b < 0x20:
            lines.append(f"  CTRL 0x{b:02x}")
            i += 1
        else:
            ts = i
            while i < len(data) and data[i] >= 0x20 and data[i] != 0x1b:
                i += 1
            text = data[ts:i].decode('utf-8', errors='replace')
            if len(text) > 80:
                lines.append(f"  TEXT: {text[:80]!r}...")
            else:
                lines.append(f"  TEXT: {text!r}")
    return "\n".join(lines)

def main():
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

    print("=== STARTUP ===")
    startup = read_available(master, timeout=1.0)
    print(f"  {len(startup)} bytes (setup sequences)")
    print(describe_output(startup))

    # Respond to ESC[6n with a cursor position report: row 5, col 1
    # (simulates the terminal emulator responding to the DSR query)
    if b'\x1b[6n' in startup:
        print("  [injecting cursor position report ESC[5;1R]")
        os.write(master, b"\x1b[5;1R")

    time.sleep(0.5)
    initial_render = read_available(master, timeout=1.0)
    print(f"\n  Initial render: {len(initial_render)} bytes")
    print(describe_output(initial_render))

    print("\n=== TYPE 'hi' ===")
    os.write(master, b"hi")
    time.sleep(0.3)
    after_type = read_available(master, timeout=1.0)
    print(f"  {len(after_type)} bytes")
    print(describe_output(after_type))

    print("\n=== PRESS ENTER ===")
    os.write(master, b"\r")
    # Drain continuously so PTY buffer doesn't block the child
    after_enter = b""
    for _ in range(30):
        time.sleep(0.1)
        after_enter += read_available(master, timeout=0.05)
    print(f"  {len(after_enter)} bytes")
    # Show just key sequences, not full re-renders
    lines = after_enter.split(b'\x1b[?2026l')
    print(f"  {len(lines) - 1} sync frames rendered")
    # Show last frame's content
    if len(lines) > 1:
        last = lines[-2] + b'\x1b[?2026l'
        # Extract TEXT segments from last frame
        texts = []
        i = 0
        while i < len(last):
            if last[i] == 0x1b:
                i += 1
                if i < len(last) and last[i] == ord('['):
                    i += 1
                    while i < len(last) and 0x20 <= last[i] <= 0x3f: i += 1
                    while i < len(last) and 0x20 <= last[i] <= 0x2f: i += 1
                    if i < len(last): i += 1
                elif i < len(last): i += 1
            elif last[i] >= 0x20:
                ts = i
                while i < len(last) and last[i] >= 0x20 and last[i] != 0x1b: i += 1
                t = last[ts:i].decode('utf-8', errors='replace').strip()
                if t: texts.append(t)
            else: i += 1
        print("  Last frame content:")
        for t in texts:
            print(f"    {t}")

    print("\n=== CTRL-C ===")
    os.write(master, b"\x03")
    time.sleep(0.5)
    after_exit = read_available(master, timeout=1.0)
    print(f"  {len(after_exit)} bytes")
    print(describe_output(after_exit))

    try:
        os.kill(pid, signal.SIGTERM)
    except:
        pass
    try:
        os.waitpid(pid, 0)
    except:
        pass
    os.close(master)

if __name__ == "__main__":
    main()
