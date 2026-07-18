# Makepad Web Phase 1a Observability Design

## Goal

Make the existing public Web performance snapshot identify whether a pump is dominated by Rust/WASM work, JavaScript/WebGL dispatch, or CPU-to-GPU transfers. Keep this dependency-free, per-pump, and additive so existing `pump_ms` and `draw_calls` consumers continue to work.

## Scope

Phase 1a changes only `platform/src/os/web/web.js`, `platform/src/os/web/web_gl.js`, and host-runnable source-contract tests under `platform/tests/`.

It does not add Rust-to-JavaScript metric messages, histograms, GPU timer queries, bind counters, browser automation, or changes to `PerformanceStats`. Rust `PerfMonitor`/`PerfGraph` web integration is a separate Phase 1b goal because it answers frame-history questions rather than per-pump backend attribution.

## Considered Approaches

### 1. Extend the existing JavaScript snapshot — selected

`WasmWebBrowser` already resets backend counters before each pump, publishes a snapshot after dispatch, exposes `window.makepad_get_perf_snapshot()`, retains the last active sample, and owns an opt-in HUD. `WasmWebGL` already inherits hook names for reset, snapshot, and HUD formatting but does not implement them.

Extending these existing seams adds no protocol or framework and gives immediate attribution with a small diff.

### 2. Also bridge metrics into Rust `PerfMonitor`

This would make `PerfGraph` useful on Web, but it creates a second deliverable with different semantics: rAF frame history and Rust draw/GC channels. It should follow as Phase 1b rather than complicate this per-pump snapshot.

### 3. Add a full profiler or asynchronous GPU timing

Histograms, trace events, `EXT_disjoint_timer_query_webgl2`, and a browser harness provide deeper answers but are unnecessary until basic transfer and dispatch attribution shows a remaining blind spot.

## Public Snapshot Contract

The existing top-level fields remain unchanged and two additive timing fields are introduced:

```js
{
  pump_ms: Number,
  wasm_ms: Number,
  dispatch_ms: Number,
  draw_calls: Number,
  backend: Object | null,
  last_active: Object | null,
}
```

- `pump_ms`: existing total duration of `do_wasm_pump`; includes setup and cleanup.
- `wasm_ms`: time spent inside `wasm_process_msg`, including the WASM export and bridge wrapper work.
- `dispatch_ms`: time spent in `from_wasm.dispatch_on_app`; includes JavaScript message dispatch and synchronous WebGL/DOM calls, but excludes `from_wasm.free()`.
- `draw_calls`: existing decoded draw-command count, including commands skipped while a shader is unavailable.

All values are per pump, not necessarily per animation frame. Browser clock precision may make very short samples zero.

`last_active` retains the same top-level timing/draw/backend fields as the last pump that performed rendering or backend work. Existing fallback behavior and exception cleanup remain unchanged.

## WebGL Backend Contract

`WasmWebGL` implements the three hooks already consumed by `WasmWebBrowser`:

```js
reset_backend_perf()
get_backend_perf_snapshot()
format_backend_perf_hud()
```

The per-pump backend snapshot is:

```js
{
  passes: 0,
  draw_commands: 0,
  submits: 0,
  uniform_write_calls: 0,
  uniform_write_bytes: 0,
  buffer_write_calls: 0,
  buffer_write_bytes: 0,
  texture_write_calls: 0,
  texture_write_bytes: 0,
}
```

`get_backend_perf_snapshot()` returns a fresh plain object so a retained snapshot is not changed by the next reset.

### Counter Semantics

- `passes`: one per dispatched canvas or render-texture pass.
- `draw_commands`: one per valid decoded `CMD_DRAW`, matching the existing `draw_calls` location.
- `submits`: one after each successful `gl.drawElementsInstanced`; XR eye submissions count separately.
- Uniform writes: actual `bufferData` or `bufferSubData` calls to `UNIFORM_BUFFER`, counted after cache skips, with source byte length.
- Buffer writes: actual `bufferData` or `bufferSubData` calls for array/index buffers, with source byte length.
- Texture writes: WASM-backed `texImage2D` calls and their source bytes. Cube faces count as six calls.

Render-target allocations with `null` pixels are not transfer bytes. Browser-owned video-frame payload sizes are unknown and remain uncounted. These can receive separate counters later if measurements make them important.

## Timing Data Flow

`do_wasm_pump()` records four `performance.now()` readings around the existing boundaries:

```text
pump start
  WASM start
    wasm_process_msg
  WASM end / dispatch start
    dispatch_on_app
  dispatch end
    free and existing snapshot publication
pump end
```

No new per-pump collection is allocated beyond the snapshot objects already created by the existing code.

## HUD

The existing opt-in HUD adds `wasm` and `dispatch` timing lines. The WebGL formatter adds compact lines for passes/submits and uniform, buffer, and texture write calls/bytes. It stays throttled by the existing 125 ms timer.

## Testing

Host-runnable Rust integration tests use the repository's established `include_str!` contract style for Web-only JavaScript:

- Verify the new timing fields exist in initialization, fallback snapshot, published snapshot, retained active snapshot, and HUD.
- Verify source ordering of `wasm_process_msg`, the WASM end timestamp, `dispatch_on_app`, the dispatch end timestamp, and `free`.
- Verify WebGL hook names, all snapshot fields, reset-to-zero values, fresh-object construction, and instrumentation at the shared upload/draw/pass sites.

`node --check` validates both JavaScript files syntactically. Release `makepad-platform` tests and a release splash WASM package validate integration.

## Acceptance Criteria

- Existing snapshot consumers still receive unchanged `pump_ms` and `draw_calls` semantics.
- Every snapshot contains numeric `wasm_ms` and `dispatch_ms` values.
- An active WebGL pump exposes the documented backend object instead of `null`.
- Counters reset once per pump and retained snapshots do not alias the mutable counter object.
- Only actual GL data writes contribute calls/bytes; cache skips and null render-target allocations do not.
- Formatting, host release tests, JavaScript syntax checks, and release WASM packaging pass.
