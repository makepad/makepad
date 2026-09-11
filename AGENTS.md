# Makepad Agent Runbook

Repository-wide rules. Read the linked references when the task needs them;
use current source for API signatures and working examples.

## Current delegation context

- HIERARCHY (user, 2026-09-11, supersedes 2026-09-10): FABLE does the work,
  CODEX reviews, GROK runs the scripted proofs.
- Fable (the supervisor session and its Fable forks) designs and BUILDS
  everything: renderers, shaders, worker pipelines, layout engines, crate
  geometry, memory/lifetime contracts, diagnose-and-fix of deep bugs. One
  Fable fork per complex step, partitioned by file ownership; the supervisor
  writes the briefs, sequences the forks, reads the reports, integrates,
  commits with explicit paths and pushes, and relaunches the user.
- Codex / Astra does CODE REVIEWS only (read-only, ephemeral): adversarial
  review of a diff or a design of record against the laws and the acceptance
  criteria, findings back as a report. Codex never edits the tree and never
  builds a lane.
- Grok does the scripted proofs and small mechanical work: hidden-instance
  walks, grabs, `/gseq` sequences, acceptance runs, extractions, renames,
  reference sweeps. A proof lane writes observations; a Fable fork acts on
  them; Grok proves again. Never bundle "prove + fix" into one lane.
- Keep this hierarchy in the context of Studio flows and their root agents,
  including when resuming an archived lane. The user's later directions can
  change these roles for a particular task or flow.

## Local work and documentation

- Plans go in `local/plans/<topic>.md`.
- Other task design documents and reports go in `local/agent_state/<topic>/`.
  Keep these local; do not publish them as hosted pages or artifacts.
- Keep reusable repository documentation in tracked files. Do not make the
  workflow depend on helper scripts or state that may disappear with `local/`.
- When adding an example crate, update its Cargo workspace and
  `makepad.splash`.
- Prefer `rg` / `rg --files` for source searches. Check existing patterns in
  `widgets/src/`, `code_editor/`, and `apps/director/` before changing Splash syntax.
  The archived `old/` tree is not the reference for current widget APIs.

## Software installation requires explicit approval

- NEVER install, upgrade, bootstrap, or download and run external software
  without the user's prior explicit approval for that software and installation.
  This includes tools, applications, runtimes, SDKs, plugins, package-manager
  installs, and third-party tools compiled from downloaded source.
- This rule applies equally to system-wide, user-local, virtual-environment,
  repository-local, and temporary installations. Putting an executable in
  `local/`, `/tmp`, or `~/.local/`, or avoiding administrator privileges, does
  not make it exempt. Compiling a third-party tool for local use counts as
  installation even without a package manager.
- A feature request, permission to build/test, or a missing dependency is NOT
  installation approval. Before installing, explain the software and version,
  source, installation location, purpose, and commands or system changes, then
  wait for explicit approval. Do not silently add a required external runtime
  tool to Studio or another app as a workaround.
- Use already-installed tools and normal builds of this repository where
  possible. If additional software is needed, leave installation pending and
  explain the limitation; continue independent work. Approval already given
  for the specific installation remains valid within its stated scope.

## Commit content and Studio iteration history

- Keep AI-generated Markdown, plans, reports, scratch helpers, logs, recordings,
  captures, build output, and other temporary artifacts out of commits. Existing
  instruction files such as `AGENTS.md` are the Markdown exception. Preserve
  existing tracked documentation and incoming human/external changes; do not
  blanket-delete files or ignore every Markdown path.
- Run the existing repository tests for validation. Do not add generated test
  files, inline test code, or test scaffolding unless the user explicitly requests
  that change. Preserve existing tests; do not remove or weaken them to pass.
- Studio's source hierarchy is `local` → `work` → `dev`: `local` records every
  build iteration, `work` contains coherent feature commits, and `dev` contains
  lower-frequency, validated milestones and incoming external PRs.
- NEVER push `local`, its private flow branches, or checkpoint/archive refs.
  Never merge private checkpoint ancestry into a public branch. Promote only
  by squash from `local` into `work`, then squash feature groups from `work`
  into named `dev` milestones. Fetch/sync incoming `work` and `dev` changes
  without rewriting published history or discarding unrelated work.
- For Studio-managed flow builds, use this order: platform build checks,
  rustfmt on changed Rust, repeat checks if formatting changed the source,
  commit the exact eligible source to `local`, release binary build, existing
  native tests, then launch. Record the checkpoint hash with the binary and
  its evidence. Reuse bounded worktrees; never create one per build.
- Agents may code the next revision while its previous app is running. The
  next compilation/check waits until the person closes that flow's app and
  Studio observes its exit. Standalone evaluations obey the same gate.
- A validated revision requires `cargo check` for its supported platforms with
  zero warnings/errors, the existing native tests on the current host, and a
  release build/runtime check when applicable. Use the repository's actual
  package/target support matrix. Missing SDKs, runners, or required tests are
  blocked coverage, not passes. Do not suppress warnings to obtain a green gate.
- Validate the exact resulting source for each feature/milestone promotion so
  `dev` remains useful for bisecting. Public squash/push operations must expose
  their source, destination, included changes, validation, and conflicts.
- Agents working in Studio flows must also follow
  [Director flow instructions](apps/director/AGENTS.md). This covers todo deltas,
  terminal image delivery, test ownership, recordings, and revision feedback.

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
- Rendering is verified on the real GPU backend, never on the headless
  raster (user, 2026-09-11: "chasing bugs in headless is useless"). Any
  picture, pixel, outline, colour, LOD, tile or frame-timing question is
  answered with an owned `--remote` instance of the release build on the
  native backend (Metal here) and `/g` / `/gseq` grabs, compared in RGB.
  `MAKEPAD=headless` suites are for logic and data-structure tests only
  (layout, budgets, orderings, parsers); a headless raster gate never
  stands in for a GPU proof and is never used to diagnose a rendering bug.
  On macOS a hidden window (`MAKEPAD_HIDE_WINDOWS=1`) does not present
  frames: it proves shader compilation (`[E] Metal shader` count) and log
  behaviour, not pictures. For a pixel proof launch the instance visible,
  unfocused, small, off to the side, and close it with `/gq`.

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
