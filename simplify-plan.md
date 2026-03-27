# Simplify spatial API plan

Status: completed

This document now records the end state that landed in code and the work that was actually required to get there.

Reference analysis: `./simplify-api.md`

## Goal

Make Makepad 2D placement follow one explicit model:

- local item placement: `rect_pos`, `rect_size`, local item clip
- draw-list-wide transform: `view_transform`
- pass/surface composition: pass APIs

The old draw-list `view_shift` placement channel is gone.

## Final state

The active codebase now has these properties:

1. no active 2D shader uses draw-list `view_shift`
2. no active helper relies on draw-list `view_shift`
3. draw-list `view_clip` is gone from the active codebase
4. `view_transform` is the only draw-list-wide spatial API
5. ordinary 2D widget placement remains in turtle/item-local coordinates
6. pass `view_shift` remains a separate pass projection concern
7. hit testing inverse-transforms event points through `view_transform`
8. the web backend no longer resets draw-list `view_transform`

## Non-goals that stayed non-goals

- preserving old `view_shift` behavior
- compatibility aliases
- dual semantics
- hidden equivalence between helper APIs and removed draw-list shift behavior

## Final model

A developer should reason about ordinary 2D placement in one sentence:

- items are positioned and clipped in local coordinates, then the whole draw list is transformed by `view_transform`, then the pass projects it

## Phase status

## Phase 1: define the new contract in code

Status: done

Completed:

- removed `view_shift` from `DrawListUniforms`
- removed `draw_list_has_clip` from `CxDrawList`
- rewrote draw-list docs around `view_transform`
- rewrote helper docs in `draw_list_2d.rs`
- rewrote turtle docs around local placement
- rewrote area helper docs around local vs transformed semantics

Deviation from the original draft:

- `view_clip` was not retained as a local clip API in the final active codebase. It was removed entirely during the completed cleanup.

## Phase 2: remove `view_shift` from all shaders

Status: done

Completed in active code:

- `draw/src/shader/draw_quad.rs`
- `draw/src/shader/draw_text.rs`
- `draw/src/shader/draw_rotated_text.rs`
- `draw/src/shader/draw_svg.rs`
- `draw/src/shader/draw_vector.rs`
- `draw/src/shader/draw_projective_quad.rs`
- `compositor/src/quad.rs`
- `widgets/src/map/view.rs`

Result:

- ordinary 2D shaders now use local geometry + local item clip + `view_transform`
- projective/compositor shaders no longer read draw-list `view_shift`

## Phase 3: replace draw-list clip semantics with local pre-transform clip semantics

Status: completed by deletion

Actual result:

- draw-list clip was removed from the active codebase instead of retained
- active area helpers and shaders no longer merge item clip with draw-list clip
- the old shifted-clip model disappeared instead of being redefined

This was simpler and matched reality: active code had no meaningful draw-list clip producer.

## Phase 4: simplify helper APIs to match rendering

Status: done

Completed:

- `map_point_to_local()` and `map_point_from_local()` remain strict `view_transform` helpers
- `Area::rect()` is local item rect only
- `Area::local_clipped_rect()` returns local clipped rect semantics
- `Area::clipped_rect()` returns transformed AABB semantics
- helper docs now describe those semantics explicitly

## Phase 5: remove dead draw-list-shift plumbing

Status: done

Completed:

- deleted `view_shift` from the active uniform contract
- deleted all active code paths reading draw-list `view_shift`
- deleted all active code paths translating clips by draw-list `view_shift`
- deleted `draw_list_has_clip`
- removed active `view_clip`

## Phase 6: tighten the public draw-list transform API

Status: done

Retained public API:

- `set_view_transform`
- `set_view_transform_self_only`
- `get_view_transform`
- `map_point_to_local`
- `map_point_from_local`

No extra wrapper API was added.

## Phase 7: tests

Status: partially done

Completed:

- helper/mapping tests landed in active code
- transform round-trip tests exist in `draw/src/draw_list_2d.rs`
- area local-vs-transformed tests landed in `platform/src/area.rs`

Deviation from the original draft:

- no new broad mixed-renderer integration suite was added in this change
- coverage was focused on helper semantics and transform behavior

## Phase 8: fix hit testing to account for `view_transform`

Status: done

Completed:

- hit testing now inverse-transforms event points through `view_transform`
- `abs_to_rel()` now works through `abs_to_local()`
- updated:
  - `platform/src/event/finger.rs`
  - `platform/src/event/drag_drop.rs`
  - `platform/src/event/xr.rs`

## Phase 9: slim down `DrawListUniforms`

Status: done

Completed semantic cleanup:

- removed `view_shift`
- removed `view_clip`
- moved projective transform out of `DrawListUniforms`
- active semantic content of `DrawListUniforms` is now only `view_transform`

Related note:

- projective composition still needs derived projective transform state, but it is no longer stored in `DrawListUniforms`

## Phase 10: follow-up API review

Status: not part of this commit

The separate review question remains:

- whether Makepad wants an explicit positioned child-scene composition API

That is still separate from the draw-list spatial simplification and was not reintroduced implicitly.

## Additional completed work not called out strongly enough in the original draft

- `platform/src/os/web/web_gl.rs` no longer forces `view_transform = identity()` during draw-list rendering
- projective shaders now receive derived projective transform through explicit draw-call uniform plumbing instead of a draw-list uniform field
- an unrelated workspace build fix landed in `media/examples/window_record_mp4/src/app.rs` to keep `cargo build` green while making the spatial changes

## Verification used for the landed state

Successful verification during implementation:

- `cargo build`
- `cargo test -p makepad-platform -p makepad-draw -p makepad-compositor`

Workspace-wide `cargo test` still includes unrelated non-spatial failures outside this change set.

## Summary

The hard simplification landed.

The active codebase no longer has two draw-list placement channels. Ordinary 2D drawing, helper APIs, hit testing, and backend behavior are aligned around one draw-list-wide transform: `view_transform`.
