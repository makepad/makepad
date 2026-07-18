# Makepad Web Phase 1a Observability Implementation Plan

> **Goal:** Add the smallest verified web performance snapshot that separates WASM work from JavaScript dispatch and reports concrete WebGL work per pump.

> **Scope:** `platform/src/os/web/web.js`, `platform/src/os/web/web_gl.js`, and one host-runnable integration test. No dependencies, browser harness, Rust/JS metric protocol, GPU queries, histograms, or `PerfMonitor` changes.

## Contract

- Preserve `pump_ms` and the existing top-level `draw_calls` field.
- Add top-level `wasm_ms` and `dispatch_ms`, measured per `do_wasm_pump()` call.
- `dispatch_ms` excludes `from_wasm.free()`.
- Add a fresh backend snapshot containing `passes`, `draw_commands`, `submits`, and uniform/buffer/texture write call and byte counts.
- Keep all backend counters per pump and reset them before dispatch.
- Count actual GL uploads only: skip uniform-cache hits, render-target allocation, and browser-owned video frames.

## Task 1: Split WASM and JavaScript dispatch timing

**Files:**

- Create: `platform/tests/web_perf.rs`
- Modify: `platform/src/os/web/web.js`

1. Add a failing source-contract test named `web_perf_snapshot_splits_wasm_and_dispatch_time`. It must assert:
   - `last_wasm_ms` and `last_dispatch_ms` are initialized.
   - `wasm_ms` and `dispatch_ms` appear in fallback, current, and `last_active` snapshots.
   - both values appear in the opt-in HUD.
   - within `do_wasm_pump()`, the ordering is `wasm_process_msg` → finish WASM timing → `dispatch_on_app` → finish dispatch timing → `free`.
2. Run the focused test and confirm it fails:

   ```bash
   rtk cargo test --release -p makepad-platform --test web_perf web_perf_snapshot_splits_wasm_and_dispatch_time
   ```

3. In `do_wasm_pump()`, measure `this.wasm_process_msg(to_wasm)` and `from_wasm.dispatch_on_app()` independently with `performance.now()`. Compute `dispatch_ms` before `from_wasm.free()` and leave `pump_ms` unchanged.
4. Publish the two values through `get_perf_snapshot()`, each current snapshot, `last_active_snapshot`, and the HUD.
5. Run:

   ```bash
   rtk node --check platform/src/os/web/web.js
   rtk cargo test --release -p makepad-platform --test web_perf
   ```

6. Commit as `feat(web): split pump performance timing`.

## Task 2: Expose per-pump WebGL work counters

**Files:**

- Modify: `platform/tests/web_perf.rs`
- Modify: `platform/src/os/web/web_gl.js`

1. Add a failing source-contract test named `webgl_perf_snapshot_counts_backend_work`. It must assert:
   - `reset_backend_perf`, `get_backend_perf_snapshot`, and `format_backend_perf_hud` exist.
   - the snapshot contains exactly the nine contracted fields.
   - the returned snapshot is a fresh object rather than the mutable counter object.
   - both render-pass entry points increment `passes`.
   - the decoded draw-command site increments `draw_commands`.
   - every `drawElementsInstanced` site increments `submits`.
   - actual uniform, buffer, and WASM-backed texture writes update call and byte counters.
   - render-target allocation and video texture updates contain no texture-upload counter updates.
2. Run the focused test and confirm it fails:

   ```bash
   rtk cargo test --release -p makepad-platform --test web_perf webgl_perf_snapshot_counts_backend_work
   ```

3. Add a private zeroed counter object and implement the three existing backend hooks. `get_backend_perf_snapshot()` must return a new plain object on every call.
4. Instrument:
   - `FromWasmBeginRenderTexture` and `FromWasmBeginRenderCanvas` for passes.
   - the valid `CMD_DRAW` decode point for draw commands.
   - all successful `gl.drawElementsInstanced` calls for submits, counting XR eyes separately.
   - actual `UNIFORM_BUFFER` uploads after cache skips.
   - actual array/index buffer uploads.
   - BGRA, R8, RGBA-f32, and all six cube-face uploads from WASM memory.
5. Leave the existing top-level `this.perf.draw_calls` behavior unchanged. Do not count null render-target allocation or browser video texture data.
6. Run:

   ```bash
   rtk node --check platform/src/os/web/web_gl.js
   rtk cargo test --release -p makepad-platform --test web_perf
   ```

7. Commit as `feat(web): expose WebGL performance counters`.

## Final verification

Run every command in release mode where applicable:

```bash
rtk cargo fmt --check
rtk node --check platform/src/os/web/web.js
rtk node --check platform/src/os/web/web_gl.js
rtk cargo test --release -p makepad-platform --lib
rtk cargo test --release -p makepad-platform --test web_phase0
rtk cargo test --release -p makepad-platform --test web_perf
rtk cargo run --release -p cargo-makepad -- wasm build -p makepad-example-splash --release
```

Then request a fresh code review against the Phase 1a design. Phase 1a is complete only if every command passes and review finds no blocking issue.
