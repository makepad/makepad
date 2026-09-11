# App remote control

Use this reference when inspecting or driving a Makepad app. Ownership,
focus, and capture policy live in [AGENTS.md](../../AGENTS.md).

The implementation is [platform/src/remote.rs](../../platform/src/remote.rs).
Fetch `GET /` from your running instance for its current route list; it can
contain newer diagnostics than this guide.

## Launch and discovery

Build the package in release mode, then launch the resulting standalone
binary with `--remote`. For a hidden verification run from the repo root:

```sh
cargo build --release -p makepad-example-splash
MAKEPAD_HIDE_WINDOWS=1 ./target/release/makepad-example-splash --remote
```

Capture the launch output using your process tool. The startup line contains
everything needed to address and clean up that instance:

```text
[makepad-remote] listening on 127.0.0.1:53412 pid=9931 app=makepad-example-splash grabs=/tmp/makepad-remote/makepad-example-splash-9931
```

The port is ephemeral. `--remote=PORT` pins it; `MAKEPAD_REMOTE=1` or
`MAKEPAD_REMOTE=PORT` also enables the service. Prefer the explicit launch
flag for agent inspection.

Do not discover a port by taking over another running instance. Retain your
own launch's PID and endpoint. On code changes, close that instance, rebuild,
and relaunch before inspecting again.

## Common routes

The core routes use GET. JSON replies are one line; help/tree dumps are text
and raw grabs are PNG. Errors carry an `err` field.

| Route | Purpose |
|---|---|
| `/`, `/help` | Current protocol help |
| `/s?w=ID` | App/PID and window geometry; omit `w` to list windows |
| `/g?w=ID&scale=0.5`, `/gseq?n=8&every_ms=50&scale=0.5` | Capture the next presented frame, or N frames on request cadence (1–64, ≥8 ms, ≤60 s span); Metal/raw readbacks downsample before worker PNG encoding; return absolute `png` path(s) and per-frame `capture_ms`/`encode_ms` (`gseq.frames`); input `wait=1` remains a next-frame barrier |
| `/g?raw=1` | Return PNG bytes instead of a path |
| `/gq?scale=0.5` | Grab windows, then quit; returns `png` paths and `quit:1` |
| `/snap?q=TEXT&w=ID&all=1` | Widget ids/types/text and window-local rectangles |
| `/d`, `/dump` | Indented widget tree |
| `/m?k=click&x=X&y=Y` | Mouse input; kinds also include move, down, up, scroll |
| `/click?x=X&y=Y` | Click alias |
| `/k?k=press&c=KeyA` | Key input; down/up are also supported |
| `/t?t=TEXT`, `/k?t=TEXT` | Text/IME input |
| `/drop?path=ABSOLUTE_PATH&x=X&y=Y` | One file through native drag, drop, and drag-end events |
| `/log?n=50&since=N` | App log tail; `n` in the reply is the latest sequence |
| `/close?w=ID` | Close one window normally |
| `/quit` | Graceful shutdown without a final grab |

`/snap` entries include `window_id` and `enabled`, plus `selected` when the
widget exposes a selection, alongside the compact `i`/`ty`/`r`/`w` fields.

Query parameters are optional unless needed for the operation. Input routes
accept `w=ID` to target a window and `wait=1` to answer after the next frame.
With no window specified, routes generally use the first window.

Mouse buttons: `b=0` left, `b=1` right, `b=2` middle. Scroll takes `dx`
and `dy`. Add `hw=1` when testing the hardware pointer-lock/pin transform.
Keyboard modifiers are `shift=1`, `ctrl=1`, `alt=1`, and `cmd=1`.

POST with a flat JSON body is supported. The input parser also accepts long
names such as `window`, `kind`, `button`, `text`, and `code`. Use
`curl --get --data-urlencode` for arbitrary text in query strings.

File drops require an absolute path (at most 4096 bytes) and finite,
nonnegative `x`/`y` inside the window. Optional parameters are `w`/`window`
and `wait=1`; duplicate or unknown parameters are rejected. The remote
thread sends only the path; the app owns validation and loading. The reply
reports `drop_handled` and `drag_response`, which confirm event handling,
not that an asynchronous file import has completed. For example:

```sh
curl --get --data-urlencode 'path=/absolute/path/reference car.png' \
  --data-urlencode 'x=300' --data-urlencode 'y=400' --data-urlencode 'wait=1' \
  'http://127.0.0.1:53412/drop'
```

## Drive an owned instance

After replacing the example port with the one your app printed:

```sh
curl --fail --silent --show-error 'http://127.0.0.1:53412/'
curl --fail --silent --show-error 'http://127.0.0.1:53412/snap?q=press_demo'
```

Read the returned `r: [x, y, width, height]`, calculate its center, and feed
that point to `/click?...&wait=1`. Do not reuse coordinates from another
window or an earlier layout.

```sh
curl --fail --silent --show-error 'http://127.0.0.1:53412/log?n=20'
curl --fail --silent --show-error 'http://127.0.0.1:53412/gq?scale=0.5'
```

Read a returned PNG with the local image viewer when visual inspection is
needed. Confirm the owned process exits after cleanup. Use `/quit` if a
backend cannot grab; do not replace a failed grab with an OS screenshot.

## GPU runs, hidden windows, and the simulated-GPU backend

- The proof rig for anything visual is the native GPU backend (Metal on
  macOS) in an owned `--remote` instance. `MAKEPAD_HIDE_WINDOWS=1` keeps its
  windows off the user's screen; grabs and input still work because `/g`
  forces a present on a hidden window. Measured on macOS (2026-09-10): the
  first hidden `/g` answers in ~100 ms, later ones in ~2 s each, and `/gseq`
  delivers at that ~2 s spacing regardless of `every_ms`; on an app that is
  not redrawing, `/gseq` can time out ("grab timeout"). Rest/settle timing
  proofs need a window that presents on its own (the user's, or a visible
  unfocused one).
- `MAKEPAD=gpusim` builds the simulated-GPU backend (`cfg(gpusim)`,
  `platform/src/os/gpusim/`): a CPU raster that writes frames to files
  with no window, no Metal shader compile and no presentation. It is for
  logic and data-structure tests only. Never use it to prove a picture or to
  chase a rendering bug, and never build it into the shared `target/`
  (`CARGO_TARGET_DIR=target-gpusim`).
- Known remote hazards: a hidden-window click is occasionally lost
  (`Event::MouseDown` never arrives) — relaunch before debugging the widget;
  tick-sampled keys need `/k?k=down` … ≥150 ms … `/k?k=up`, a `press` lands
  between ticks; in the code map every `/g` drops keyboard focus, click the
  map before the next key batch; `MAKEPAD_HIDE_WINDOWS` is implemented only
  on macOS. Test instances of apps with audio or a shared home run with
  `SANDBOX_MUTE=1` and their own `SANDBOX_HOME` / `--state-dir`.

## Coordinate and lifecycle details

- Rectangles are layout points, window-local, with Y increasing downward.
  Window `sz` is logical size; `px` is physical pixels. No DPI arithmetic
  is needed to turn a widget rectangle into a click.
- Window ids are stable slots. A human-closed window reports
  `window N closed by user`; its closure is not a crash or an invitation
  to relaunch.
- Remote input is injected through the app event loop and does not need OS
  focus. Remote windows have a `[remote]` title suffix by default;
  `--remote-title-tag=NAME` customizes it.
- Grabs read the app's own drawable. Files are stored under the startup
  grab directory, with a bounded retained history per window.
- Backend support varies. Inspect the current implementation and response
  rather than assuming every platform supports readback.
- The Studio websocket protocol remains an internal implementation detail;
  agent inspection uses the standalone HTTP surface.

The existing [remote smoke script](../../tools/remote_smoke.sh) exercises
the protocol across example apps. Inspect its current launch/setup behavior
before using it for a task.

For design feedback and styling, see [Tweaker](tweaker.md).
