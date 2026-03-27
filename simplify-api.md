# Simplified Makepad spatial API

Status: completed

This document records the active spatial model after the draw-list simplification landed.

It replaces the earlier analysis of the old `view_shift` / `view_clip` split model.

## Executive summary

The active 2D spatial model is now:

1. compute geometry in local item coordinates
2. clip in local item coordinates
3. apply draw-list `view_transform`
4. project through the pass

The old draw-list `view_shift` path is gone from the active codebase.
The old draw-list `view_clip` path is gone from the active codebase.
The dead `draw_list_has_clip` flag is gone.

This is now true across:

- ordinary 2D shaders
- helper APIs
- hit testing
- the web backend

## Files changed in the landed cleanup

Core and helpers:

- `platform/src/draw_list.rs`
- `platform/src/area.rs`
- `draw/src/draw_list_2d.rs`
- `draw/src/turtle.rs`

Shaders:

- `draw/src/shader/draw_quad.rs`
- `draw/src/shader/draw_text.rs`
- `draw/src/shader/draw_rotated_text.rs`
- `draw/src/shader/draw_svg.rs`
- `draw/src/shader/draw_vector.rs`
- `draw/src/shader/draw_projective_quad.rs`
- `compositor/src/quad.rs`
- `widgets/src/map/view.rs`

Interaction/backend:

- `platform/src/event/finger.rs`
- `platform/src/event/drag_drop.rs`
- `platform/src/event/xr.rs`
- `platform/src/os/web/web_gl.rs`

## Current state

## 1. Draw-list transform model

`DrawListUniforms` now contains one semantic field:

- `view_transform`

That is the only draw-list-wide spatial transform API in active code.

The old fields removed from active code are:

- `view_shift`
- `view_clip`

The old dead draw-list flag also removed from active code is:

- `draw_list_has_clip`

## 2. Ordinary 2D shaders now share one contract

The active ordinary 2D shaders all follow the same model:

- local geometry
- local item clip
- `view_transform`
- pass projection

This applies to:

- `DrawQuad`
- `DrawText`
- `DrawRotatedText`
- `DrawVector`
- `DrawSvg`
- `DrawMapVector`

The old shift-aware clip conversion logic is gone.

## 3. Projective/compositor path is explicit

Projective shaders still need derived projective transform state, but it is no longer encoded as a draw-list uniform field.

Instead:

- draw-list derived state is computed outside `DrawListUniforms`
- projective shaders receive explicit `u_projective_transform` uniform data

This keeps the ordinary 2D draw-list contract minimal while preserving projective composition support.

## 4. Helper API semantics now match the renderer

`draw/src/draw_list_2d.rs`:

- `map_point_to_local()` and `map_point_from_local()` are strict `view_transform` helpers
- they do not pretend to account for item rects, item clips, or pass shift/scale

`platform/src/area.rs` now has a clean split:

- `rect()` -> local item rect
- `local_clipped_rect()` -> local clipped rect
- `clipped_rect()` -> transformed axis-aligned bounding box approximation
- `abs_to_local()` -> inverse-transform point through `view_transform`
- `abs_to_rel()` -> local item-relative point derived from `abs_to_local()` and `rect()`

This matches the rendering model instead of the old mixed helper behavior.

## 5. Hit testing now accounts for `view_transform`

The old bug was:

- hit testing compared screen-space points against local-space rects

That is fixed.

Active hit testing now:

- inverse-transforms event points through draw-list `view_transform`
- tests those points against local rect/clip geometry

Updated paths:

- `platform/src/event/finger.rs`
- `platform/src/event/drag_drop.rs`
- `platform/src/event/xr.rs`

This is the correct model for translation, scale, rotation, and skew.

## 6. Web backend now honors draw-list transforms

The old WebGL backend bug was:

- `render_view` forcibly reset `view_transform` to identity before drawing

That reset is gone.

The web backend now follows the same draw-list transform contract as the native backends.

## 7. Turtle placement remains the ordinary placement basis

The cleanup did not move ordinary widget placement into draw-list transforms.

The intended and active model remains:

- turtle placement establishes local item coordinates
- item rects and item clip live in that local coordinate space
- draw-list `view_transform` applies after local placement

This matches the renderer and the updated turtle/docs comments.

## Deviations from the original draft analysis

The original analysis left one open question:

- retain draw-list `view_clip` as a local pre-transform clip, or remove it

The landed code chose the simpler result:

- remove it

That was the right choice because there was no active producer for meaningful draw-list clip state in the codebase being simplified.

The original draft also suggested a compatibility stopgap:

- patch `DrawText` to support old `view_shift`

That was not needed in the final change because the simplification removed the old shift model instead of extending it.

## Active API surface after the simplification

## Ordinary 2D drawing

Use:

- turtle placement
- item rects
- item clip
- draw-list `view_transform`
- pass APIs

Do not expect any draw-list translation lane separate from `view_transform`.

## Draw-list transform API

Retained public API:

- `set_view_transform`
- `set_view_transform_self_only`
- `get_view_transform`
- `map_point_to_local`
- `map_point_from_local`

## Pass transform API

Pass `view_shift` and `view_scale` still exist and remain pass projection concerns.

They are not draw-list placement APIs.

## What is now gone from active code

Gone:

- draw-list `view_shift`
- draw-list `view_clip`
- draw-list `draw_list_has_clip`
- shader logic that translated clips by `view_shift`
- helper semantics that depended on draw-list shift
- web backend behavior that zeroed `view_transform`

## Tests and verification

Relevant verification completed during landing:

- `cargo build`
- `cargo test -p makepad-platform -p makepad-draw -p makepad-compositor`

New helper coverage now exists for:

- draw-list transform point mapping
- area local-vs-transformed semantics
- inverse-transform point mapping for relative coordinates

Workspace-wide `cargo test` still includes unrelated failures outside the spatial cleanup.

## Bottom line

The active codebase now has one draw-list-wide spatial story:

- local item coordinates for placement and clipping
- `view_transform` for draw-list-wide transforms
- pass APIs for pass-level composition

That is the simplified API the old analysis argued for, and it is now what the code does.
