# makepad-browser-scene

Retained browser-scene API and renderer for Makepad.

Public module boundaries:

- `scene.rs` — document and scene ownership
- `transaction.rs` — retained scene/resource updates
- `spatial.rs` — reference frames, scroll, sticky, embed roots
- `clip.rs` — clip nodes and clip chains
- `effect.rs` — opacity, blend, filter, isolation, mask metadata
- `primitive.rs` — retained browser primitives
- `resource.rs` — retained image/font/external resource keys and updates
- `embed.rs` — child scene attachment
- `hit_test.rs` — retained hit-test tags and queries
- `renderer.rs` — execution boundary for direct paint and isolated compositor groups

Current executable subset:

- direct paint: solid rect, rounded rect, uniform border, box shadow,
  linear/radial/conic gradients, text-run, basic image primitives, and
  child-document embed surfaces
- isolated compositor boundaries: effect groups that require isolation
- clip execution: rect clips, rounded-rect clips, image-mask clips, and
  plane-set clips through compositor-facing clip chains
- embed contract: retained documents may own child documents keyed by pipeline id;
  embed items resolve parent spatial and clip context before compositing the child scene
- text contract: retained glyph-run resources with exact glyph positions,
  font keys, metrics, and decoration data
- retention hooks: `MpScene::set_scroll_offset()` and `MpScene::update_scroll_offsets()`
  support scroll-only scene reuse without rebuilding primitives or resources
- renderer stats: `MpRendererStats` reports compositor surface count, total offscreen
  pixel area, scratch surface usage, and scratch reuse/new-allocation counters

Current examples:

- `examples/basic-rects` — direct primitives plus isolated group routing
- `examples/effects-clip` — clip-chain resolution plus isolated effect routing
- `examples/text-layout` — retained glyph-run resource submission and text paint

The public retained scene and resource boundaries are stable.

## Spatial model

Retained clip data is declarative and stored in origin space only.

- `origin space`: the coordinate system of each retained primitive or picture
  after browser-scene lowering.
- `scene_from_origin` (stored in `MpPrimitiveTransform`): maps from origin space
  to the browser-scene host's local coordinate system.
- At compositor draw time, the full basis is derived explicitly:
  `clip_from_origin = clip_from_world * world_from_scene * scene_from_origin`
  where `world_from_scene` is the outer Makepad draw-list `view_transform` and
  `clip_from_world` is the current pass view-projection.
- Both geometry and clipping use this same full basis. The shared clip evaluator
  in `compositor/src/quad.rs` (`evaluate_clip_chain`) converts origin-space
  planes and mask matrices through the explicit basis at draw time.
- HAVI passes the resolved host rect explicitly. No ambient turtle or draw-list
  geometry is read by the retained browser path.
