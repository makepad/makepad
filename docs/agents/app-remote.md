# App remote control

Use this reference when inspecting or driving a Makepad app. Ownership,
focus, and capture policy live in [AGENTS.md](../../AGENTS.md).

The app side is [platform/src/remote_core.rs](../../platform/src/remote_core.rs)
(the `COMMANDS` table); the client is [libs/remote](../../libs/remote). Run
`help` against your running instance for its current command list; it can
contain newer diagnostics than this guide.

## Launch and discovery

Build the package and the client in release mode, then launch the resulting
standalone binary with `--remote`. For a hidden verification run from the
repo root:

```sh
cargo build --release -p makepad-example-splash -p makepad-remote
MAKEPAD_HIDE_WINDOWS=1 ./target/release/makepad-example-splash --remote
```

Capture the launch output using your process tool. The startup line contains
everything needed to address and clean up that instance:

```text
[makepad-remote] pid=9931 app=makepad-example-splash socket=/tmp/makepad-remote/9931.sock
```

`--remote` (no value) or `MAKEPAD_REMOTE=1` opens a Unix-socket endpoint. No
TCP port exists and nothing listens on the network. `--remote=PORT`,
`--remote HOST:PORT` and `MAKEPAD_REMOTE=<port>` are refused (exit 2); other
machines are reached through the hub. Prefer the explicit launch flag for
agent inspection. On macOS `--focus` brings the app to the front as its
window opens; a binary launched from a terminal otherwise stays behind the
launcher.

Address the instance with the client:

```sh
P=9931   # the pid from your own launch's startup line
./target/release/makepad-remote app $P status
```

`makepad-remote app <pid|app-name|socket> <command> [k=v ...]`; `makepad
inspect <pid> <command>` speaks the same commands. The exit status is 1 when
the answer has an error line and 2 when the app is unreachable.

Do not discover an instance by taking over another running one. Retain your
own launch's PID and socket. On code changes, close that instance, rebuild,
and relaunch before inspecting again.

## Common commands

Each request is one command; the answer is the same JSON as before, one line
per answer. Tree dumps are text lines. Errors are an `{"err":...}` line.
Query parameters are `k=v` arguments with the same names; a value with spaces
is a quoted shell argument. There is no HTTP, no status code and no POST body.

| Command | Purpose |
|---|---|
| `help` | The command list as JSON lines |
| `status [w=ID]` | App/PID and window geometry; omit `w` to list windows |
| `grab [w=ID] [scale=0.5] [out=FILE]`, `gseq [n=8] [every_ms=50] [scale=0.5] [out=DIR]` | Capture the next presented frame, or N frames on request cadence (1-64, >=8 ms, <=60 s span); the app's `frames` service answers and the client writes the PNG files; prints JSON with `png` path(s) and per-frame `capture_ms`/`encode_ms` (`gseq.frames`); input `wait=1` remains a next-frame barrier |
| `gq [scale=0.5]` | Grab windows, then quit; returns `png` paths and `quit:1` |
| `snap [q=TEXT] [w=ID] [all=1]` | Widget ids/types/text and window-local rectangles |
| `dump [paths=1]` | Indented widget tree (plain lines) |
| `mouse k=click x=X y=Y` | Mouse input; kinds also include move, down, up, scroll |
| `click x=X y=Y` | Click alias |
| `key k=press c=KeyA` | Key input; down/up are also supported |
| `text t=TEXT`, `key t=TEXT` | Text/IME input |
| `drop path=ABSOLUTE_PATH x=X y=Y` | One file through native drag, drop, and drag-end events |
| `log [n=50] [since=N]` | App log tail; `n` in the reply is the latest sequence |
| `step [frames=K] [fps=60]` | Run K virtual-clock frames (needs `--virtual-clock`), see below |
| `settled` | Whether anything is still pending: `{"settled":bool,"reasons":[...]}` |
| `cap start path=ABS.mp4 [fps=60]`, `cap stop` | In-process mp4 capture of the window, see below |
| `cursor [show=1] [style=arrow]` | Synthetic cursor drawn into the frame |
| `close [w=ID]`, `resize w=W h=H`, `window k=...` | Window operations |
| `activity` | Native user activity counters, see below |
| `trace`, `midi`, `ai`, `ai transcript`, `handsoff`, `shader consts`, `shader const` | Diagnostics and overlays |
| `tweak ...`, `design <op> ...` | Design feedback and source edits, see [Tweaker](tweaker.md) |
| `quit` | Graceful shutdown without a final grab |

Images: `grab`, `gseq`, `gq` and `tweak grab` read the app's `frames`
service and the client writes the PNG files, by default to
`$TMPDIR/makepad-remote/<app>-<pid>/grab-w<window>-<seq>.png` on the client
machine (newest 64 per window kept). The command prints the JSON with the
`png` paths; `out=FILE` (grab) or `out=DIR` (gseq) chooses the location.
Metal/raw readbacks downsample before worker PNG encoding. There is no raw
PNG-to-stdout mode.

`snap` entries include `window_id` and `enabled`, plus `selected` when the
widget exposes a selection, alongside the compact `i`/`ty`/`r`/`w` fields.
When the app's widgets layer registers id paths (`Cx::widget_paths_callback`)
each entry also carries `p`, the dotted id path from the window root (named
widgets only, e.g. `main.new_note`), and `snap q=path:main.new_note` (or
`snap path=main.new_note`) matches a path exactly or as a suffix of whole
segments: `new_note` finds `main.new_note`, `note` does not (paths match
case-sensitively; the plain `q=` search does not). `dump paths=1`
lists every visible widget as `path type x y w h`.

Arguments are optional unless needed for the operation. Input commands
accept `w=ID` to target a window and `wait=1` to answer after the next frame.
With no window specified, commands generally use the first window.

Mouse buttons: `b=0` left, `b=1` right, `b=2` middle. Scroll takes `dx`
and `dy`. Add `hw=1` when testing the hardware pointer-lock/pin transform.
Keyboard modifiers are `shift=1`, `ctrl=1`, `alt=1`, and `cmd=1`.

The input parser also accepts long names such as `window`, `kind`, `button`,
`text`, and `code`. Quote values with spaces as ordinary shell arguments.

File drops require an absolute path (at most 4096 bytes) and finite,
nonnegative `x`/`y` inside the window. Optional parameters are `w`/`window`
and `wait=1`; duplicate or unknown parameters are rejected. The remote
thread sends only the path; the app owns validation and loading. The reply
reports `drop_handled` and `drag_response`, which confirm event handling,
not that an asynchronous file import has completed. For example:

```sh
makepad-remote app $P drop 'path=/absolute/path/reference car.png' x=300 y=400 wait=1
```

## Drive an owned instance

With `P` set to the pid your app printed:

```sh
makepad-remote app $P help
makepad-remote app $P snap q=press_demo
```

Read the returned `r: [x, y, width, height]`, calculate its center, and feed
that point to `click x=.. y=.. wait=1`. Do not reuse coordinates from another
window or an earlier layout.

```sh
makepad-remote app $P log n=20
makepad-remote app $P gq scale=0.5
```

Read a returned PNG with the local image viewer when visual inspection is
needed. Confirm the owned process exits after cleanup. Use `quit` if a
backend cannot grab; do not replace a failed grab with an OS screenshot.

## Deterministic runs and in-process capture

A scripted demo or a pixel test that must come out the same every time pins
the four inputs that otherwise vary between runs, at launch:

```sh
MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 ./target/release/APP --remote \
  --virtual-clock --window 360x640@3 --seed 1
```

- `--virtual-clock` (or `MAKEPAD_VIRTUAL_CLOCK=1`): the app's time
  (`seconds_since_app_start`, `Cx::time_now`, NextFrame and Draw stamps,
  pass uniforms, `start_timeout`/`start_interval` timers, and with them
  animators and the caret blink) starts at 0 and moves only through
  `step frames=K [fps=60]`, which runs exactly K frames of `1/fps` each:
  due timers, then NextFrame, then the draw, then the present. It answers
  `{"frame":N,"time":T}` after the K-th frame is presented (and captured,
  with `"captured":M`, while a capture runs). Input and grabs between steps
  see the app at the current time; nothing advances on its own. `Cx::time_now`
  reads 2026-01-01T00:00:00Z plus the virtual time. Without the flag nothing
  changes; `step` then refuses. The flag needs `--remote` (only `step`
  moves the clock); without it, it is ignored with a log line. Children an
  app spawns inherit `MAKEPAD_VIRTUAL_CLOCK`/`MAKEPAD_SEED`; prefer the flags.
  Timers fire once per frame at most: one a handler starts or re-arms fires
  on the next frame. Two injected clicks with no step between them carry
  the same timestamp (a double click); step the frames a person would take.
  OS input and a few widgets that read the wall clock directly are not on
  the virtual clock.
  Every `step` answers within a bound: a frame that cannot be presented,
  or a capture encoder that falls behind or loses frames, fails the step
  with an error after 20 s; a failed capture is reported once and later
  steps go on uncaptured.
- `--window WxH@scale`: the first window's logical size and dpi scale,
  applied before its first frame (hidden windows too, and not fitted to the
  displays). `status` then reports `sz:[W,H]` and `px:[W*scale,H*scale]`. The
  first window applied claims it. Off macOS the native size assumes a display
  at scale 1, so check `status` there; and only macOS stops its paint beat for a
  NextFrame waiting on `step` (other backends keep ticking, harmlessly).
- `--seed N` (or `MAKEPAD_SEED=N`): Splash `random`/`random_u32` start from
  N instead of the wall clock.
- `settled` answers `{"settled":bool,"reasons":[...]}`: `next_frame` (an
  animator or anything else waiting on a frame), `redraw`, `repaint`,
  `timer` (a virtual timer due within the next frame), `step` (one is
  running), `shaders` (Metal pipelines still compiling), `loads`
  (asynchronous work whose result will change a frame, counted by kind in
  `"loads":{"image":2,...}`: image decodes and fetches, glyph rasters,
  Splash resources; subsystems and apps add kinds with
  `Cx::register_async_load_probe`). Step until it settles rather than
  sleeping.
- Loads run on workers and the network, off the virtual clock, so under
  load a picture can land a frame earlier or later from run to run.
  `step frames=K wait_loads=1` opens no frame while loads are in flight
  (each wait bounded at 30 s; then the step fails naming them, e.g.
  `image=1`), which makes such runs deterministic.
- `cap start path=/ABS/file.mp4 [fps=60] [audio=1] [overwrite=1] [w=ID]`
  starts an in-process H.264 recording of the window at native pixels. The
  directory must exist and an existing file needs `overwrite=1`; both are
  checked before the capture starts. `cap stop`
  answers once the file is finalized, with `frames`, `sz`, `missing` and a
  `hash` of the raw frames (`hashes=1` adds one per frame). Under the virtual
  clock exactly one frame per `step` frame is written, at `pts = n / fps`
  counted from the first captured step; a `step` at another fps than the
  capture's is refused. Frames presented for input or grabs between steps
  are not written. Without the virtual clock, every presented frame is
  written at its wall time, and frames the encoder cannot keep up with are
  dropped (counted in `missing`). `audio=1` adds the app's own output (the
  audio tap) on the frames' sample clock, padded with silence; the device
  runs in real time, so under the virtual clock only the last 0.1 s the
  device played is kept per frame and the sound is not deterministic (run
  with `SANDBOX_MUTE=1`). Quitting, or the app closing, with a capture open
  finishes the file. `cap pause` and `cap resume` stop and restart
  writing without stopping the clock: while paused, `step` frames advance
  the app but are not written (nor is sound), and the file stays gapless, as
  pts count written frames (`{"ok":1,"paused":1,"frames":N}` /
  `{"ok":1,"resumed":1,"frames":N}`, N written so far; an error without a
  capture). In wall-clock mode the paused time comes off the file's clock.
  The red hands-off frame and ScreenCap's REC dot are
  never in the file; the tweaker overlay is, if it is on.
- `cursor show=1 [style=arrow|hand|text|crosshair|app]` draws a synthetic
  pointer into the window's frame (so grabs and captures show it), moved by
  every injected mouse event, with a ripple after each press (0.4 s of app
  time). `x=&y=` places it without an event; `show=0` hides it. Off by
  default.

A typical run: launch as above, check `status`, `cursor show=1`,
`cap start path=...`, `step frames=30`, then for each action locate the
target with `snap q=path:...`, inject it, and `step` the frames it should
take; `cap stop`; `quit`.

## GPU runs, hidden windows, and the headless backend

- The proof rig for anything visual is the native GPU backend (Metal on
  macOS) in an owned `--remote` instance. `MAKEPAD_HIDE_WINDOWS=1` keeps its
  windows off the user's screen; grabs and input still work because `g`
  forces a present on a hidden window. Measured on macOS (2026-09-10): the
  first hidden `g` answers in ~100 ms, later ones in ~2 s each, and `gseq`
  delivers at that ~2 s spacing regardless of `every_ms`; on an app that is
  not redrawing, `gseq` can time out ("grab timeout"). Rest/settle timing
  proofs need a window that presents on its own (the user's, or a visible
  unfocused one).
- `MAKEPAD_HAL_BACKEND=headless` runs the HAL headless backend: the native
  client with no window, no GPU and no raster. It is for logic and
  data-structure tests only. Never use it to prove a picture or to chase a
  rendering bug.
- Known remote hazards: a hidden-window click is occasionally lost
  (`Event::MouseDown` never arrives) — relaunch before debugging the widget;
  tick-sampled keys need `key k=down` … ≥150 ms … `key k=up`, a `press` lands
  between ticks; in the code map every `g` drops keyboard focus, click the
  map before the next key batch; `MAKEPAD_HIDE_WINDOWS` is implemented on
  macOS and Windows. Test instances of apps with audio or a shared home run with
  `SANDBOX_MUTE=1` and their own `SANDBOX_HOME` / `--state-dir`.

## Coordinate and lifecycle details

### Sharing an instance with a person

Read `activity` before driving an app. `status` includes the same `activity`
object. It reports `user_active`, the monotonic `user_seq`, `idle_ms`, the
two-second `quiet_ms`, whether input is `held`, and the last input's kind and
window. It never exposes typed text, key values or pointer coordinates.
Only native input advances the counter; remote and Studio injections are marked
remote, including hardware-path mouse injection and file drops. Focus, layout
and paint notifications do not count as interaction.

Carry `if_user_seq=N` from the beginning of an automation sequence on every
mutating input command. A command without it is gated by the quiet period alone;
with it, the command is also refused once the person has interacted since N.
Input, window changes, closing/quitting, tweaker mutations, AI overlay
mutations and shader patches are refused with an `{"err":"user_interacting"}` or
`{"err":"user_intervened"}` line (with `"applied":false`) if the person is
active or the counter differs. This check happens on the UI thread immediately
before dispatch. Queued mutations that outlive their request deadline expire.
Read-only status, snapshots, logs and grabs remain available.

User input does not revoke authorization to test or restart an app in the active
development workflow (user instruction, 2026-09-22). If input interrupts a
sequence, discard its interrupted evidence, inspect the reply's `applied` flag
and current state, then read a fresh counter and continue without requesting a
handoff. Wait for the protocol's quiet period when it refuses an input request;
do not replay an already-applied toggle or edit blindly. Graceful close and
replacement launches remain authorized. `gq` also rechecks its counter after
its grabs; if it refuses to quit, inspect the state and retry with a fresh
counter. These permissions apply to the current workflow's app, not unrelated
user instances.

Answers carry no user-sequence headers. Read `activity` before a bounded
sequence and again after it: a changed counter means the person interacted
during it; compare the ending counter with the sequence's original counter
before attributing a capture to your test. A `wait=1` command interrupted before
its frame acknowledgment answers `user_intervened` with `applied:true`; its action already
ran, so retrying it could duplicate an effect. A command refused before dispatch
answers `applied:false`.

- Rectangles are layout points, window-local, with Y increasing downward.
  Window `sz` is logical size; `px` is physical pixels. No DPI arithmetic
  is needed to turn a widget rectangle into a click.
- Window ids are stable slots. A human-closed window reports
  `window N closed by user`; its closure is not a crash. Launch a replacement
  when needed for the active workflow, unless the user has asked to stop.
- Remote input is injected through the app event loop and does not need OS
  focus. Remote windows have a `[remote]` title suffix by default;
  `--remote-title-tag=NAME` customizes it.
- Grabs read the app's own drawable. Files are stored under the startup
  grab directory, with a bounded retained history per window.
- Backend support varies. Inspect the current implementation and response
  rather than assuming every platform supports readback.
- The Studio websocket protocol remains an internal implementation detail;
  agent inspection uses the Unix-socket `makepad-remote` endpoint.

The existing [remote smoke script](../../tools/remote_smoke.sh) exercises
the protocol across example apps. Inspect its current launch/setup behavior
before using it for a task.

For design feedback, styling and structural source edits (`design <op>`),
see [Tweaker](tweaker.md).
