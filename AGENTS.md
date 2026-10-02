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
  `widgets/core/src/`, `widgets/families/`, `code_editor/`, and `apps/` before changing Splash syntax.
  The archived `old/` tree is not the reference for current widget APIs.

## Current agent workflow

- Opus 5.5 does all work that takes judgement: implementation lanes, reviews
  of every lane's diff, research and design.
- Grok handles only hyper-mechanical work (bulk renames, repetitive edits,
  transcription, log sweeps) under precise briefs.
- Codex, Sol and Fable are not used for now.
- Resume a finished lane with its context for follow-ups instead of starting
  a fresh one that repays the same input. Idle without polling while waiting.
- This is the user's current workflow (2026-09-27) and supersedes older role
  assignments in local skills or memories (including the 2026-09-16 one).

## Sending your changes to Makepad

People who installed Makepad apps with the Makepad Builder change them with
their own coding agent. When the person asks you to "send what I changed",
"share my changes with Makepad" or similar, write a change report and send
it only with their approval. The format, limits and anonymisation rules are
in [Change reports](docs/agents/change-report.md); the steps:

1. Find the installation. This checkout is `builder/sources/<release>/makepad`
   inside it (app repositories at `makepad/apps/<name>`); the installation
   folder holds `makepad-builder.exe` (Windows) or the `makepad` command
   (macOS, Linux). `builder/installed/<app>.json` names the app (`"id"`) and
   the release its source came from (`"release"`, `"repositories"`). Below,
   `BUILDER` is `<installation>\makepad-builder.exe` or
   `<installation>/makepad`.
2. Collect the changes: `BUILDER changes <app>` prints the diff of the app's
   source against that release (`--files` lists the files). Edits saved by
   an earlier update are in `builder/changes/<app>-<date>.diff`. With git
   available, `git status` and `git diff HEAD` in each repository give the
   same. Skip build output and anything unrelated to the app.
3. Write the list as concepts, not code: per change a one-line title, a
   kind (fix, feature, tweak, ui, performance, refactor, docs, other), what
   is different for someone using the app and why, and the screens or
   components it touches. Add a tiny snippet only when a change cannot be
   told without it.
4. Anonymise everything that will be sent: replace names and usernames,
   email addresses, host names and private IP addresses, absolute and home
   folder paths (use repo-relative paths), keys, tokens and passwords,
   private service URLs and personal data in strings or comments with
   `[name]`, `[email]`, `[host]`, `[path]`, `[secret]`, `[private-url]` or
   `[personal]`, and count what you took out, by kind.
5. Show the person the list and what was taken out, ask whether to include
   a trimmed diff (default: no) and whether Makepad may reply by email, and
   wait for their explicit approval or edits. Never send without it.
6. Write `builder/changes/<app>-report/` with `report.json` (schema:
   `docs/agents/change-report.schema.json`), `REPORT.md` (the same list for
   people) and, only when they opted in, `changes.diff`. `BUILDER
   send-changes <app>` checks it (schema, 256 KB limit, the anonymisation
   scan) and prints what would be sent; fix what it reports.
7. Send it: `BUILDER send-changes <app> --yes`, adding `--with-email` only
   when the person wants a reply (the Builder adds the address it is logged
   in with; you never need it). Or leave the report for them to read and
   send in the Builder (`e` on the app's row). Without the Builder, POST the
   zip: `curl -sS -X POST -H 'X-Makepad-Feedback: 1' -H 'Content-Type:
   application/zip' --data-binary @report.zip
   https://makepad.nl/api/feedback/changes`.
8. Tell the person the report id the Builder (or the server's
   `{"ok":true,"id":N}`) returned.

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

- NEVER create or publish additional public branches in `makepad/makepad`.
  Use only the established `local` → `work` → `dev` workflow. `local` remains
  private and must never be pushed. Feature, test, release, packaging, and
  distribution work do not authorize another public branch; using a separate
  checkout or worktree does not change this rule.
- Keep AI-generated Markdown, plans, reports, scratch helpers, logs, recordings,
  captures, build output, and other temporary artifacts out of commits. Existing
  instruction files such as `AGENTS.md` are the Markdown exception. Preserve
  existing tracked documentation and incoming human/external changes; do not
  blanket-delete files or ignore every Markdown path.
- NEVER commit third-party source snapshots (`cargo vendor` output, copied
  crates, SDK trees), model weights, datasets, media dumps, or any other bulk
  import. GitHub keeps every pushed blob, so one such commit bloats every
  clone of the repository forever. Dependencies come from crates.io or from
  in-repo `libs/` ports; offline mirrors live outside the tree. Read
  `git diff --stat` before every commit and stop on paths you did not write;
  a chain that carries such content is rewritten before it is pushed, never
  fixed with a follow-up delete.
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
- Agents may code, check, and build the next revision while its previous app
  is running. When the replacement is ready, gracefully close and restart
  that workflow's app without asking the user to close it. Verify the old
  process exits and launch the replacement with the same workspace and state.
  This applies to Studio flows and standalone evaluations. An app being open
  is not a reason to defer a build, ask for permission, or require the user
  to close it.
- A validated revision requires `cargo check` for its supported platforms with
  zero warnings/errors, the existing native tests on the current host, and a
  release build/runtime check when applicable. Use the repository's actual
  package/target support matrix. Missing SDKs, runners, or required tests are
  blocked coverage, not passes. Do not suppress warnings to obtain a green gate.
- Validate the exact resulting source for each feature/milestone promotion so
  `dev` remains useful for bisecting. Public squash/push operations must expose
  their source, destination, included changes, validation, and conflicts.
- An app with its own `AGENTS.md` (including the private products in the
  commercial repository checked out at `apps/commercial`) adds flow
  instructions for agents working in it; follow them alongside these.

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
- Build profiles live in the root `Cargo.toml`: dev keeps line tables only,
  release is incremental without LTO. cargo-makepad's packaging commands
  build release non-incrementally.
- The private commercial repository (`makepad/commercial`: Stage with Amp,
  Scope, Sandbox) is cloned into `apps/commercial` and has no workspace of
  its own: its root `Cargo.toml` is a package whose `cfg(any())` path
  dependencies make Stage's and Scope's crates members of this workspace
  (matched by `apps/*/src/..`), so they share its profiles, patches and
  `target/`, and every Makepad crate compiles once for all apps. Sandbox
  (`apps/commercial/sandbox`) is its own workspace, excluded here. Nothing is
  required when the clone is absent. `Cargo.lock` is not committed.
- Reference videos from the web (YouTube, X posts, most pages with a video):
  `./target/release/makepad-stage-web-grab <url> --every 0.5 --dir <folder>
  --launch <scratch>` grabs one through Stage's web grab in its own hidden,
  muted Stage (H.264-only sites such as X via the media file behind the page)
  and writes the clip, provenance and frames. See
  `apps/commercial/stage/WEB-GRABS.md`.
- The parallel rustc frontend is a local opt-in, never for CI or shipped
  builds. It needs `RUSTC_BOOTSTRAP` on stable and roughly halves a clean
  dev build. Put it in the user config, `~/.cargo/config.toml`, so every
  build on the machine gets the same flags (a flag difference rebuilds
  every crate); do not set it per shell with `RUSTFLAGS`:
  `[env]` `RUSTC_BOOTSTRAP = "1"` and `[build]` `rustflags = ["-Zthreads=8"]`.
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
- Never pass a variable that could be empty or unset to `rm` (or `rm -rf`),
  e.g. `rm -rf "$DIR/"*` or `rm -rf $TMP/build`: an empty value turns it into
  a delete at `/` or `$HOME` and raises a macOS permission popup for the
  user. Delete literal, absolute paths inside the repo or scratchpad, or
  guard first (`: "${DIR:?}"` / `[ -n "$DIR" ] || exit 1`).
- Rendering is verified on the real GPU backend, never on the gpusim
  raster (the CPU simulated-GPU backend, `MAKEPAD=gpusim`, formerly called
  "headless"; user, 2026-09-11: "chasing bugs in headless is useless"). Any
  picture, pixel, outline, colour, LOD, tile or frame-timing question is
  answered with an owned `--remote` instance of the release build on the
  native backend (Metal here) and `/g` / `/gseq` grabs, compared in RGB.
  `MAKEPAD=gpusim` suites are for logic and data-structure tests only
  (layout, budgets, orderings, parsers); a gpusim raster gate never
  stands in for a GPU proof and is never used to diagnose a rendering bug.
  The window need not be visible: a hidden instance (`MAKEPAD_HIDE_WINDOWS=1`)
  on the native backend is the normal pixel-proof rig; `/g` forces a present
  (about 2 s per grab on a hidden window). Only rest/settle timing proofs
  need a window that presents on its own. Close it with `/gq`.
- Apps that embed the AI chat build only its CLI/MCP/cloud backends. The
  local model runtimes (Qwen on Metal/CUDA, the metallib and kernel builds)
  come in through the app's own `localai` cargo feature
  (`cargo build --release -p <app> --features localai`, or in a Builder
  catalog entry's `features`). It is on by default only where the app's own
  job runs local models: `apps/ai-hub`, route and files.

## App ownership, focus, and screenshots

- Launch any app you intend to inspect or drive with `--remote`.
- Do not drive or stop unrelated user instances. For an app in the active
  development workflow, close and restart it when the replacement is ready;
  no separate user-close confirmation is required. Before a fresh launch,
  gracefully close the previous workflow instance and verify its exit.
- Remote windows stay visible but unfocused. Do not activate them or use
  `MAKEPAD_FOCUS=1` unless the user explicitly asks to bring one forward.
- User interaction with an app in the active development workflow does not
  suspend agent testing or require a handoff. Continue inspecting, driving,
  closing and restarting that app as needed, preserving its workspace and
  saved state. This is the user's standing authorization (2026-09-22).
- Read `/activity` and use `if_user_seq` for each bounded input sequence.
  A changed counter or HTTP 409 invalidates that sequence's evidence, not
  authorization to continue: inspect whether the action was applied, read a
  fresh counter and retry or restart the sequence without asking permission.
  Never replay an already-applied toggle or edit blindly. See
  [App remote control](docs/agents/app-remote.md).
- Subagent verification runs use `MAKEPAD_HIDE_WINDOWS=1 <bin> --remote`.
  Only the main session opens a visible inspection window; avoid duplicates.
- CEF apps keep their browser profile (cookies, logins) at
  `$HOME/.makepad-cef/<executable name>` unless `MAKEPAD_CEF_PROFILE_DIR`
  names one, so a renamed or hash-pinned binary silently starts empty. When
  replacing a user's CEF app, launch it with `MAKEPAD_CEF_PROFILE_DIR` set to
  the profile the user's instance was already using. Hidden native tests set
  their own distinct test profile. Never run two instances on the user's
  profile at once. `MAKEPAD_HOME` and asset roots are separate state; keep
  them as they were.
- Capture only the app's own drawable through `/g`, `/gq`, `/tweak/grab`,
  or an app-provided capture hook. OS/window/display screenshots are forbidden.
  If native chrome or another app matters, ask the user for an image.
- A `user closed` log entry means the human dismissed the window, not a crash.
  Continue the active workflow, including launching a needed replacement,
  unless the user has asked to stop.
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

- The UI thread never waits on a channel or a `Condvar`, and takes a lock
  another thread can hold only when every holder keeps it briefly (see the
  spin rule below).
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
  path alongside a wasm workaround. The UI thread and the audio worklet never
  block or wait on long work. Brief spins on short locks (held only for a
  few field updates, never across I/O, parsing, evaluation or a job) are
  allowed; on wasm the optimiser's `waits` pass turns a contended
  `Atomics.wait` on those threads into such a spin, so plain `.lock()` is
  the one mechanism. Anything longer goes to a worker and returns through a
  channel.
- Do not spawn a temporary thread for each job. Use `cx.thread_spawner()`,
  the pool TaskHandle API, or a long-lived platform worker fed by a channel.

## Performance dynamics and limits

- Always carefully evaluate the performance dynamics a change has on the main
  UI loop: what now runs per frame, per event, or per item, how that cost
  scales with data size and view state, and what moves on or off the UI
  thread. State this in the change's report, with measurements for hot paths.
- Never add arbitrary limits (memory caps, CPU/time budgets, chunk or item
  counts, queue sizes, retry counts) that only surface as random failures
  when real data reaches them. It is better to keep working with a
  performance hiccup than to stop working.
- Where a bound is genuinely required, hitting it must degrade gracefully
  (queue, spill, split, slow down, or retry on a later frame) and never
  drop data, refuse input, or fail the operation; log when it happens.

## Platform changes stay application-neutral

- `platform/`, `widgets/`, `draw/`, and the other shared layers serve every
  app. A change made there to speed up or fix one particular application is
  not landed on the agent's own judgment: state the proposed platform change
  and the app that motivated it, and get the user's feedback and checks
  first.
- Never leak application specifics into these layers: no app names, app
  data shapes, app-only flags, or code paths that exist for one caller.
  Express the need as a general facility with a general name, or keep the
  code in the app.
- Prove a platform optimisation on more than the app that asked for it
  before it lands, and say in the commit which apps were checked.

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
