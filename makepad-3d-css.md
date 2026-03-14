# Makepad internals review for browser-style CSS 3D compositor support in HAVI

## Scope

This review focuses on Makepad internals under `makepad/` and only touches HAVI where needed to frame integration.

Goal: determine whether Makepad can support a HAVI-side compositor path that preserves browser-style CSS 3D semantics when needed:

- full hierarchical 4x4 transforms
- perspective and perspective divide
- `preserve-3d` participation across a subtree
- offscreen surfaces / render-to-texture flattening boundaries
- projected clipping
- depth ordering / z handling
- hit-testing implications only where they affect API design

The question is not whether Makepad can become a browser engine. The question is whether Makepad can expose enough low-level compositor primitives for HAVI to submit surfaces/quads/layers with 4x4 transforms while leaving the normal 2D path intact.

---

## Existing Makepad capabilities found

### 1. Matrix math is already present and sufficient

Files:

- `libs/math/src/math_f32.rs`
- `draw/src/draw_list_2d.rs`
- `platform/src/draw_pass.rs`

Makepad already has the needed math primitives:

- `Mat4f::perspective`
- `Mat4f::ortho`
- `Mat4f::look_at`
- `Mat4f::mul`
- `Mat4f::transform_vec4`
- `Mat4f::invert`

This is enough for a compositor that keeps transforms as 4x4 matrices until final GPU submission.

Important detail: `draw/src/draw_list_2d.rs` already treats `DrawListUniforms.view_transform` as a full `Mat4f` and its helper methods do a perspective divide when mapping points:

- `map_point_to_local`
- `map_point_from_local`

That means the core math model is not limited to affine 2D even though most widgets use it that way.

### 2. Draw lists already carry a full 4x4 transform

Files:

- `platform/src/draw_list.rs`
- `draw/src/draw_list_2d.rs`
- `draw/src/shader/draw_quad.rs`

`DrawListUniforms` contains:

- `view_transform: Mat4f`
- `view_clip: Vec4f`
- `view_shift: Vec2f`

`DrawList2d` exposes:

- `set_view_transform`
- `set_view_transform_self_only`
- `get_view_transform`

`DrawQuad` shaders multiply by `draw_list.view_transform` before pass projection.

This is a real compositor-relevant primitive: a subtree draw list can already be transformed by a 4x4 matrix.

Limit: the standard 2D shaders clip to axis-aligned rectangles in local/pass space before transform. That is acceptable for ordinary 2D widgets, but it is not enough for true CSS 3D projected clipping.

### 3. Makepad already supports parent/child passes and render-to-texture

Files:

- `draw/src/cx_draw.rs`
- `platform/src/draw_pass.rs`
- `widgets/src/view.rs`
- `widgets/src/window.rs`
- `platform/src/texture.rs`

Existing pass primitives:

- `CxDraw::make_child_pass`
- `CxDraw::begin_pass`
- `CxDraw::end_pass`
- `DrawPass::set_color_texture`
- `DrawPass::add_color_texture`
- `DrawPass::set_depth_texture`

Existing render-target textures:

- `TextureFormat::RenderBGRAu8`
- `TextureFormat::RenderRGBAf16`
- `TextureFormat::RenderRGBAf32`
- `TextureFormat::DepthD32`

Existing use:

- `widgets/src/view.rs` texture-caches a subtree by rendering it into a child pass and then drawing the resulting texture.
- `widgets/src/window.rs` creates a pass-attached depth texture for the main window pass.

This is the exact base needed for CSS flattening boundaries:

- preserve-3d subtree rendered live in one pass
- flattening boundary rendered into offscreen texture
- parent compositor then reuses that texture as a transformed surface

### 4. Arbitrary GPU geometry is already supported

Files:

- `platform/src/geometry.rs`
- `draw/src/geometry/geometry_gen.rs`
- `draw/src/shader/draw_pbr.rs`

Makepad is not restricted to rect-only rendering internally.

It already has:

- `Geometry`
- `Geometry::update`
- arbitrary index and vertex buffers
- custom draw shaders via `DrawVars`
- custom instance attributes and uniforms

This matters because a CSS compositor does not need full triangle meshes, but it does need more than `rect_pos + rect_size`. It needs at minimum:

- projected quads
- optional subdivided quads later
- custom per-instance data such as 4x4 transform, UV rect, opacity, backface flag, clip planes

A companion compositor crate can build this on top of existing `Geometry + DrawVars + custom shader` without replacing Makepad’s renderer.

### 5. There is already a working 3D draw path in Makepad

Files:

- `draw/src/shader/draw_pbr.rs`
- `draw/src/shader/draw_text_3d.rs`
- `widgets/src/3d/scene_3d.rs`
- `widgets/src/3d/gltf_bridge.rs`

What exists:

- `DrawPbr` uses model/view/projection 4x4 matrices
- `DrawText3d` projects world positions to screen
- `Scene3D` builds a 3D camera and submits 3D content
- GLTF bridge traverses hierarchical transforms and multiplies world matrices

This proves that Makepad’s GPU/shader/backend stack can already carry 4x4 transforms and depth-tested 3D content.

It does **not** by itself solve CSS 3D compositing, because CSS needs textured surfaces and browser flattening rules rather than PBR meshes. But it shows the lower levels are capable.

### 6. Depth buffers exist across the main 3D path

Files:

- `widgets/src/window.rs`
- `platform/src/draw_pass.rs`
- `platform/src/os/apple/metal.rs`
- `platform/src/os/linux/opengl.rs`
- `platform/src/os/windows/d3d11.rs`
- `platform/src/os/linux/vulkan.rs`
- `platform/src/os/web/web_gl.rs`

Backends generally enable depth testing and use `LESS_OR_EQUAL` style depth behavior.

This is enough for a preserve-3d compositor path as long as the submitted quads are actual 3D geometry and not just painter-sorted 2D rects.

### 7. Makepad already has a dormant draw-matrix subsystem

File:

- `platform/src/draw_matrix.rs`

There is a `DrawMatrix` / `CxDrawMatrixPool` type that appears intended for hierarchical transform nodes.

Current state:

- partially implemented
- parent propagation unfinished
- not used by the active draw system

This is evidence that hierarchical transform storage was considered, but it is not a usable foundation today.

### 8. Shader language and packing already support matrix data

Evidence:

- `Mat4f` is a script pod type
- shaders use `uniform(mat4x4f(...))`
- backends chunk attributes into 4-float pieces where needed

This means a compositor shader can carry 4x4 transforms today. No large shader-language rewrite is required for the base design.

### 9. Hit-testing math helpers exist, but the widget hit model is still rect-based

Files:

- `draw/src/draw_list_2d.rs`
- `platform/src/area.rs`
- `platform/src/event/finger.rs`

Useful part:

- `DrawList2d::map_point_to_local` and `map_point_from_local` can invert a 4x4 transform with perspective divide.

Blocking part:

- `Area::clipped_rect` is axis-aligned rect logic
- `Event::hits*` tests against `Rect` / inset-expanded rects
- widget hit-testing is not polygon/projected-surface aware

For HAVI this is acceptable if HAVI owns hit-testing for compositor surfaces. It is not acceptable if the goal is generic Makepad widget-level projected hit-testing.

---

## Missing capabilities

### 1. Projected clipping is not present

Files:

- `draw/src/shader/draw_quad.rs`
- `draw/src/shader/draw_vector.rs`
- `platform/src/area.rs`
- `platform/src/event/finger.rs`
- backend scissor state in `platform/src/os/*`

Current clipping model:

- `draw_clip` and `view_clip` are axis-aligned rectangles
- clipping happens before or in shader relative to axis-aligned local/pass rectangles
- hit-testing uses axis-aligned clipped rects
- backend scissor support is effectively not part of the public drawing model

Backend reality:

- Vulkan sets a full-pass scissor only
- D3D11 rasterizer has `ScissorEnable: FALSE`
- OpenGL exposes `glScissor` but the draw path does not use it per draw
- WebGL path similarly does not expose projected clipping

For CSS 3D, projected ancestor clips need either:

- per-fragment clip planes / projected polygon tests in shader, or
- real scissor/stencil/polygon clipping machinery

Makepad does not currently provide this.

### 2. `preserve-3d` subtree participation is not modeled anywhere

There is no compositor scene graph concept for:

- flattening boundaries
n- preserve-3d groups
- backface visibility rules on textured surfaces
- parent perspective that propagates into descendants and then stops at flattening boundaries

This is not a backend problem. It is an API/model-layer gap.

### 3. Existing 3D scene APIs are mesh-oriented, not surface-compositor-oriented

`DrawPbr` is about meshes and material state.

A CSS compositor needs:

- textured quads or small polygon batches
- offscreen surface allocation
- hierarchical surface/node submission
- opacity/filter/mask hooks later

That API does not exist today.

### 4. Per-draw depth-write control is only partially real

Files:

- `platform/src/draw_shader.rs`
- `platform/src/draw_vars.rs`
- `draw/src/shader/draw_pbr.rs`
- backend files under `platform/src/os/*`

The option exists in the draw system:

- `CxDrawShaderOptions.depth_write`
- parsed from shader object in `DrawVars`
- `DrawPbr::set_depth_write`

Backend status:

- Metal honors it by switching depth states
- Vulkan hardcodes `.depth_write_enable(true)` in pipeline creation
- D3D11 builds one depth stencil state with `DepthWriteMask = ALL`
- OpenGL/WebGL paths do not wire per-draw `glDepthMask`

So the API exists, but it is not consistently implemented.

This matters for a compositor because:

- opaque surfaces should usually write depth
- blended surfaces often need depth test without depth write
- backface or overlay passes may need explicit control

### 5. `DrawPbr` advertises clip/depth controls that are currently dead

File:

- `draw/src/shader/draw_pbr.rs`

`DrawPbr` exposes:

- `clip_ndc`
- `depth_range`
- `depth_forward_bias`

`Scene3D` computes and supplies these values.

But in the shader code they are uploaded and never actually used in the vertex or fragment path.

So current Makepad 3D does **not** already provide projected clip rectangles or depth remapping through this API, despite the surface appearance.

### 6. No generalized surface allocator / compositor submission API exists

Current pieces are low-level and scattered:

- `DrawPass`
- `Texture`
- `Geometry`
- custom draw shaders

There is no focused API for:

- create offscreen surface
- render closure into surface
- submit surface as 3D-transformed quad
- group surfaces into preserve-3d or flattening contexts

### 7. Widget hit-testing is not suitable for transformed surface trees

If HAVI uses a companion compositor path, HAVI should own hit-testing for those surfaces.

Trying to extend existing widget `Event::hits()` semantics to projected polygons would be invasive and unnecessary for the immediate HAVI need.

### 8. Plane splitting is absent

There is no machinery for:

- quad/plane intersection splitting
- BSP-style ordering
- polygon clipping of intersecting transformed layers

This is the right omission for now.

For a browser-style layer compositor, projected quads plus offscreen surfaces are enough for the overwhelming majority of CSS 3D usage. Full plane splitting is only required for exact handling of pathological cases where independently transformed layers geometrically intersect in 3D in ways that cannot be represented by simple per-surface depth ordering.

For HAVI’s target, plane splitting is **not** a first implementation requirement.

---

## Are projected quads + offscreen surfaces enough?

Yes for the practical target.

They are enough to implement:

- full 4x4 transform propagation
- perspective from ancestor contexts
- `preserve-3d` participation inside a compositor group
- flattening boundaries via render-to-texture
- correct GPU depth testing among participating surfaces
- backface visibility
- ordinary browser content staying on the existing 2D path

They are **not** enough for exact general handling of all intersecting 3D surfaces if you want mathematically exact order for arbitrary penetrating quads. That requires polygon splitting or a more complex surface decomposition pass.

Recommendation: do **not** make plane splitting part of the initial Makepad/HAVI CSS 3D compositor plan.

What is required in the first version beyond projected quads and offscreen surfaces is projected clipping. That can be done in shader space with clip planes or projected polygon tests rather than full backend stencil infrastructure.

---

## Strategy A: in-Makepad API

### Viability

Technically viable.

This would add a first-class compositor surface/quad API inside existing Makepad crates, likely in `makepad-draw` plus small `makepad-platform` backend changes.

This is viable because the critical low-level pieces already exist:

- 4x4 matrices
- custom geometry
- render-to-texture passes
- depth buffers
- shader-driven drawing

### API surface I would expose

At minimum:

```rust
pub struct CompositorSurface {
    pub color: Texture,
    pub depth: Option<Texture>,
    pub size: Vec2d,
}

pub struct CompositorNode {
    pub texture: Texture,
    pub local_rect: Rect,
    pub uv_rect: Rect,
    pub transform: Mat4f,
    pub opacity: f32,
    pub premultiplied: bool,
    pub backface_visible: bool,
    pub depth_test: bool,
    pub depth_write: bool,
    pub clip_mode: CompositorClipMode,
    pub clip_planes: SmallVec<[Vec4f; 8]>,
}

pub enum CompositorClipMode {
    None,
    ScreenPlanes,
}

pub struct CompositorPass {
    pub pass: DrawPass,
}

impl CompositorPass {
    pub fn begin_surface(...)
    pub fn end_surface(...)
    pub fn draw_node(...)
    pub fn draw_nodes(...)
}
```

Optional higher-level grouping API if you want Makepad itself to understand flattening/preserve-3d boundaries:

```rust
pub enum CompositorGroupMode {
    Flat,
    Preserve3d,
}
```

I would **not** put browser semantics such as CSS transform-style enums or DOM concepts into Makepad. Only generic compositor concepts.

### Narrowest implementation inside Makepad

The narrowest useful addition is not a full scene graph. It is:

1. a new textured projected-quad draw shader and wrapper
2. small helpers for offscreen surface allocation via child passes
3. backend fixes for per-draw depth-write
4. optional projected clip-plane uniform path

That is enough for HAVI to own the actual compositor tree logic.

### Files/modules to modify

Likely:

- `draw/src/lib.rs`
- new `draw/src/shader/draw_compositor_quad.rs`
- optionally new `draw/src/compositor.rs`
- `platform/src/draw_shader.rs` if adding new draw options
- `platform/src/draw_vars.rs` if adding option plumbing or helper setters
- `platform/src/os/apple/metal.rs`
- `platform/src/os/windows/d3d11.rs`
- `platform/src/os/linux/opengl.rs`
- `platform/src/os/linux/vulkan.rs`
- `platform/src/os/web/web_gl.rs`

Possible shader-language touch points only if desired:

- fixed-size uniform arrays or nicer uniform-buffer ergonomics for clip planes

### Estimated LOC

Minimal useful version:

- new compositor quad draw wrapper + shader: `350-600`
- small compositor surface helper layer: `200-400`
- depth-write fixes across backends: `250-500`
- projected clip-plane support in shader/API: `150-350`
- tests/examples/docs: `200-350`

**Total: `1,150-2,200 LOC`**

If you try to make Makepad itself own compositor groups and flattening logic, increase that to roughly:

**`2,000-3,500 LOC`**

### Main risks / blockers

1. **API scope creep**
   - easy to accidentally build browser-specific semantics into Makepad
   - wrong abstraction boundary

2. **Backend consistency work**
   - per-draw depth-write is not consistently wired today
   - Vulkan/D3D11/OpenGL/Web need cleanup

3. **Projected clipping design**
   - if you insist on backend stencil/scissor correctness, scope grows quickly
   - shader-space clip planes are the right first step

4. **This duplicates logic HAVI already owns**
   - HAVI already knows about paint order, flattening, and 2D layout
   - moving too much of that into Makepad is unnecessary

### Bottom line on Strategy A

Viable, but only if kept very narrow.

The right Strategy A is not “put CSS 3D in Makepad.” It is “add a generic projected-surface compositor primitive to Makepad.”

---

## Strategy B: companion `makepad-compositor` crate

### Viability

This is the best fit and is technically viable.

A companion crate can sit on top of existing Makepad low-level APIs:

- `DrawPass`
- `Texture`
- `Geometry`
- `DrawVars`
- custom shaders
- child-pass render-to-texture

It can own the compositor model needed by HAVI without polluting core widget/render abstractions.

### Why this matches the problem better

HAVI already does:

- layout
- paint ordering
- browser-side flattening decisions
- DOM/CSS semantics

What it needs from Makepad is a rendering/compositor backend, not a second browser engine.

A companion crate can model exactly the missing layer:

- surfaces
- nodes
- preserve-3d groups
- flattening to texture
- projected textured quads
- GPU depth handling
- projected clip planes

### API surface I would expose

I would build the companion crate around three concepts.

#### 1. Surface allocation / render target

```rust
pub struct MpSurface {
    pub color: Texture,
    pub depth: Option<Texture>,
    pub pass: DrawPass,
    pub size: Vec2d,
}

impl MpSurface {
    pub fn begin(cx: &mut Cx2d, parent: Option<&DrawPass>, size: Vec2d, with_depth: bool) -> Self;
    pub fn end(&mut self, cx: &mut Cx2d);
}
```

#### 2. Submitted compositor primitive

```rust
pub struct MpCompositedQuad {
    pub texture: Texture,
    pub uv_rect: Rect,
    pub local_rect: Rect,
    pub transform: Mat4f,
    pub opacity: f32,
    pub premultiplied: bool,
    pub backface_visible: bool,
    pub depth_test: bool,
    pub depth_write: bool,
    pub clip_planes: SmallVec<[Vec4f; 8]>,
}
```

#### 3. A compositor encoder / renderer

```rust
pub struct MpCompositor {
    // internal quad geometry, shader state, temp buffers
}

impl MpCompositor {
    pub fn begin_frame(&mut self, cx: &mut Cx2d, target_pass: &DrawPass, viewport: Rect);
    pub fn draw_quad(&mut self, cx: &mut Cx2d, quad: &MpCompositedQuad);
    pub fn draw_batch(&mut self, cx: &mut Cx2d, quads: &[MpCompositedQuad]);
    pub fn end_frame(&mut self, cx: &mut Cx2d);
}
```

This is enough for HAVI to decide:

- when to flatten into a surface
- when descendants stay in a shared preserve-3d context
- what projected clip planes apply
- what order to submit non-depth-writing translucent quads in

### Exact Makepad capabilities this strategy can reuse immediately

1. **4x4 matrix math**
2. **custom geometry and draw shader pipeline**
3. **render-to-texture passes**
4. **texture sampling in shader**
5. **depth buffers on passes**

### Small Makepad extensions still needed under Strategy B

I would still upstream a few narrow changes to Makepad itself.

#### Required

1. **Per-draw depth-write support on all backends**
   - Metal already has it
   - add equivalent handling for Vulkan, D3D11, OpenGL, WebGL

2. **A dedicated projected-quad shader wrapper or enough low-level access to build one cleanly**
   - this can live in the companion crate if preferred

#### Nice to have, not mandatory

3. **Small shader-language convenience for clip planes**
   - not required if encoded as fixed uniforms or uniform buffer fields
   - useful if you want cleaner array-style clip plane submission

4. **Optional scissor API**
   - not needed for first compositor version
   - shader-space projected clipping is enough

### Files/modules to modify

New companion crate, likely under Makepad workspace:

- `makepad-compositor/src/lib.rs`
- `makepad-compositor/src/surface.rs`
- `makepad-compositor/src/quad.rs`
- `makepad-compositor/src/shader.rs`
- `makepad-compositor/src/batch.rs`
- `makepad-compositor/examples/...` or test harness

Small upstream Makepad touches:

- `draw/src/lib.rs` only if re-export desired
- maybe no changes in `draw/` if crate directly uses `makepad_draw` and `makepad_platform`
- `platform/src/os/apple/metal.rs`
- `platform/src/os/windows/d3d11.rs`
- `platform/src/os/linux/opengl.rs`
- `platform/src/os/linux/vulkan.rs`
- `platform/src/os/web/web_gl.rs`

### Estimated LOC

Companion crate itself:

- surface management: `150-300`
- projected quad geometry/shader: `300-550`
- compositor batching/encoder: `250-500`
- projected clip planes / backface / opacity handling: `150-300`
- tests/examples/docs: `200-400`

**Subtotal: `1,050-2,050 LOC`**

Small upstream Makepad fixes:

- per-draw depth-write across backends: `250-500`
- optional helper cleanup / exports: `50-150`

**Grand total: `1,350-2,700 LOC`**

This is similar raw size to Strategy A, but it keeps ownership in the right place.

### Main risks / blockers

1. **Backend depth-write inconsistency**
   - real blocker for robust compositor blending behavior
   - fixable, medium-sized

2. **Projected clipping policy**
   - need to decide clip-plane representation up front
   - fixable in shader without backend stencil work

3. **Surface lifetime / pooling**
   - not hard, but needed for performance
   - should live in the companion crate, not core Makepad

4. **Hit-testing boundary**
   - must be explicitly left to HAVI
   - do not attempt widget-level projected hit test in v1

### Bottom line on Strategy B

Viable and clean.

This gives HAVI exactly the layer it needs while minimizing Makepad core changes.

---

## Recommended path

## Recommendation: Strategy B, companion `makepad-compositor` crate

This is the better architecture.

### Why

1. **The missing piece is compositor policy, not low-level GPU capability**
   - Makepad already has enough GPU primitives
   - what is missing is a focused surface/quad compositor abstraction

2. **HAVI already owns browser semantics**
   - flattening rules
   - preserve-3d participation
   - paint order decisions
   - clip propagation rules

3. **Core Makepad should stay general-purpose**
   - projected textured quads are generic
   - CSS-specific tree semantics are not

4. **The required core fixes are narrow**
   - per-draw depth-write on non-Metal backends
   - maybe tiny shader convenience for clip planes

### Concrete first implementation plan

#### Phase 1: narrow Makepad fixes

1. Fix per-draw `depth_write` on:
   - `platform/src/os/linux/vulkan.rs`
   - `platform/src/os/windows/d3d11.rs`
   - `platform/src/os/linux/opengl.rs`
   - `platform/src/os/web/web_gl.rs`

2. Do not extend widget hit-testing.

3. Do not add plane splitting.

#### Phase 2: new `makepad-compositor` crate

Implement:

- offscreen surface helper using `DrawPass + Texture`
- projected textured quad shader using existing matrix support
- backface visibility
- premultiplied alpha blending expectations
- optional clip-plane uniform path
- batch submission API for HAVI

#### Phase 3: HAVI integration

HAVI uses the companion crate to:

- allocate flattening surfaces where CSS requires flattening
- submit preserve-3d descendants directly into a shared 3D compositor context
- submit flattened descendants as textures
- use GPU depth where surfaces remain in the same preserve-3d group
- keep ordinary 2D content on existing fast paths

### What not to build first

- generic Makepad widget-level projected hit testing
- plane splitting / BSP
- full stencil/projected polygon clip backend infrastructure
- CSS-specific scene graph inside Makepad core

---

## Estimated LOC table

| Work item | Files | LOC estimate |
|---|---:|---:|
| Backend per-draw depth-write fixes | `platform/src/os/*` | 250-500 |
| Projected textured quad shader/wrapper | new draw or companion files | 300-600 |
| Offscreen surface helper | companion crate or draw helper | 150-300 |
| Batch encoder / node submission | companion crate | 250-500 |
| Clip-plane support | companion shader/API | 150-300 |
| Tests/examples/docs | companion + examples | 200-400 |
| **Strategy A total, narrow in-Makepad API** | mixed | **1,150-2,200** |
| **Strategy B total, companion crate + small Makepad fixes** | mixed | **1,350-2,700** |
| Strategy A if Makepad owns compositor groups too | mixed | 2,000-3,500 |

---

## Key source observations

### 2D renderer is still fundamentally rect-clip-first

- `draw/src/shader/draw_quad.rs` clips local rects with `draw_clip` and `view_clip` before final projection.
- `platform/src/area.rs` reconstructs clipped rects as axis-aligned rectangles.
- `platform/src/event/finger.rs` hit-tests those rectangles.

That is correct for existing UI and wrong for browser CSS 3D semantics.

### The matrix path is more capable than the widget path

- `DrawListUniforms.view_transform` is a full `Mat4f`
- `DrawList2d::map_point_to_local` already does inverse + perspective divide

This is enough for a compositor layer, not for widget hit-testing.

### Offscreen pass composition is already real

- `widgets/src/view.rs` proves Makepad can render subtrees into textures and composite them later.

This is the core flattening primitive required by CSS 3D.

### Existing 3D API is not the compositor API you want

- `DrawPbr` is useful as proof of backend capability.
- It is not the right abstraction for browser surface composition.

### `DrawPbr` clip/depth API currently overstates what exists

- `clip_ndc`, `depth_range`, `depth_forward_bias` are present in Rust state and uploaded as uniforms.
- They are not consumed by the shader.

So current `Scene3D` is not already a reusable CSS compositor substrate.

### Draw-matrix tree exists only as an unfinished stub

- `platform/src/draw_matrix.rs` should not be the basis of the design.

---

## Final conclusion

Makepad already has enough low-level rendering machinery for a proper HAVI CSS 3D compositor path:

- full 4x4 math
- GPU geometry submission
- render-to-texture passes
- texture sampling
- depth buffers

What it does **not** already have is the right compositor-facing API.

The missing work is medium-sized, not large.

The right design is:

- keep Makepad core changes small and generic
- add per-backend depth-write correctness
- build a companion `makepad-compositor` crate around projected textured quads and offscreen surfaces
- let HAVI continue to own browser flattening and preserve-3d semantics

Projected quads + offscreen surfaces are enough for the first real implementation.
Projected clipping is required.
Plane splitting is not a first-version requirement.

---

## Executive summary

Makepad is not a browser compositor today, but it is close enough at the GPU primitive level that HAVI does **not** need a renderer rewrite.

What Makepad already has:

- 4x4 transform math everywhere it matters
- draw lists carrying `Mat4f`
- child passes and render-to-texture
- depth textures and 3D rendering
- arbitrary geometry + custom shaders

What is missing:

- a focused projected-surface compositor API
- projected clipping
- consistent per-draw depth-write across all backends
- any notion of preserve-3d / flattening groups at the API layer
- projected hit-testing in widgets, which should be left out of scope

Decision:

- **Strategy A: direct in-Makepad API** is viable only if kept narrow.
- **Strategy B: companion `makepad-compositor` crate** is the better choice.

Recommended path:

1. fix per-draw depth-write in Makepad backends
2. create `makepad-compositor`
3. implement offscreen surfaces + projected textured quads + clip planes
4. keep CSS semantics in HAVI
5. do not build plane splitting first

Best estimate:

- companion crate plus small Makepad fixes: **~1.35k-2.7k LOC**
- enough for a real CSS 3D compositor path for HAVI while preserving the current fast 2D path for ordinary content
