# Surface authoring

These are entries in `model.apply.operations`. Each operation has `op` and the
fields below. Unknown and duplicate fields fail. Material IDs are u32 JSON
integers; vertex IDs and pattern seeds are canonical decimal strings. Colors and factors
are finite linear numbers in 0..1. Layer PNG/raw RGB bytes use sRGB for base color
and emissive, linear normalized bytes for data and normal channels. UV0 follows
the image row direction: row zero corresponds to V=0.

Channels: `base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive`.
Metallic/roughness packs roughness in G and metallic in B. Normal RGB is a
normalized tangent-space vector encoded from -1..1 to 0..1. Occlusion uses R.

- `surface_material {material,base_color?,metallic?,roughness?,normal_scale?,occlusion_strength?,emissive?,emissive_strength?,alpha?,alpha_cutoff?,double_sided?,fur?}`
  sets factors while preserving an existing layer stack; on first use it seeds
  a `legacy` base-color layer from the material's existing PNG. Unspecified factors use
  defaults: base RGBA1, metal0, rough1, normal scale1, AO strength1, emissive RGB0,
  strength1, opaque, cutoff0.5, double-sided false. `alpha` is `opaque|mask|blend`.
  Normal scale is 0..16; emissive strength is 0..100000. Material must already
  exist (the ordinary `material` operation creates one).
  This replaces factors rather than patching them: inspect the current material
  and repeat its other factors when changing just roughness or emission.
  `fur:{length:0.015,density:0.65,scale:180,seed:0}` adds short procedural fur:
  length 0.001–0.05 model metres, density 0.05–1, scale 20–1000 strand cells per
  metre, integer seed 0–65535. These are also the defaults for omitted fields
  inside `fur`. Omitted/null `fur` disables it; repeat it when changing factors.
  Fur reuses the mesh and skin weights in at most six shader shells, with
  distance reduction, a 12,000 extra-triangle ceiling per model instance and
  a 96,000 ceiling per view. It adds no editable strands or collider geometry.
  Prefer it for a coat of short fur/fuzz, especially on Quest. Layered rendering
  still costs vertex work and overdraw; keep the base mesh economical. Long
  hair, individually modeled strands and strand physics need other geometry.
  The recipe persists in model source and GLB makepadFur material extras;
  generic GLB viewers show the base PBR surface.
- `surface_remove_material {material}` removes the rich sidecar and restores the
  legacy material's factors/image. This explicitly removes its retained layers.
- `surface_layer {material,channel,layer,width,height,color?,rgba_hex?,mask_hex?,opacity?,blend?,visible?}`
  appends a named layer, or replaces that same layer while retaining stack order.
  `color` is linear RGBA, defaulting to the channel's neutral value. `rgba_hex` is
  exactly width*height*4 lowercase hex bytes and conflicts with `color`.
  Alternatively `{material,channel,layer,png_hex,...}` imports a complete RGBA PNG;
  width/height/color/rgba_hex must then be omitted. Optional opacity defaults1,
  blend `over|multiply|add|subtract` defaults over, visibility defaults true.
  `mask_hex` is one linear byte per pixel. No external image URI is accepted.
- `surface_remove_layer {material,channel,layer}` removes an existing layer.
- `surface_move_layer {material,channel,layer,index}` changes stack order; index is
  zero-based in the final stack. Bottom is0, top is last.
- `surface_mask {material,channel,layer,mask_hex}` replaces the mask; null removes
  it. A mask byte is coverage0..255, independent of pixel alpha.
- `surface_pattern {material,channel,layer,pattern:{kind,color_a,color_b,scale?,seed?}}`
  fills the layer deterministically and retains the pattern recipe. Kind is
  `checker|stripes|gradient|noise|perlin|fbm|yarn`; colors are RGBA. Scale
  defaults[8,8], each component is >0 and <=4096. Seed defaults"0". Gradient
  repeats across U. Perlin is smooth gradient noise, fbm sums four seeded noise
  octaves, and yarn forms curved strands with fine fibers. These three patterns
  tile continuously at integral scale components; nonintegral scales crop the
  field. The original `noise` remains discrete cell noise.
- `surface_derive {material,source_channel,source_layer?,channel,layer,strength?,roughness_min?,roughness_max?,metallic?,wrap?}`
  creates/replaces a named `normal` or `metallic_roughness` layer from height.
  A legacy base-color PNG is seeded on first use, as with `surface_layer`.
  Omit `source_layer` to use the composed visible source channel (bitmap and
  procedural layers with their masks/blends); name a layer to use it even when
  hidden, including its mask and opacity. Source `base_color`/`emissive` uses
  linear-light luminance, `occlusion` uses R, `metallic_roughness` uses G. Normal
  maps cannot be height sources. Transparent coverage contributes zero height;
  derived data maps are opaque, including neutral normals in empty regions.
  Normal output uses centered UV derivatives, positive height toward +Z, and
  strength0..16 (default1); constant height gives neutral [128,128,255]. Strength
  measures height per UV unit so resolution changes do not change bump amplitude.
  A rise toward increasing U or V tilts the corresponding normal component
  negative. Worker clients use `SurfaceDerivation::derive_image` for the same
  deterministic conversion; this does not change imported/generated normal maps.
  `wrap` defaults true and samples opposite edges for periodic height; false
  clamps edge samples. Wrapping cannot make a nonperiodic input seamless.
  Roughness maps height0..1 to min..max (defaults0.2..0.9); min/max and metallic
  (default0) are0..1 and min<=max. Output packs roughness in G and metallic in B;
  R is255. Deriving sets the target normal-scale or metal/rough factors to1 so
  the declared output is applied once. Existing target mask, blend, opacity and
  visibility are retained (a mask of incompatible dimensions is removed).
  The height source and its recipes remain intact. The output retains its
  derivation recipe and pixels as a **snapshot**: rerun `surface_derive` after
  source edits; there is no implicit dependency refresh. Replacing the source
  itself is rejected. Painting/pattern/dilation of derived pixels clears their
  derivation recipe. No image byte payload is required by this operation.
- `surface_stroke {material,channel,layer,points,radius,color,hardness?,opacity?,mask?}`
  paints the continuous UV polyline with1..256 two-component points. Radius is
  positive in UV units; hardness defaults0.5, opacity1, mask false. Hardness0 is a
  linear radial falloff and1 is a hard disk. Mask painting uses color.R.
- `surface_projected_stroke {object,material,channel,layer,origin,direction,radius,depth,color,hardness?,opacity?,mask?}`
  projects one circular brush through a cylinder on front-facing visible
  triangles. Origin/direction are 3-vectors in supplied mesh coordinates, radius
  and positive depth use the same units. The document host evaluates scene
  coordinates before invoking this operation. Multiple operations form a stroke.
- `surface_vertex_paint {object,vertices,color,opacity?}` blends linear RGBA into
  stable vertex IDs (default white). New vertices produced by geometry edits
  receive closest-source-surface barycentric color interpolation. Derived LODs
  preserve paint and resample moved/new vertices from the source surface; the
  editable mesh and its paint remain unchanged.
- `surface_bake {source,target,material,channel,layer,width,height,max_distance,ao_samples?,ao_distance?,dilation?}`
  rasterizes target UV0 and transfers nearest high-resolution source attributes
  using a bounded triangle BVH. Positive max_distance bounds correspondence.
  Normal output is tangent-space and includes source normal texture/scale.
  Occlusion casts deterministic cosine hemisphere rays:1..64 samples(default16),
  positive distance(default1). Base color includes vertex paint and material
  factors; metal/rough and HDR emissive factors are also transferred. HDR emission
  is normalized into one target strength without clipping. Dilation defaults2.
  Bake creates/replaces the named layer and normalizes that channel's target
  factors so transferred factors are applied exactly once. No matched texels is
  an error. Target UVs must lie in0..1; overlapping UVs producing conflicting
  transferred values are rejected. Separate source/target objects may share a
  material, in which case editing that shared material affects both.
- `surface_dilate {material,channel,layer,iterations,preserve_alpha?}` propagates
  nearest covered neighboring texels by0..64 iterations. Preserve-alpha defaults
  true and fills RGB gutters without changing coverage. Bake uses false to fill
  uncovered texels. Painting/pattern/dilation invalidates derived mip levels.
- `surface_mips {material,channel,layer}` retains a full mip chain down to1x1.
  Color filtering uses linear light and premultiplied alpha; normal vectors are
  renormalized; data channels average directly. Odd edge texels are included.

A material supports at most32 layers per channel. Images obey document limits
(default2048 dimension,8MiB decoded RGBA per image); total source, transaction,
worker-memory, and work budgets still apply. Operations check cancellation and
publish state only after all validation succeeds. Source checkpoints retain
pixels, masks, pattern/derivation recipes, layers, vertex colors and requested mip chains.

Compile flattens visible layers, preserves source UVs, emits tangent vectors and
RGBA vertex colors, and embeds PNGs for all five standard glTF PBR channels.
`KHR_materials_emissive_strength` carries HDR strength. Materials preserve alpha
mode, cutoff and double-sided state. Export regenerates channel-aware composite
mips; optional `material.extras.makepadMips` maps standard texture field names to
embedded IMAGE indices for levels1..N. Ordinary glTF clients can use the standard
level-zero image and mip-filtering sampler. Rich skinned materials keep separate
primitive material slots; documents without sidecars retain the legacy shared
opaque atlas contract. All GLB resources are self-contained.

Vertex paint targets owned mesh vertices. Run `make_unique` before painting a
linked instance independently; baking and projected strokes evaluate linked
source geometry through its object transform.
