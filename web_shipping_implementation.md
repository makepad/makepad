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
