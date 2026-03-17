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
