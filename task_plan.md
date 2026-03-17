# Task Plan: Web Shipping Performance

## Goal
Implement a concrete `cargo makepad wasm ship` pipeline that reduces shipped web package size, emits deployment metadata, and adds shipping-aware serving behavior.

## Phases
- [x] Phase 1: Plan and setup
- [x] Phase 2: Gather implementation context
- [x] Phase 3: Implement CLI and build pipeline changes
- [x] Phase 4: Implement metadata, resource packaging, and shipping preview behavior
- [x] Phase 5: Add tests, docs, and verification

## Key Questions
1. Which existing wasm build/server seams can absorb shipping behavior cleanly?
2. How much of the runtime font/lazy-resource work already exists and only needs packaging alignment?
3. What is the narrowest resource-graph implementation that materially cuts package size without breaking apps?

## Decisions Made
- Use `wasm ship` as a new explicit shipping path instead of changing `wasm run`.
- Keep dev and shipping serving semantics separate.
- Reuse existing lazy web font/resource loading where possible instead of inventing a second runtime path.
- Use a pragmatic shipping resource graph: direct `crate_resource(...)` references from the app plus curated web widget defaults and Cargo preserve-list overrides.
- Keep default web shipping in small-font mode, with `--full-fonts` / `full_i18n` as the explicit opt-in for the heavyweight fallback fonts.

## Errors Encountered
- `ccc` CLI is not installed in this environment; using local source reads and built-in code search instead.
- `cp_brotli` tried to minify JS before creating the destination directory; fixed by creating the parent directory before writing the minified output.
- `crate_resource` raw-string extraction dropped the first character of raw literals; fixed the literal decoder and covered it with a unit test.

## Follow-up Pass
- [x] Inspect the split loader/runtime contract and confirm which assets are truly startup-blocking.
- [x] Patch deferred secondary wasm loading so its fetch starts after first paint instead of on the initial critical path.
- [x] Align manifest and perf-report startup-blocking flags with the actual loader behavior.
- [x] Re-verify shipping metrics and note any remaining gaps.

## Split Overhead Pass
- [x] Inspect why `makepad-example-splash` still falls back to startup-path secondary wasm in automatic split mode.
- [x] Reduce split-module overhead by exporting/importing only the primary functions that split bodies directly reference.
- [x] Re-verify the splash shipping build and capture the updated startup-transfer baseline.

## Startup Classification Pass
- [x] Relax startup hot/cold classification so active-element functions are only treated as startup-hot when startup code performs matching indirect calls.
- [x] Re-verify that `makepad-example-splash` now takes a true deferred secondary wasm split in automatic mode.
- [x] Capture the new startup-transfer baseline after the classifier change.

## Cold Prefix Selection Pass
- [x] Revert the active-only data shortcut after it regressed the splash shipping baseline.
- [x] Update cold split selection to choose the largest safe deferred prefix instead of the first safe prefix.
- [x] Re-verify wasm-strip and cargo-makepad tests against the stronger cold split behavior.
- [x] Capture the updated splash shipping baseline and compare transfer versus raw startup wasm size.

## Active-Only Split Data Overlap Pass
- [x] Detect active-only split data at build time and emit it into the generated web loader config.
- [x] Add a wasm-loader fast path that compiles and instantiates the primary module before `data.bin` finishes loading, then patches active segments into memory before startup.
- [x] Keep the old rebuild-before-compile path for modules with passive split data segments.
- [x] Harden asset finalization against duplicate logical entries after a shipping build exposed a fingerprinting failure.
- [x] Re-verify splash shipping output and confirm the emitted HTML uses the active-only split-data fast path.

## Status
**Completed** - splash now uses a true deferred secondary wasm split in automatic shipping mode, a larger cold split for startup wasm reduction, and an active-only split-data loader path that overlaps `data.bin` fetch with primary wasm compile/instantiate. The remaining biggest byte wins are still the split data blob and default web font payloads.
