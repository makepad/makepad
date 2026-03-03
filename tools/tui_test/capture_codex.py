#!/usr/bin/env python3
"""Capture terminal escape sequences from codex startup + 'hi' input."""
import os, pty, select, signal, struct, sys, time, fcntl, termios

CODEX_BIN = "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/codex/codex"
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

def escape_for_display(data):
    """Convert raw bytes to readable escape sequence notation."""
    lines = []
    i = 0
    while i < len(data):
        b = data[i]
        if b == 0x1b:  # ESC
            # Find the extent of this escape sequence
            seq_start = i
            i += 1
            if i < len(data):
                next_b = data[i]
                if next_b == ord('['):  # CSI
                    i += 1
                    while i < len(data) and 0x20 <= data[i] <= 0x3f:
                        i += 1
                    if i < len(data):
                        i += 1  # final byte
                    seq = data[seq_start:i]
                    lines.append(f"  CSI: {seq!r}  =>  {describe_csi(seq)}")
                elif next_b == ord(']'):  # OSC
                    i += 1
                    while i < len(data) and data[i] not in (0x07, 0x1b):
                        i += 1
                    if i < len(data) and data[i] == 0x07:
                        i += 1
                    elif i + 1 < len(data) and data[i] == 0x1b and data[i+1] == ord('\\'):
                        i += 2
                    seq = data[seq_start:i]
                    lines.append(f"  OSC: {seq!r}")
                elif next_b == ord('7'):
                    i += 1
                    lines.append(f"  ESC 7  (DECSC - save cursor)")
                elif next_b == ord('8'):
                    i += 1
                    lines.append(f"  ESC 8  (DECRC - restore cursor)")
                else:
                    i += 1
                    lines.append(f"  ESC {chr(next_b)!r}")
            else:
                lines.append(f"  ESC (bare)")
        elif b == 0x0d:  # CR
            lines.append(f"  CR (\\r)")
            i += 1
        elif b == 0x0a:  # LF
            lines.append(f"  LF (\\n)")
            i += 1
        elif b == 0x08:  # BS
            lines.append(f"  BS (\\x08)")
            i += 1
        elif b < 0x20:
            lines.append(f"  CTRL: 0x{b:02x}")
            i += 1
        else:
            # Collect printable text
            text_start = i
            while i < len(data) and data[i] >= 0x20 and data[i] != 0x1b:
                i += 1
            text = data[text_start:i].decode('utf-8', errors='replace')
            if len(text) > 80:
                lines.append(f"  TEXT: {text[:80]!r}... ({len(text)} chars)")
            else:
                lines.append(f"  TEXT: {text!r}")
    return "\n".join(lines)

def describe_csi(seq):
    """Human-readable description of common CSI sequences."""
    s = seq.decode('utf-8', errors='replace')
    # Strip ESC[
    if s.startswith('\x1b['):
        body = s[2:]
    else:
        return ""
    
    descs = {
        '?2004h': 'Enable bracketed paste',
        '?2004l': 'Disable bracketed paste',
        '?1004h': 'Enable focus reporting',
        '?1004l': 'Disable focus reporting',
        '?2026h': 'Begin synchronized output',
        '?2026l': 'End synchronized output',
        '?25h': 'Show cursor',
        '?25l': 'Hide cursor',
        '?1049h': 'Enable alt screen',
        '?1049l': 'Disable alt screen',
        '>1u': 'Enable kitty keyboard (flags=1)',
        '>3u': 'Enable kitty keyboard (flags=3)',
        '>5u': 'Enable kitty keyboard (flags=5)',
        '>7u': 'Enable kitty keyboard (flags=7)',
        '<1u': 'Disable kitty keyboard (pop 1)',
        '<u': 'Disable kitty keyboard (pop)',
        's': 'Save cursor position (SCO)',
        'u': 'Restore cursor position (SCO)',
        '0m': 'Reset attributes (SGR)',
        'm': 'Reset attributes (SGR)',
        '2K': 'Erase entire line',
        '0K': 'Erase to end of line',
        'K': 'Erase to end of line',
        'H': 'Cursor home',
        '2J': 'Erase entire screen',
        '6n': 'Device status report (cursor position query)',
    }
    
    if body in descs:
        return descs[body]
    
    if body.endswith('A'):
        return f"Cursor up {body[:-1] or '1'}"
    if body.endswith('B'):
        return f"Cursor down {body[:-1] or '1'}"
    if body.endswith('C'):
        return f"Cursor forward {body[:-1] or '1'}"
    if body.endswith('D'):
        return f"Cursor back {body[:-1] or '1'}"
    if body.endswith('m'):
        return f"SGR: {body}"
    if body.endswith('H'):
        return f"Cursor position: {body}"
    if body.endswith('J'):
        return f"Erase display: {body}"
    if body.endswith('K'):
        return f"Erase line: {body}"
    if body.endswith('t'):
        return f"Window op: {body}"
    if body.endswith('n'):
        return f"Device status: {body}"
    
    return body

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
        os.environ['COLUMNS'] = str(COLS)
        os.environ['LINES'] = str(ROWS)
        os.chdir('/Users/admin/makepad/makepad')
        os.execv(CODEX_BIN, [CODEX_BIN])
    
    os.close(slave)
    
    phases = []
    alive = True
    
    def safe_write(fd, data):
        nonlocal alive
        if not alive:
            return
        try:
            os.write(fd, data)
        except OSError:
            alive = False
    
    def safe_read(fd, timeout):
        nonlocal alive
        if not alive:
            return b""
        data = read_available(fd, timeout)
        if not data:
            # Check if child still alive
            r, status = os.waitpid(pid, os.WNOHANG)
            if r != 0:
                alive = False
        return data
    
    # Phase 1: Startup
    print("--- Waiting for startup (6s) ---", flush=True)
    startup = safe_read(master, timeout=6.0)
    phases.append(("STARTUP", startup))
    print(f"  Captured {len(startup)} bytes", flush=True)
    
    # Phase 2: Type "hi" + Enter
    if alive:
        print("--- Typing 'hi' ---", flush=True)
        safe_write(master, b"hi")
        time.sleep(0.5)
        after_type = safe_read(master, timeout=1.5)
        phases.append(("AFTER_TYPING_HI", after_type))
        print(f"  Captured {len(after_type)} bytes", flush=True)
    
    if alive:
        print("--- Pressing Enter ---", flush=True)
        safe_write(master, b"\r")
        time.sleep(3.0)
        after_enter = safe_read(master, timeout=4.0)
        phases.append(("AFTER_ENTER", after_enter))
        print(f"  Captured {len(after_enter)} bytes", flush=True)
    
    # Phase 3: Send Ctrl-C to exit
    if alive:
        print("--- Sending Ctrl-C ---", flush=True)
        safe_write(master, b"\x03")
        time.sleep(1.0)
        after_exit = safe_read(master, timeout=2.0)
        phases.append(("EXIT", after_exit))
        print(f"  Captured {len(after_exit)} bytes", flush=True)
    
    try:
        os.kill(pid, signal.SIGTERM)
    except:
        pass
    try:
        os.waitpid(pid, 0)
    except:
        pass
    os.close(master)
    
    # Write analysis
    out = []
    for name, data in phases:
        out.append(f"\n{'='*60}")
        out.append(f"PHASE: {name} ({len(data)} bytes)")
        out.append(f"{'='*60}")
        out.append(f"\nRAW HEX (first 2000 bytes):")
        hexdata = data[:2000]
        for off in range(0, len(hexdata), 32):
            chunk = hexdata[off:off+32]
            hex_str = " ".join(f"{b:02x}" for b in chunk)
            ascii_str = "".join(chr(b) if 0x20 <= b < 0x7f else '.' for b in chunk)
            out.append(f"  {off:04x}: {hex_str:<96s} {ascii_str}")
        out.append(f"\nESCAPE SEQUENCE ANALYSIS:")
        out.append(escape_for_display(data))
    
    report = "\n".join(out)
    with open("/Users/admin/makepad/makepad/tools/tui_test/codex_capture.txt", "w") as f:
        f.write(report)
    print(report)
    print("\n\nReport written to codex_capture.txt")

if __name__ == "__main__":
    main()
