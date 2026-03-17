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

## Status
**Completed** - shipping pipeline, packaging metadata, docs, and validation are in place.
