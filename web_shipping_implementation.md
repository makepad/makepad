# Web Shipping Implementation

## Implemented
- Added `cargo makepad wasm ship` with shipping defaults for `small` profile, stripping, brotli, splitting, small-font packaging, and single-threaded web output.
- Added `--threads`, `--full-fonts`, `--no-split`, `--no-brotli`, and `--serve` handling on the wasm path.
- Added shipping asset metadata generation:
  - `asset-manifest.json`
  - `web-perf-report.json`
  - hashed startup-blocking wasm/js/css asset names
  - HTML generation from finalized asset paths with preload hints
- Replaced shipping-mode blanket resource copying with a narrower selection path:
  - direct `crate_resource(...)` references from the app source
  - curated web widget defaults
  - `[package.metadata.makepad.web] preserve = [...]`
  - `[package.metadata.makepad.web] full_i18n = true`
- Kept `wasm run` on dev/no-store semantics and added shipping-aware cache policy handling in the built-in wasm server.

## Verified
- `cargo test -p cargo-makepad`
- `cargo run -p cargo-makepad -- wasm ship -p makepad-example-splash`

## Observed Result
- `target/makepad-wasm-app/small/makepad-example-splash` now builds to about `8.9M` raw on disk.
- `web-perf-report.json` reports:
  - `total_raw_bytes = 7268654`
  - `startup_blocking_transfer_bytes = 257836`
  - all currently encoded shipping budgets passing

## Remaining Gap
- Cross-browser smoke automation was not added in this pass.
- The built-in server shipping policy is covered by unit tests; the long-running `--serve` preview path was not fully exercised end-to-end in this session.

## Follow-up In Progress
- Tightening split startup-path behavior:
  - treat `split_data.bin` as startup-blocking in the manifest/report because the loader waits on it before compile,
  - treat `secondary.wasm` as startup-blocking unless the build emitted a true deferred split,
  - move deferred secondary fetch kickoff to after the first paint opportunity instead of starting it immediately during initial instantiation.

## Follow-up Verified
- `cargo test -p cargo-makepad`
- `cargo run -p cargo-makepad -- wasm ship -p makepad-example-splash`
- Verified shipping output for splash:
  - package size remains about `8.9M` raw,
  - `makepad-example-splash.secondary.wasm` is now fingerprinted and counted as startup-blocking for fallback split mode,
  - `web-perf-report.json` now reports `startup_blocking_transfer_bytes = 1112499`, which still passes the current `1200000` byte budget.

## Split Overhead Follow-up Verified
- `cargo test -p makepad-wasm-strip`
- `cargo test -p cargo-makepad`
- `cargo run -p cargo-makepad -- wasm ship -p makepad-example-splash`
- Verified after reducing split-module import/export overhead:
  - function-split primary shrank by `95029` bytes before data splitting,
  - `makepad-example-splash.secondary.wasm` transfer size dropped from `627083` to `613202`,
  - `makepad-example-splash.wasm` transfer size dropped from `251619` to `230012`,
  - `web-perf-report.json` now reports `startup_blocking_transfer_bytes = 1077011`,
  - splash still uses automatic fallback split mode, so the next optimization target is the startup-hot root set and cold-function selection.

## Startup Classification Follow-up Verified
- `cargo test -p makepad-wasm-strip`
- `cargo test -p cargo-makepad`
- `cargo run -p cargo-makepad -- wasm ship -p makepad-example-splash`
- Verified after relaxing active-element startup classification:
  - automatic split now reports `mode: cold-first split, secondary deferred`,
  - deferred `secondary.wasm` is no longer startup-blocking and remains unhashed because it is fetched lazily,
  - `web-perf-report.json` now reports `startup_blocking_transfer_bytes = 1019127`,
  - `makepad-example-splash.wasm` transfer is `785330`,
  - `makepad-example-splash.data.bin` transfer is `227429`,
  - the remaining startup-path target is now the primary wasm/data split itself rather than the deferred secondary module.

## Cold Prefix Selection Follow-up Verified
- Reverted the active-only data shortcut after it regressed the shipping baseline for splash.
- Updated `wasm_split_functions_to_target_primary_size_cold` to choose the largest safe cold prefix under the target instead of the first safe split count.
- `cargo test -p makepad-wasm-strip`
- `cargo test -p cargo-makepad`
- `cargo run -p cargo-makepad -- wasm ship -p makepad-example-splash`
- Verified after the larger safe cold split selection:
  - automatic split still reports `mode: cold-first split, secondary deferred`,
  - deferred split count increased from `1` to `67`,
  - `makepad-example-splash.wasm` raw size dropped from `4807768` to `3623999`,
  - `makepad-example-splash.secondary.wasm` is now `10097` raw bytes / `2099` transfer bytes and remains non-startup,
  - `web-perf-report.json` now reports `startup_blocking_transfer_bytes = 1019413`,
  - `makepad-example-splash.data.bin` transfer remains `227429`,
  - inference: transfer size is nearly unchanged, so this pass is primarily a browser compile/instantiate-time optimization rather than a network transfer optimization.

## Active-Only Split Data Overlap Follow-up Verified
- Added `split_data_active_only` to the generated loader config when the split data blob contains only active segments.
- Updated `libs/wasm_bridge/src/wasm_bridge.js` so active-only split data no longer blocks primary wasm compile:
  - it compiles/instantiates the primary wasm first,
  - fetches and parses `data.bin` in parallel,
  - applies active segments directly into linear memory before startup proceeds.
- Kept the existing rebuild-before-compile loader path for modules with passive split data.
- Hardened asset finalization so duplicate logical pending assets are ignored instead of breaking hashed-asset renaming, and added a regression test for that case.
- `node --check libs/wasm_bridge/src/wasm_bridge.js`
- `cargo test -p cargo-makepad`
- `cargo run -p cargo-makepad -- wasm ship -p makepad-example-splash`
- Verified after this change:
  - splash `index.html` now emits `split_data_active_only: true`,
  - package size remains about `8.4M` raw,
  - `web-perf-report.json` still reports `startup_blocking_transfer_bytes = 1019413`,
  - inference: this pass reduces serialization on the startup path but does not reduce shipped bytes, so the next measurable package-size optimization should target the default web font payloads.
