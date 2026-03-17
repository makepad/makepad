# Notes: Web Shipping Performance

## Current Build State
- `tools/cargo_makepad/src/wasm/mod.rs` supports `build` and `run`, with wasm options parsed ahead of the subcommand.
- `tools/cargo_makepad/src/wasm/compile.rs` currently:
  - builds the wasm app,
  - copies the full `resources/` tree for the build crate and dependencies,
  - optionally strips/optimizes/splits/brotli-compresses wasm,
  - generates a static `index.html`,
  - serves dev output through an internal webserver.

## Existing Runtime Behavior
- `platform/src/script/res.rs` already skips eager loading for heavy CJK/emoji fallback fonts on wasm and can fetch missing script resources over HTTP.
- `draw/src/shader/draw_text.rs` already performs text-driven lazy loading of fallback font members.
- `widgets/src/lib.rs` already uses lighter default web font sets and keeps wider i18n fallbacks separate.

## Implementation Direction
- Add a new explicit shipping subcommand with shipping defaults and opt-backs.
- Add manifest/perf metadata and hash startup-blocking assets.
- Keep dev server behavior for `wasm run`, but introduce shipping preview semantics for the shipping path.
- Replace blanket dependency resource copying with a narrower shipping package policy plus explicit preserve-list support.

## Doc Gap
- `README.md` currently claims `--profile=small` uses smaller fonts, but font shrinking is currently gated by `--small-fonts`.

## Startup Path Follow-up
- `libs/wasm_bridge/src/wasm_bridge.js` currently awaits `split_data_url` before compiling the primary module, so the split data blob is always startup-blocking.
- `secondary_wasm_url` is only truly non-blocking when the build emits `defer_secondary_wasm`; the fallback split path still awaits it before startup finishes.
- Even in deferred mode, the loader was starting the secondary fetch immediately, so transfer bytes still competed on the initial network path.
- The follow-up patch now delays deferred secondary fetch kickoff until after the first paint opportunity, while keeping eager startup fetches for fallback split mode.
- Verified on `makepad-example-splash`:
  - `total_raw_bytes = 7268115`
  - `startup_blocking_transfer_bytes = 1112499`
  - the corrected startup-blocking budget still passes under the `1200000` byte limit.

## Split Overhead Follow-up
- `libs/wasm_strip/src/wasm_strip.rs` no longer exports every defined function from the primary split module; it now exports only the primary functions that split bodies directly reference.
- The secondary split module now imports only that referenced primary subset and rewrites direct function refs so split-to-split calls stay inside the secondary module.
- Verified on `makepad-example-splash` after this change:
  - function-split primary shrank from `2360835` to `2265806` bytes before the data split stage,
  - the final package dropped to about `8.7M` raw,
  - `startup_blocking_transfer_bytes` improved from `1112499` to `1077011`,
  - splash still lands in automatic fallback mode, so the next remaining blocker is the startup-hot/cold selection itself rather than split-module overhead.

## Startup Classification Follow-up
- `startup_hot_function_indices` no longer roots every active table entry as startup-hot.
- Active-element functions are now pulled into the startup-hot set only when already-hot code performs an indirect call of a matching function type.
- Verified on `makepad-example-splash` after this change:
  - automatic split switched from `mode: automatic fallback split, secondary remains on the startup path` to `mode: cold-first split, secondary deferred`,
  - only `1` function is currently safe to defer under the cold-first heuristic for splash,
  - `makepad-example-splash.secondary.wasm` shrank to `2860` raw bytes / `1106` transfer bytes and is no longer startup-blocking,
  - package size dropped to about `8.4M` raw,
  - `startup_blocking_transfer_bytes` improved again from `1077011` to `1019127`.

## Cold Prefix Selection Follow-up
- The active-only `data.bin` shortcut was reverted after measurement showed it regressed splash shipping output (`startup_blocking_transfer_bytes` rose from `1019127` to `1023884` and the package grew to about `9.4M` raw).
- `wasm_split_functions_to_target_primary_size_cold` now keeps searching for the largest safe cold prefix under the target instead of stopping at the first safe split count.
- Verified on `makepad-example-splash` after this change:
  - automatic split still reports `mode: cold-first split, secondary deferred`,
  - deferred split count increased from `1` to `67`,
  - `makepad-example-splash.wasm` raw size dropped from `4807768` to `3623999`,
  - `makepad-example-splash.secondary.wasm` grew to `10097` raw bytes / `2099` transfer bytes and remains non-startup,
  - `startup_blocking_transfer_bytes` is effectively flat at `1019413`,
  - inference: this pass mainly reduces startup wasm compile/instantiate work rather than transfer size, so the next wins are more likely to come from `data.bin` and default web font payloads than from more aggressive function splitting.

## Active-Only Split Data Overlap Follow-up
- The web loader now receives `split_data_active_only: true` when the build knows `data.bin` contains only active segments.
- In that mode, `libs/wasm_bridge/src/wasm_bridge.js` no longer waits for `data.bin` before compiling the primary wasm. It now:
  - starts primary wasm compile/instantiate immediately,
  - fetches `data.bin` in parallel,
  - patches active segments directly into linear memory before startup continues.
- Modules with any passive split data still use the old rebuild-before-compile path, because passive segments must remain available to wasm `memory.init`.
- A shipping build exposed a separate packaging issue where duplicate logical pending assets could make fingerprinting re-read a file after it had already been renamed. `finalize_pending_assets` now skips duplicate logical entries and has a regression test.
- Verified on `makepad-example-splash` after this change:
  - generated `index.html` now contains `split_data_active_only: true`,
  - package size remains about `8.4M` raw,
  - `startup_blocking_transfer_bytes` remains `1019413`,
  - inference: this pass is a startup overlap improvement, not a transfer-byte reduction, so the next materially measurable package-size win is still font/resource payload cleanup.
