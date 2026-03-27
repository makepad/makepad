# Makepad Composition Guide

This guide documents the current Makepad composition and placement model after
the draw-list spatial simplification landed.

It replaces the older guide that still described draw-list `view_shift` and
`view_clip` as active APIs. Those fields are gone from the active codebase.

Read files covered for this guide:

- `makepad/simplify-api.md`
- `makepad/simplify-plan.md`
- `makepad/platform/src/draw_list.rs`
- `makepad/platform/src/draw_pass.rs`
- `makepad/draw/src/cx_draw.rs`
- `makepad/draw/src/draw_list_2d.rs`
- `makepad/draw/src/turtle.rs`
- `makepad/draw/src/shader/draw_quad.rs`
- `makepad/draw/src/shader/draw_text.rs`
- `makepad/widgets/src/view.rs`
- `makepad/compositor/src/quad.rs`
- `makepad/compositor/src/surface.rs`
- `havi/ports/havishell/src/servo_web_view.rs`
- `havi/crates/render/src/lib.rs`

This guide focuses on:

- what the active spatial and composition contracts are
- what each composition layer actually does
- what draw lists, turtles, passes, and surfaces are for
- what the compositor path does today
- what that means for HAVI-style retained browser composition

---

## 1. Executive summary

The active Makepad spatial model is:

1. place geometry in local item coordinates
2. clip in local item coordinates
3. apply draw-list `view_transform`
4. project through the pass

The active codebase has one draw-list-wide spatial transform:

- `view_transform`

The following are no longer active draw-list spatial APIs:

- `view_shift`
- `view_clip`
- `draw_list_has_clip`

Passes still have their own projection controls:

- `view_shift`
- `view_scale`

Those are pass-level composition concerns, not draw-list placement APIs.

The main structural conclusions remain:

- `SubList` is a structural parent/child draw-list edge
- it is not an explicit positioned child-scene edge carrying transform and clip
  payload
- turtle layout remains the normal local placement basis for ordinary widgets
- child passes and texture composition remain the strongest built-in subtree
  composition path for isolated content
- explicit compositor quads remain the strongest built-in transformed surface
  composition path

The most important HAVI conclusion remains:

- the real problem is not that Makepad lacks local widget placement
- the real problem is mixing multiple root coordinate stories for the same
  retained browser content
- all render routes must agree on one explicit webview host basis

---

## 2. Composition layers and what each one means

## 2.1 Draw-list hierarchy

Primary files:

- `makepad/platform/src/draw_list.rs`
- `makepad/draw/src/draw_list_2d.rs`

Core types:

- `DrawList`
- `DrawList2d`
- `CxDrawList`
- `CxDrawKind::SubList(DrawListId)`

What draw-list hierarchy provides:

- structural traversal
- redraw grouping and invalidation scope
- a per-draw-list uniform scope
- recursive rendering in backends
- codeflow parent/child tracking

What it does not provide:

- an explicit parent-relative transform payload on the `SubList` edge
- an explicit parent-relative origin on the `SubList` edge
- an explicit child-scene clip payload on the `SubList` edge

`append_sub_list(...)` still appends a structural child draw list. The edge is
still structural, not a full scene-node attachment primitive.

---

## 2.2 Turtle layout and local widget placement

Primary file:

- `makepad/draw/src/turtle.rs`

Turtle is still the ordinary local 2D placement system.

Important APIs:

- `begin_turtle`
- `end_turtle`
- `walk_turtle`
- `begin_page_root_turtle`
- `begin_root_turtle`
- `begin_unclipped_root_turtle`
- `begin_root_turtle_for_pass`
- `begin_unclipped_root_turtle_for_pass`
- `end_pass_sized_turtle`
- `end_pass_sized_turtle_with_shift`
- `push_clip_rect`
- `pop_clip_rect`

Important active behavior:

- turtles define local layout space
- item rects and `draw_clip` are established in local coordinates
- root turtles define local root origin and root clip for the draw sequence
- align-list processing can still mutate accumulated item placement and clipping
  after layout is known

Current root-turtle comments in code explicitly match the simplified model:

- ordinary widget placement happens in local rect and clip coordinates
- any draw-list-wide transform is carried separately by `view_transform`

### `begin_page_root_turtle`

`begin_page_root_turtle(origin, size, layout)` starts a root turtle at an
explicit local origin and pushes the initial clip rectangle.

This is still a layout/clip basis operation. It is not a draw-list transform
operation.

### `begin_root_turtle_for_pass`

`begin_root_turtle_for_pass(layout)` uses the current pass size and starts a root
turtle at `(0, 0)` in that pass.

That is the natural local basis for offscreen pass rendering.

### `end_pass_sized_turtle_with_shift`

This still records a subtree shift through align-list processing.

It is a post-layout subtree shift mechanism in local 2D space. It is not a
child-scene transform edge on `SubList`.

---

## 2.3 Draw-call instance data

Primary files:

- `makepad/platform/src/draw_vars.rs`
- active shader files under `makepad/draw/src/shader/`

Ordinary 2D placement still primarily ends up in instance data:

- `rect_pos`
- `rect_size`
- `draw_clip`
- custom instance fields
- custom uniforms

Turtle and align-list processing matter because they define and mutate those
local rect and clip values before drawing.

---

## 2.4 Draw-list uniforms

Primary files:

- `makepad/platform/src/draw_list.rs`
- `makepad/draw/src/draw_list_2d.rs`
- active shaders

The active draw-list uniform contract is:

- `DrawListUniforms { view_transform }`

That is the only draw-list-wide spatial uniform in active code.

### `view_transform`

Available APIs:

- `set_view_transform`
- `set_view_transform_self_only`
- `get_view_transform`
- `map_point_to_local`
- `map_point_from_local`

Active semantics:

- local geometry and local clipping are established first
- `view_transform` is then applied draw-list-wide
- pass projection happens after that

Important helper limitation:

- `map_point_to_local()` and `map_point_from_local()` are matrix helpers only
- they do not include item rect placement, item clip, or pass shift/scale

### Recursive vs self-only transform APIs

`set_view_transform(...)` applies recursively to the draw list and existing child
draw lists.

`set_view_transform_self_only(...)` updates only that draw list.

That is useful for grouping, but it still does not turn the `SubList` edge into
an explicit parent-relative scene node.

### Projective transform support

`CxDrawList` still maintains a derived `projective_transform`, but it is not part
of `DrawListUniforms` anymore.

The derived projective transform is computed from:

- current draw-list `view_transform`
- current pass view-projection

and written into shaders that explicitly need `u_projective_transform`.

That keeps the ordinary draw-list uniform contract minimal.

---

## 2.5 Pass projection and pass placement

Primary files:

- `makepad/platform/src/draw_pass.rs`
- `makepad/draw/src/cx_draw.rs`

Pass APIs:

- `make_child_pass`
- `begin_pass`
- `end_pass`
- `set_pass_area`
- `set_pass_area_with_origin`
- `set_pass_shift_scale`

Pass state still includes:

- `pass_rect`
- `view_shift`
- `view_scale`
- `camera_projection`
- `camera_view`

Important current rule:

- pass `view_shift` and `view_scale` still exist
- they are pass projection controls
- they are not draw-list placement APIs

### `set_pass_area`

Binds a pass rect to an `Area`.

### `set_pass_area_with_origin`

Binds a pass size to an `Area` but overrides the pass rect origin.

This is the explicit parent-space attachment mechanism for a child pass.

### `set_pass_shift_scale`

Applies pass-level projection shift and scale.

This is a pass composition tool. It is not a substitute for local item placement
or draw-list transform semantics.

---

## 2.6 Surfaces, child passes, and texture-backed subtree composition

Primary files:

- `makepad/widgets/src/view.rs`
- `makepad/compositor/src/surface.rs`

Important types:

- `DrawPass`
- `Texture`
- `MpSurface`
- `ViewOptimize::Texture`

These are the strongest built-in subtree composition mechanisms.

### `ViewOptimize::Texture`

Current behavior in `View`:

- allocate child pass and render texture
- render subtree into the child pass
- draw the texture back into the parent
- reattach the pass with `set_pass_area(...)` or `set_pass_area_with_origin(...)`

This remains the clearest in-tree example of texture-backed subtree composition.

### `MpSurface`

`MpSurface` is the compositor-side reusable surface abstraction:

- own pass
- own color texture
- render subtree into the pass
- sample the result elsewhere

Like `ViewOptimize::Texture`, `MpSurface` solves surface ownership and local
pass rendering.

It does not by itself solve parent attachment. Parent placement still has to be
explicit.

---

## 2.7 Explicit transformed surface composition via compositor quads

Primary file:

- `makepad/compositor/src/quad.rs`

Important type:

- `MpCompositedQuad`

Current composition fields include:

- `texture`
- `local_rect`
- `uv_rect`
- `transform`
- `opacity`
- `clip_planes`
- `mask`

Current quad shader behavior is explicit:

- local quad geometry comes from `local_rect`
- quad transform is composed under the active draw-list `view_transform`
- pass projection is applied after that

This is the active surface-compositor primitive for transformed textured content.

It is not an ordinary widget-layout primitive.

---

## 3. What current shaders actually do

## 3.1 `DrawQuad`

Primary file:

- `makepad/draw/src/shader/draw_quad.rs`

Current ordinary path:

1. compute local point from `rect_pos` and `rect_size`
2. clamp against local `draw_clip`
3. multiply by `draw_list.view_transform`
4. project through pass view and projection

There is no active draw-list `view_shift` or `view_clip` stage in this shader.

## 3.2 `DrawText`

Primary file:

- `makepad/draw/src/shader/draw_text.rs`

Current text path:

1. compute local point from glyph `rect_pos` and `rect_size`
2. clamp against local `draw_clip`
3. multiply by `draw_list.view_transform`
4. project through the pass

This now matches the simplified contract.

## 3.3 Projective shaders

Projective composition still exists where needed, but explicit projective
uniforms are supplied outside `DrawListUniforms`.

That is the intended current split:

- ordinary 2D shaders use local rect and clip plus `view_transform`
- projective or compositor paths receive explicit projective state separately

---

## 4. Backend behavior relevant to composition

Backends still recurse into child draw lists when they encounter `SubList`.

That means child draw-list uniform scope is real.

The previous WebGL bug where `render_view` reset `view_transform` to identity is
fixed. The current WebGL backend forwards the active `draw_list_uniforms`
without forcing identity.

That matters because `view_transform` is now part of the shared active contract
across native and web backends.

---

## 5. What `SubList` is good for

`SubList` is good for:

- structural traversal
- redraw grouping
- child uniform scope
- overlay and nesting structure
- batching and subtree ownership organization

`SubList` is not, by itself, a documented child-scene attachment API with:

- explicit parent-relative transform payload
- explicit parent-relative clip payload
- explicit child local-to-parent placement contract

If you need those semantics, you must combine other mechanisms or use a
texture-backed composition path.

---

## 6. Available composition strategies in current Makepad

## Strategy A — ordinary local widget drawing

Use:

- turtles
- instance rects and local clips
- ordinary shaders
- optional draw-list grouping

Best for:

- ordinary widgets
- local 2D content
- simple retained content that does not need isolated surfaces

## Strategy B — draw-list-wide transform over local content

Use:

- local turtle or direct instance placement
- `view_transform`

Best for:

- draw-list-wide transform of a grouped subtree
- cases where one transform over the whole draw list is sufficient

Caution:

- this is a draw-list transform, not an explicit child-scene edge
- item placement still happens in local rect/clip coordinates

## Strategy C — child pass to texture, then parent composition

Use:

- `make_child_pass`
- child pass rendering
- render textures or `MpSurface`
- `set_pass_area` or `set_pass_area_with_origin`

Best for:

- isolated subtrees
- caching
- subtree flattening before later composition

## Strategy D — transformed surface quad composition

Use:

- render surface to texture
- `MpCompositedQuad`

Best for:

- browser-compositor-like surface composition
- transformed layers
- explicit quad transform and clipping

---

## 7. What `View` teaches about intended composition

`makepad/widgets/src/view.rs` remains the clearest reference.

It shows three active patterns:

1. `ViewOptimize::None`
   - ordinary current-pass widget drawing
2. `ViewOptimize::DrawList`
   - separate draw list, same pass
3. `ViewOptimize::Texture`
   - child pass to texture, then texture composition back into parent

What it shows clearly:

- draw lists are used for grouping and redraw scope
- child passes are used for isolated surface rendering
- parent attachment of a child pass is explicit through pass-area APIs

What it does not show:

- `SubList` as a fully explicit positioned child-scene API carrying its own
  placement contract independent of local rects, turtles, and pass attachment

---

## 8. How this applies to HAVI

## 8.1 Normal widget-local placement already exists

In `ServoWebView`:

- the widget establishes its area through ordinary widget drawing
- `self.draw_bg.area().rect(cx)` is the resolved host rect

That is the correct explicit webview host basis.

## 8.2 Retained browser content currently uses multiple render routes

HAVI retained browser content may go through:

- direct local primitives
- text runs
- pictures and tasks
- surface composition

Those routes are acceptable.

The architectural requirement is not one route. The requirement is one shared
root coordinate truth.

## 8.3 Root basis is explicit

The render crate does not rediscover host placement from ambient layout state.
The widget resolves the host rect explicitly via `self.draw_bg.area().rect(cx)`
and passes it as `host_rect: Rect` to `render_fragments_clipped()`.

Retained clip data is stored in origin space only. The compositor's shared clip
evaluator (`evaluate_clip_chain` in `quad.rs`) derives the full basis at draw
time:

```
clip_from_origin = clip_from_world * world_from_scene * scene_from_origin
```

This ensures geometry and clipping always use the same transform chain,
regardless of where the webview is placed in the Makepad draw-list hierarchy.

---

## 9. Retained browser composition model

For HAVI:

- one explicit webview host basis (`host_rect` from widget)
- retained scene data is placement-independent (origin-space clips)
- direct primitives stay direct where appropriate
- isolated or transformed content uses surfaces and compositor quads
- no ambient coordinate discovery in retained rendering
- one shared clip evaluator for both primitive and picture paths
- `scene_from_origin` in `MpPrimitiveTransform` stores only the
  origin-to-scene mapping; draw-list and pass transforms are applied at draw time

---

## 10. Final conclusions

### What current Makepad clearly has

- local item placement and clipping through turtles and instance data
- one draw-list-wide transform: `view_transform`
- pass-level projection controls: `view_shift` and `view_scale`
- child-pass and texture-backed subtree composition
- explicit transformed textured-quad composition
- backend support for active `view_transform` across native and web

### What current Makepad still does not clearly define

It still does not define `SubList` as a first-class child-scene attachment edge
with explicit transform and clip payload semantics.

### Current practical rule

For complex retained composition such as HAVI:

- local geometry and local clip belong to retained scene data
- root placement must be explicit
- texture-backed subtree composition and explicit compositor quads are the
  strongest existing composition tools when isolation or transformed surfaces are
  needed
- all render routes must agree on one explicit host basis
