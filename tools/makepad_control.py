"""Makepad UI control library.

Reusable client and relay for any Makepad app using stdin/stdout control mode.
"""

from __future__ import annotations

import json
import os
import queue
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, Callable

# ---------------------------------------------------------------------------
# Makepad KeyCode integer encoding
# ---------------------------------------------------------------------------
# Makepad serializes KeyCode as an integer index via manual SerJson/DeJson.
# The order must match KEYCODE_VARIANTS in makepad/platform/src/event/keyboard.rs.

KEYCODE_VARIANTS = [
    "Escape", "Back", "Backtick",
    "Key0", "Key1", "Key2", "Key3", "Key4", "Key5", "Key6", "Key7", "Key8", "Key9",
    "Minus", "Equals", "Backspace", "Tab",
    "KeyQ", "KeyW", "KeyE", "KeyR", "KeyT", "KeyY", "KeyU", "KeyI", "KeyO", "KeyP",
    "LBracket", "RBracket", "ReturnKey",
    "KeyA", "KeyS", "KeyD", "KeyF", "KeyG", "KeyH", "KeyJ", "KeyK", "KeyL",
    "Semicolon", "Quote", "Backslash",
    "KeyZ", "KeyX", "KeyC", "KeyV", "KeyB", "KeyN", "KeyM",
    "Comma", "Period", "Slash",
    "Control", "Alt", "Shift", "Logo",
    "Space", "Capslock",
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    "PrintScreen", "ScrollLock", "Pause",
    "Insert", "Delete", "Home", "End", "PageUp", "PageDown",
    "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4",
    "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9",
    "NumpadEquals", "NumpadSubtract", "NumpadAdd", "NumpadDecimal",
    "NumpadMultiply", "NumpadDivide", "Numlock", "NumpadEnter",
    "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
    "Unknown",
]

_KEYCODE_INDEX = {name.lower(): idx for idx, name in enumerate(KEYCODE_VARIANTS)}

KEY_MAP = {
    "enter": "ReturnKey", "return": "ReturnKey", "tab": "Tab", "escape": "Escape", "esc": "Escape",
    "backspace": "Backspace", "delete": "Delete",
    "up": "ArrowUp", "down": "ArrowDown", "left": "ArrowLeft", "right": "ArrowRight",
    "home": "Home", "end": "End", "pageup": "PageUp", "pagedown": "PageDown",
    "space": "Space", "capslock": "Capslock",
    "printscreen": "PrintScreen", "scrolllock": "ScrollLock", "pause": "Pause",
    "insert": "Insert", "numlock": "Numlock",
    **{c: f"Key{c.upper()}" for c in "abcdefghijklmnopqrstuvwxyz"},
    **{d: f"Key{d}" for d in "0123456789"},
    **{f"f{n}": f"F{n}" for n in range(1, 13)},
}

MODS = {"shift": False, "control": False, "alt": False, "logo": False}

BUTTON_BITS = {"left": 1, "right": 2, "middle": 4}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def keycode_index(name: str) -> int:
    """Map key name to Makepad KeyCode integer index."""
    canonical = KEY_MAP.get(name.lower(), name)
    idx = _KEYCODE_INDEX.get(canonical.lower())
    return idx if idx is not None else _KEYCODE_INDEX["unknown"]


def tick(conn: Connection) -> None:
    """Send Tick to trigger event processing."""
    conn.send({"Tick": []})


def mkv(name: str, data: Any) -> dict:
    """Wrap a value as a Makepad SerJson tuple variant: {"Name":[data]}"""
    return {name: [data]}


def parse_dump_origin(dump: str) -> tuple[float, float, float] | None:
    """Extract origin (ox, oy, dpi) from widget dump's first O line."""
    for line in dump.split("\n"):
        parts = line.split()
        if parts and parts[0] == "O":
            try:
                return (float(parts[1]), float(parts[2]), float(parts[3]))
            except (IndexError, ValueError):
                pass
    return None


def query_widget_dump(dump: str, query: str) -> list[str]:
    """Query widget dump for matching widgets. Returns list of line strings."""
    results = []
    query_lower = query.lower()
    is_id = query.startswith("id:")
    is_type = query.startswith("type:")
    term = query[3:] if is_id else query[5:] if is_type else query_lower

    for line in dump.split("\n"):
        parts = line.split()
        if len(parts) < 4 or parts[0] in ("O", "W"):
            continue
        # Format: index parent name type x y w h [ops]
        name = parts[2]
        wtype = parts[3] if len(parts) > 3 else ""
        if is_id:
            match = name == term
        elif is_type:
            match = wtype.lower() == term.lower()
        else:
            match = term in name.lower() or term in wtype.lower()
        if match:
            results.append(line)
    return results


# ---------------------------------------------------------------------------
# Connection: Unix domain socket, JSON lines
# ---------------------------------------------------------------------------

class Connection:
    """Bidirectional JSON-line connection over a Unix domain socket."""

    def __init__(self, sock_path: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(sock_path)
        self.rfile = self.sock.makefile("r", encoding="utf-8")
        self.wfile = self.sock.makefile("w", encoding="utf-8")
        self.rx: queue.Queue[dict] = queue.Queue()
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self) -> None:
        try:
            for line in self.rfile:
                line = line.strip()
                if not line:
                    continue
                try:
                    self.rx.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
        except (OSError, ValueError):
            pass

    def send(self, msg: Any) -> None:
        """Send a JSON message as one line."""
        self.wfile.write(json.dumps(msg) + "\n")
        self.wfile.flush()

    def recv(self, match_fn: Callable | None = None, timeout: float = 30) -> dict | None:
        """Wait for a matching response."""
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            try:
                msg = self.rx.get(timeout=min(remaining, 0.5))
            except queue.Empty:
                continue
            if match_fn is None or match_fn(msg):
                return msg

    def send_recv(self, msg: Any, match_fn: Callable | None = None, timeout: float = 30) -> dict | None:
        """Send and wait for matching response."""
        self.send(msg)
        return self.recv(match_fn, timeout)

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


def connect(sock_path: str) -> Connection:
    """Open connection to a Makepad control socket."""
    return Connection(sock_path)


# ---------------------------------------------------------------------------
# Socket relay
# ---------------------------------------------------------------------------

def run_relay(
    cmd: list[str],
    socket_path: str,
    *,
    env: dict[str, str] | None = None,
    on_json_line: Callable[[dict], None] | None = None,
    on_nonjson_line: Callable[[str], None] | None = None,
    ready_event: threading.Event | None = None,
    ready_timeout: float = 30.0,
) -> int:
    """Launch subprocess, relay JSON lines over Unix socket.

    cmd: subprocess command
    socket_path: path for Unix domain socket
    env: environment for subprocess (merged with os.environ)
    on_json_line: called for each parsed JSON line from subprocess stdout
    on_nonjson_line: called for each non-JSON line from subprocess stdout
    ready_event: if set, will be signaled when ReadyToStart is received
    ready_timeout: seconds to wait for ReadyToStart before continuing

    Blocks until subprocess exits. Returns exit code.
    """
    # Clean up stale socket.
    sock_parent = os.path.dirname(socket_path)
    if sock_parent:
        os.makedirs(sock_parent, exist_ok=True)
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    # Create Unix domain socket.
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(socket_path)
    server.listen(8)
    server.setblocking(False)

    # Build subprocess environment.
    proc_env = os.environ.copy()
    # Enable Makepad app-level control mode (stdin/stdout JSON protocol).
    proc_env["MAKEPAD_EVENTS"] = "1"
    if env:
        proc_env.update(env)

    proc = subprocess.Popen(
        cmd,
        env=proc_env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=None,
    )
    assert proc.stdin is not None
    assert proc.stdout is not None

    clients: list[socket.socket] = []
    client_bufs: dict[int, str] = {}
    stdout_lock = threading.Lock()
    stdin_lock = threading.Lock()
    got_ready = threading.Event()

    def _read_stdout() -> None:
        assert proc.stdout is not None
        for raw in proc.stdout:
            line = raw.decode("utf-8", errors="replace") if isinstance(raw, bytes) else raw
            line = line.rstrip("\n")
            if not line:
                continue

            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                if on_nonjson_line:
                    on_nonjson_line(line)
                else:
                    print(line, file=sys.stderr)
                continue

            if on_json_line:
                on_json_line(msg)

            if isinstance(msg, dict) and "ReadyToStart" in msg:
                got_ready.set()
                if ready_event:
                    ready_event.set()

            # Broadcast to all connected clients.
            encoded = (line + "\n").encode("utf-8")
            with stdout_lock:
                dead = []
                for c in clients:
                    try:
                        c.sendall(encoded)
                    except (OSError, BrokenPipeError):
                        dead.append(c)
                for c in dead:
                    clients.remove(c)
                    client_bufs.pop(id(c), None)
                    c.close()

        # stdout EOF
        got_ready.set()
        if ready_event:
            ready_event.set()

    reader = threading.Thread(target=_read_stdout, daemon=True)
    reader.start()

    # Wait for ReadyToStart.
    got_ready.wait(timeout=ready_timeout)

    rc = proc.poll()
    if rc is not None and not got_ready.is_set():
        server.close()
        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass
        return rc

    def _write_to_proc(data: bytes) -> None:
        assert proc.stdin is not None
        with stdin_lock:
            try:
                proc.stdin.write(data)
                proc.stdin.flush()
            except (OSError, BrokenPipeError):
                pass

    try:
        while proc.poll() is None:
            rlist = [server] + clients
            try:
                readable, _, _ = select.select(rlist, [], [], 0.5)
            except (ValueError, OSError):
                with stdout_lock:
                    alive = []
                    for c in clients:
                        try:
                            c.fileno()
                            alive.append(c)
                        except Exception:
                            client_bufs.pop(id(c), None)
                    clients[:] = alive
                continue

            for s in readable:
                if s is server:
                    conn, _ = server.accept()
                    conn.setblocking(True)
                    with stdout_lock:
                        clients.append(conn)
                        client_bufs[id(conn)] = ""
                else:
                    try:
                        data = s.recv(65536)
                    except (OSError, ConnectionResetError):
                        data = b""
                    if not data:
                        with stdout_lock:
                            if s in clients:
                                clients.remove(s)
                            client_bufs.pop(id(s), None)
                        s.close()
                        continue
                    buf = client_bufs.get(id(s), "") + data.decode("utf-8", errors="replace")
                    while "\n" in buf:
                        line, buf = buf.split("\n", 1)
                        line = line.strip()
                        if line:
                            _write_to_proc((line + "\n").encode("utf-8"))
                    client_bufs[id(s)] = buf
    except KeyboardInterrupt:
        pass
    finally:
        server.close()
        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)

    return proc.returncode or 0
