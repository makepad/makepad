# Makepad Agent Runbook

Repository-wide rules. Read the linked references when the task needs them;
use current source for API signatures and working examples.

## Local work and documentation

- Plans go in `local/plans/<topic>.md`.
- Other task design documents and reports go in `local/agent_state/<topic>/`.
  Keep these local; do not publish them as hosted pages or artifacts.
- Keep reusable repository documentation in tracked files. Do not make the
  workflow depend on helper scripts or state that may disappear with `local/`.
- When adding an example crate, update its Cargo workspace and
  `makepad.splash`.
- Prefer `rg` / `rg --files` for source searches. Check existing patterns in
  `widgets/src/`, `code_editor/`, and `apps/studio/` before changing Splash syntax.
  The archived `old/` tree is not the reference for current widget APIs.

## Builds and runtime verification

- Use printf-style debugging (`log!`, `eprintln!`, or equivalent tracing).
  Do not launch or attach a debugger such as LLDB or GDB; debugger access
  triggers system permission popups.

- Use release builds for runtime validation, profiling, benchmarks, and
  timing checks unless the user explicitly requests debug.
- Build with `cargo build --release -p <package>` from the package's owning
  workspace, then launch the resulting standalone executable from this
  checkout. Check the target directory and resource working directory;
  some apps have their own workspace.
- Use the WM’s Cargo launch path for its hosted apps: `cargo run --release`
  builds each app on demand, and the WM shows compilation while it starts.
  Do not collect prebuilt app binaries for a WM session.
- Outside that WM workflow, do not use `cargo run`, `cargo makepad`, or the
  Studio remote bridge (`ObserveMount`, `RunItem`, websocket clients) for UI inspection.
- After UI/runtime changes, rebuild and relaunch before drawing conclusions.
  A successful build/check alone does not verify UI behavior.
- Exercise the relevant interaction and inspect logs; capture a frame when
  visual verification is needed. Avoid unrelated or routine captures.
- Command-line builds, tests, linting, and file operations run directly in
  the shell.

## App ownership, focus, and screenshots

- Launch any app you intend to inspect or drive with `--remote`.
- Never stop, drive, or replace an instance the user is running. Before a
  fresh launch, gracefully close only older instances you launched.
- Remote windows stay visible but unfocused. Do not activate them or use
  `MAKEPAD_FOCUS=1` unless the user explicitly asks to bring one forward.
- Subagent verification runs use `MAKEPAD_HIDE_WINDOWS=1 <bin> --remote`.
  Only the main session opens a visible inspection window; avoid duplicates.
- Capture only the app's own drawable through `/g`, `/gq`, `/tweak/grab`,
  or an app-provided capture hook. OS/window/display screenshots are forbidden.
  If native chrome or another app matters, ask the user for an image.
- A `user closed` log entry means the human dismissed the window. Do not
  interpret it as a crash or relaunch it.
- Finish test sessions with `GET /gq` (grab and quit). If grabbing is
  unavailable, use `/close` and/or `/quit`. Verify your process exits;
  only fall back to stopping its exact owned PID if graceful cleanup fails.
- The exception is an app the user explicitly asks to keep open for them:
  hand off that instance and close any separate test/capture instances.
  Otherwise, no process you launch may outlive the task.

## Remote control quick start

1. Build the release binary and launch your own instance with `--remote`.
2. Read its startup line for port, PID, and grab directory.
3. Fetch `GET /` for the running binary's current protocol.
4. Use `/snap?q=...` to find widget rectangles, then inject input.
5. Inspect results through snapshots/logs and, when needed, `/g`.
6. Finish with `/gq` and confirm shutdown.

Coordinates are window-local layout points; no DPI conversion is needed.
Add `wait=1` to input requests to wait for the resulting frame.

Read [App remote control](docs/agents/app-remote.md) for routes and examples,
or [Tweaker](docs/agents/tweaker.md) for live styling and source write-back.

## Threading and realtime ownership

- The UI thread never takes a `Mutex`, `RwLock`, or `Condvar` another thread
  can hold, and never waits on a channel.
- UI-to-worker/audio commands use bounded, non-blocking sends. Report a
  full queue and retain/retry the command on a subsequent frame.
- Workers/audio publish snapshots through atomics, a triple buffer, or a
  channel consumed with `try_recv`.
- Large payloads (PCM, stems, grids, images) travel as `Arc` through
  channels. Return replaced payloads so the UI disposes of them; never
  perform their final drop on a realtime callback.
- A realtime audio callback owns its state, does not allocate on its hot
  path, and never takes a lock the UI or a worker can hold.
- Use one mechanism on native and wasm. Do not retain a desktop shared-lock
  path alongside a wasm workaround. UI/audio threads must not use
  `Atomics.wait` or spin-wait fallbacks.
- `lock_from_ui` is allowed only for state provably touched by the UI alone.
- Do not spawn a temporary thread for each job. Use `cx.thread_spawner()`,
  the pool TaskHandle API, or a long-lived platform worker fed by a channel.

## Splash and shader essentials

- Use `script_mod!` and current widget APIs. For object properties use
  `name: value`; for named widget instances use `name := Type{...}`.
  Module assignments such as `mod.widgets.Name = ...` are valid.
- Merge typed properties with `+:` when preserving inherited fields:
  `draw_bg +: {color: #f00}`. Use typed layout values such as
  `padding: Inset{left: 10}` and `align: Align{x: 0.5 y: 0.5}`.
- Use `theme.*`, `instance(...)` for per-draw values, and `uniform(...)`
  for values shared by a shader's instances.
- Use `#(expr)` for Rust interpolation. In Rust macros, use the `#x`
  color prefix when a digit followed by `e`/`E` would trigger exponent
  tokenization, e.g. `#x1e1e2e`.
- Register widget/component modules before UI that uses them. Match the
  current app's startup and lookup signatures rather than copying old ones.
- Custom draw shaders need `#[repr(C)]`. Put non-instance fields BEFORE
  the `#[deref]` draw base, and only shader instance fields AFTER it.
  The instance buffer is read contiguously; incorrect order corrupts it.
- Prefer enum `match` with a catch-all in shaders. If supported enum
  matching fails, add a compiler regression case and fix the compiler
  instead of replacing it with integer-like `if/else` chains.

Read [Splash and widget reference](docs/agents/splash.md) for examples,
registration, templates, and links to the implementations.
