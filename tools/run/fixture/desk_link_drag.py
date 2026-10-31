#!/usr/bin/env python3
"""Regression test: a Desk Link viewer's pointer drives the desktop while a button is stuck down on
another device (a wireless mouse that lost its release, one left under a book).

It presses and drags on the real desktop of the box it runs on, so it only runs on a named pool box
with nobody on it. Pin the job to that box, from the workspace root:
`devhub job submit --kind screen --node <pool box> -- python3 -I tools/run/fixture/desk_link_drag.py --test-box [app]`
Without `--test-box` it refuses, and it refuses whenever the WM senses a person, what devhub's lease
sense reads: a Desk Link viewer attached, or a person's input in the last IDLE_S (its own input
counts, so a rerun on one box waits that long), or a WM that does not answer. Everything it pressed
goes up, and the WM gets to read those ups, before its devices go away: on a failed assert and on
SIGTERM/SIGHUP/SIGINT too. (A WM from before the per-device tracking never hears a release for a
device that is destroyed holding a button.)

`app`: a JIT under the WM (Sandbox, a desktop app) that must get the drag too, by its inspect name.

The viewer's input goes in as what the WM's Desk Link injector makes of it, the same uinput device
ids and reports (viewing a desktop takes the admin credential, which nodes do not hold; the wire
in front of the injector is libs/desk_link/tests/server_input.rs). A second uinput mouse holds
BTN_RIGHT. Each app's recorded input must be, in order:
- with nothing held: press, moves, release, on the left button;
- with the button stuck, the WM: the stuck button's up BEFORE the viewer's first motion (hover and
  so the cursor only run with no button down), then press, moves, release, with no release lost;
- the mouse's own late release: nothing.
"""
import fcntl, glob, json, os, re, signal, socket, struct, subprocess, sys, tempfile, time

UI_SET_EVBIT, UI_SET_KEYBIT, UI_SET_RELBIT, UI_SET_ABSBIT = 0x40045564, 0x40045565, 0x40045566, 0x40045567
UI_SET_PROPBIT, UI_DEV_SETUP, UI_ABS_SETUP = 0x4004556E, 0x405C5503, 0x401C5504
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
EV_SYN, EV_KEY, EV_REL, EV_ABS = 0, 1, 2, 3
BTN_LEFT, BTN_RIGHT, REL_X, REL_Y, ABS_X, ABS_Y = 0x110, 0x111, 0, 1, 0, 1
INPUT_PROP_DIRECT = 1
BUS_USB, BUS_VIRTUAL = 3, 6
RESCAN_S = 2.5  # raw input finds new devices within a second; a loaded box gets a margin
RELEASE_S = 0.5  # the WM's read of the last ups, before the device goes
IDLE_S = 180  # lease.conf's default user_idle_min


def wm_activity():
    """The WM's own answer on who is here (platform/hal/src/wm_socket.rs); exits when none answers."""
    runtime = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    for path in sorted(glob.glob(os.path.join(runtime, "makepad-wm-*"))):
        try:
            with socket.socket(socket.AF_UNIX) as sock:
                sock.settimeout(5)
                sock.connect(path)
                sock.sendall(b"makepad-wm activity\n")
                line = sock.makefile().readline()
        except OSError:
            continue
        if line.startswith("makepad-wm activity "):
            return json.loads(line[len("makepad-wm activity "):])
    sys.exit("desk_link_drag: REFUSED: no WM answers here, cannot tell whether a person is on this box")


def refuse_viewer():
    viewers = wm_activity().get("viewers")
    if viewers != 0:
        sys.exit(f"desk_link_drag: REFUSED: Desk Link viewers attached: {viewers}")


def refuse_user():
    refuse_viewer()
    ms = wm_activity().get("input_ms")
    if ms is not None and ms < IDLE_S * 1000:
        sys.exit(f"desk_link_drag: REFUSED: a person's input {ms} ms ago (needs {IDLE_S} s quiet)")


def on_signal(number, _frame):
    sys.exit(f"desk_link_drag: signal {number}")  # unwinds through every device's release


class Uinput:
    def __init__(self, bus, vendor, product, name, setup):
        self.fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
        setup(self.fd)
        fcntl.ioctl(self.fd, UI_DEV_SETUP, struct.pack("HHHH80sI", bus, vendor, product, 1, name, 0))
        fcntl.ioctl(self.fd, UI_DEV_CREATE)
        self.pressed = set()

    def emit(self, *events):
        for ty, code, value in events:
            if ty == EV_KEY:
                (self.pressed.add if value else self.pressed.discard)(code)
        for ty, code, value in events + ((EV_SYN, 0, 0),):
            os.write(self.fd, struct.pack("qqHHi", 0, 0, ty, code, value))
        time.sleep(0.05)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        try:
            if self.pressed:
                self.emit(*[(EV_KEY, code, 0) for code in sorted(self.pressed)])
            time.sleep(RELEASE_S)
        finally:
            fcntl.ioctl(self.fd, UI_DEV_DESTROY)
            os.close(self.fd)


def stuck_mouse():
    def setup(fd):
        for bit, value in ((UI_SET_EVBIT, EV_KEY), (UI_SET_EVBIT, EV_REL), (UI_SET_KEYBIT, BTN_LEFT),
                           (UI_SET_KEYBIT, BTN_RIGHT), (UI_SET_RELBIT, REL_X), (UI_SET_RELBIT, REL_Y)):
            fcntl.ioctl(fd, bit, value)
    return Uinput(BUS_USB, 0x1234, 0x5678, b"desk_link_drag mouse", setup)


def viewer_pointer():
    """apps/wm/src/desk_link/inject.rs's pointer: DESK_LINK_INPUT_VENDOR, DESK_LINK_POINTER_PRODUCT."""
    def setup(fd):
        fcntl.ioctl(fd, UI_SET_PROPBIT, INPUT_PROP_DIRECT)
        for bit, value in ((UI_SET_EVBIT, EV_KEY), (UI_SET_EVBIT, EV_ABS), (UI_SET_KEYBIT, BTN_LEFT),
                           (UI_SET_KEYBIT, BTN_RIGHT), (UI_SET_ABSBIT, ABS_X), (UI_SET_ABSBIT, ABS_Y)):
            fcntl.ioctl(fd, bit, value)
        for code in (ABS_X, ABS_Y):
            fcntl.ioctl(fd, UI_ABS_SETUP, struct.pack("HHiiiiii", code, 0, 0, 0, 65535, 0, 0, 0))
    return Uinput(BUS_VIRTUAL, 0x4D50, 0x0001, b"Desk Link pointer", setup)


def makepad(*args):
    cmd = ["cargo", "run", "-q", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", *args]
    out = subprocess.run(cmd, capture_output=True, text=True)
    assert out.returncode == 0, f"{cmd}: {out.stdout}\n{out.stderr}"
    return out.stdout


def drag(viewer, x=30000, y=30000):
    viewer.emit((EV_ABS, ABS_X, x), (EV_ABS, ABS_Y, y))
    viewer.emit((EV_KEY, BTN_LEFT, 1))
    for i in range(1, 4):
        viewer.emit((EV_ABS, ABS_X, x + i * 800), (EV_ABS, ABS_Y, y + i * 500))
    viewer.emit((EV_KEY, BTN_LEFT, 0))


def record(apps, tmp, action):
    path_of = lambda app: os.path.join(tmp, app + ".rec")
    for app in apps:
        makepad("inspect", app, "input", "record", f"path={path_of(app)}")
    try:
        time.sleep(0.2)
        action()
        time.sleep(0.5)
    finally:
        for app in apps:
            makepad("inspect", app, "input", "stop")
    got = {}
    for app in apps:
        with open(path_of(app)) as rec:
            got[app] = [l.split(None, 2)[2].strip() for l in rec if l.strip() and not l.startswith("#")]
    return got


def buttons(lines):
    found = (re.match(r"mouse k=(down|up) .*?b=(\d+)", l) for l in lines)
    return [(m.group(1), int(m.group(2))) for m in found if m]


def check_drag(app, lines, stuck_up_first):
    pointer = [l for l in lines if l.startswith("mouse")]
    show = f"{app}:\n" + "\n".join(pointer)
    want = ([("up", 1)] if stuck_up_first else []) + [("down", 0), ("up", 0)]
    assert buttons(pointer) == want, f"buttons {buttons(pointer)}, want {want}\n{show}"
    index = [i for i, l in enumerate(pointer) if " b=0 " in l and (" k=down " in l or " k=up " in l)]
    moves = [l for l in pointer[index[0] + 1 : index[1]] if " k=move " in l]
    assert len(moves) >= 2, f"{len(moves)} moves within the drag\n{show}"
    if stuck_up_first:
        first_move = next(i for i, l in enumerate(pointer) if " k=move " in l)
        first_up = next(i for i, l in enumerate(pointer) if " k=up " in l)
        assert first_up < first_move, f"hover stayed frozen during the viewer's first motion\n{show}"


def main():
    args = sys.argv[1:]
    if "--test-box" not in args:
        sys.exit("desk_link_drag: REFUSED: it presses and drags on this desktop; run it on a pinned pool box with --test-box")
    args.remove("--test-box")
    refuse_user()
    for number in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(number, on_signal)
    extra = args[0] if args else None
    apps = ["makepad-app-wm"] + ([extra] if extra else [])
    with tempfile.TemporaryDirectory() as tmp, viewer_pointer() as viewer, stuck_mouse() as mouse:
        time.sleep(RESCAN_S)
        refuse_viewer()
        for app, lines in record(apps, tmp, lambda: drag(viewer)).items():
            check_drag(app, lines, False)
        print("drag, nothing held: ok")

        refuse_viewer()
        mouse.emit((EV_KEY, BTN_RIGHT, 1))
        time.sleep(0.3)
        got = record(apps, tmp, lambda: drag(viewer, 36000, 34000))
        check_drag("makepad-app-wm", got["makepad-app-wm"], True)
        print("drag, button stuck on another device: ok")
        if extra:
            pressed = buttons(got[extra])
            assert pressed[-2:] == [("down", 0), ("up", 0)], f"{extra}: {pressed}"
            print(f"{extra} under the WM: ok")

        late = record(["makepad-app-wm"], tmp, lambda: mouse.emit((EV_KEY, BTN_RIGHT, 0)))
        assert buttons(late["makepad-app-wm"]) == [], late
        print("late release: ok")
    print("desk_link_drag: ok")


if __name__ == "__main__":
    main()
