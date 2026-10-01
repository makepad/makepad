//! The world scene pass, moved verbatim from gamemaker's game_view.rs:
//! sky dome → terrain mesh → opaque per-shape batches → alpha per-shape
//! batches. Statics come from packed instance slabs cached against
//! `world.render_rev`; dynamics re-pack every frame.

use makepad_draw::*;
use crate::custom_material::CustomMaterial;
use makepad_scene::{
    entity_index_sorted, BodyKind, ChunkKey, Entity, World, Part, Shape, Terrain, TerrainMaterials,
    VoxelView, WaterView, WaterSurface, MAX_WAVES,
};

use crate::bake::{BakeSettings, BakeStats, LightBake};
use crate::geometry::shape_geometry_data;
use crate::light_grid::{
    merge_transients_into_block, LightBlock, LightGrid, LIGHT_BLOCK_FLOATS, LIGHT_GRID_CELL,
};
use crate::shaders::{
    DrawSceneAlpha, DrawSceneCube, DrawSceneDecal, DrawSceneFlare, DrawSceneScreen, DrawSceneShadow,
    DrawScenePbr, DrawSceneShadowSdf, DrawSceneSkinned, DrawSceneSkinnedGpu, DrawSceneSky,
    DrawSceneSkyAnalytic,
    DrawSceneSkyMap,
    DrawSceneTerrain, DrawSceneViewModel, DrawSceneWater,
};
use crate::shadow_mesh::{
    caster_points, shadow_drop_params, Receiver, ShadowMeshBuilder, SHADOW_NORMAL_BIAS,
    SHADOW_SLOPE_BIAS,
};
use crate::model::StaticModel;
use crate::stage::{Stage, StageMode};
use crate::particles::ParticleInstance;
use crate::shaders::DrawSceneFirework;
use crate::sun::SunLight;
use crate::thermometer::{Quality, Thermometer};
#[path = "fast_gi/scene.rs"]
mod gi_scene;

/// The host widget's themed draw structs, lent to the renderer per frame.
/// They stay `#[live]` fields on the widget so script-side styling applies.
pub struct SceneDraws<'a> {
    pub cube: &'a mut DrawSceneCube,
    pub alpha: &'a mut DrawSceneAlpha,
    pub sky: &'a mut DrawSceneSky,
    /// The analytic (Preetham) sky + stars, drawn for default-sky worlds.
    /// Optional so a host that has not adopted it keeps the gradient dome.
    pub sky_analytic: Option<&'a mut DrawSceneSkyAnalytic>,
    pub terrain: &'a mut DrawSceneTerrain,
    /// Silhouette shadow mesh (shadow_mesh.rs). Optional so a host that has
    /// not adopted it yet keeps the blob tier.
    pub shadow: Option<&'a mut DrawSceneShadow>,
    /// Fireworks. Optional so a host that does not want a sky show pays
    /// nothing — not even the shared spark geometry.
    pub firework: Option<&'a mut DrawSceneFirework>,
    /// Old-school lamp lens flares (one additive billboard per visible
    /// street lamp). Optional the same way the fireworks are.
    pub flare: Option<&'a mut DrawSceneFlare>,
    /// SDF silhouette shadows (shadow_sdf.rs) — THE dynamic shadow tier:
    /// characters and driven cars draw one morphing quad each from their
    /// baked atlas. Optional: a host without it keeps the blob tier.
    pub shadow_sdf: Option<&'a mut DrawSceneShadowSdf>,
    /// The wave-displaced water sheet (mix.md W1). Optional: a host without
    /// it renders `game.water` volumes as nothing (their touch sensor is
    /// hidden) — the sandbox passes it; the retiring gamemaker host does not.
    pub water: Option<&'a mut DrawSceneWater>,
    /// In-world video screen: one textured quad whose texture the host
    /// updates per frame. The host positions it (`screen_pos`/`screen_size`)
    /// and binds the texture; a zero `screen_size.x` draws nothing.
    pub screen: Option<&'a mut DrawSceneScreen>,
    /// Extra camera-facing quads (map-placed billboards). Drawn with
    /// `screen`'s shader; empty when the host has none.
    pub screen_instances: &'a [ScreenInstance],
    /// Camera-space held model, isolated from world shading/caster paths.
    pub view_model: Option<&'a mut DrawSceneViewModel>,
}

/// One world-space quad. `pos.xyz` is the centre, `pos.w` is yaw
/// (camera yaw faces the orbit/walk camera). `size.xy` is width/height in
/// world units; `size.zw` retains the texture's pixel size for callers that
/// need the sheet metadata. Pixel-art sampling itself uses the shader's
/// exact nearest fetch and therefore does not depend on half-texel maths.
///
/// The quad stands UPRIGHT (its second axis is world +Y) unless `size.w` is
/// NEGATIVE, which asks for a FLOOR-ALIGNED card lying flat on the ground
/// plane — what a top-down tiled map draws, where the artwork already
/// encodes the oblique view and a standing quad would be seen edge-on. The
/// magnitude is the sheet's pixel height either way, so a caller that never
/// heard of floor cards keeps exactly the behaviour it had.
///
/// A NEGATIVE `size.z` (the sheet's pixel WIDTH, positive for every other
/// caller) asks for a GROUND-ANCHORED billboard: `pos.xyz` is the point the
/// sprite STANDS on rather than its centre, and every fragment of the quad
/// takes that ground point's depth. The second half is what makes a crowd of
/// standing sprites sort the way a player reads them — by where each piece
/// stands, not by how tall it is — while the world still occludes them
/// normally. Such an instance spends `size.z` on the request instead of the
/// sheet width: its magnitude is `1 + pad`, `pad` being the transparent
/// padding under the figure as a share of `size.y`, so the DRAWING touches
/// down rather than the empty cell it is packed in. The two flags are
/// independent; a floor card (`size.w < 0`) ignores this one.
#[derive(Clone)]
pub struct ScreenInstance {
    pub texture: Texture,
    pub pos: Vec4f,
    pub size: Vec4f,
    /// Sub-rectangle of the texture this quad shows, `(u0, v0, u1, v1)`.
    /// `u0 > u1` draws it X-mirrored. A whole-texture quad passes
    /// `(0, 0, 1, 1)`.
    pub uv: Vec4f,
    /// Per-instance albedo multiplier. White preserves the authored sprite.
    pub tint: Vec4f,
    /// Hue degrees, saturation multiplier, value multiplier, reserved.
    pub color_adjust: Vec4f,
}

/// Per-frame render counters, handed back for the host's profiler.
#[derive(Default, Clone, Copy)]
pub struct RenderStats {
    /// Extra triangles actually submitted for material fur this frame.
    pub fur_triangles: usize,
    /// Grass patches drawn (one instance each) and their blade budget.
    pub grass_patches: usize,
    pub grass_blades: usize,
    pub gi: crate::fast_gi::GiStats,
    /// Device-local clustered light assignment/upload cost and overflow.
    pub clustered: crate::clustered::ClusterStats,
    pub slab_rebuilds: u64,
    pub slab_us: u64,
    pub static_instances: u64,
    pub dyn_instances: u64,
    /// Sky dome drawn this frame (suppressed on an MR stage — the room is
    /// the environment). Tests assert the suppression.
    pub sky_drawn: bool,
    /// Firework shells drawn — one GPU instance each.
    pub firework_shells: u64,
    /// Lamp flare billboards drawn — one GPU instance each.
    pub flares: u64,
    /// Resident bullet-hole quads submitted this frame (bounded at 64).
    pub bullet_decals: u64,
    /// Prop draws that bound a baked AO atlas, and that did not.
    pub ao_bound: u64,
    pub ao_missing: u64,
    /// Terrain mesh drawn this frame (suppressed on an MR stage).
    pub terrain_drawn: bool,
    /// Shadow-catcher quad drawn under the diorama (MR only).
    pub shadow_catcher_drawn: bool,
    /// Cast shadows drawn this frame (both tiers).
    pub shadows: u64,
    /// How many of those were full projected silhouettes rather than blobs.
    pub projected_shadows: u64,
    /// Device-local particles drawn this frame.
    pub particles: u64,
    /// Stock props placed this frame, and how many draw items they cost.
    /// `model_draws` < `model_instances` is the batching working: copies of
    /// one prop share a draw item.
    pub model_instances: u64,
    pub model_draws: u64,
    pub model_triangles: usize,
    /// In-world presentation props attached to moving actors (for example a
    /// third-person held weapon). Counted separately because these meshes do
    /// not belong to the placed scene or any of its bake/shadow inputs.
    pub world_attachment_instances: u64,
    pub world_attachment_draws: u64,
    pub world_attachment_triangles: usize,
    /// Private camera-space presentation cost (normally one 70-triangle
    /// weapon). Kept separate so world-prop profiling remains comparable.
    pub view_model_instances: u64,
    pub view_model_triangles: usize,
    /// What the CPU light bake cost (bake.rs). Zero on frames it skipped.
    pub bake: BakeStats,
    /// Floats per cube instance, read from the compiled shader rather than
    /// counted by hand — this is the number the bandwidth budget is about.
    pub instance_floats: u32,
    /// CPU frustum culling: instances skipped before packing/upload this
    /// frame. All zero on XR stages, where the runtime owns the eye matrices
    /// and the CPU cannot know the true frustum (see stage.rs).
    pub model_culled: u64,
    pub world_attachment_culled: u64,
    pub dyn_culled: u64,
    pub skinned_culled: u64,
    /// Bytes of skinning data uploaded this frame: the joint-palette texture.
    /// The rest meshes upload once at rig load and never again — this number
    /// replacing the full posed vertex streams is the point of GPU skinning.
    pub skin_upload_bytes: u64,
    /// World-grid chunk culling, per kind: static instance slab cells,
    /// terrain tiles. drawn+culled = total.
    pub chunks_drawn: u64,
    pub chunks_culled: u64,
    pub terrain_tiles_drawn: u64,
    pub terrain_tiles_culled: u64,
    /// Per-frame CPU spent on DYNAMIC shadows (character/car anchors +
    /// pre-delivery blob builds + the mesh upload + SDF instance pushes).
    /// The number the baked SDF tier exists to hold near zero — anchors
    /// only — independent of caster count.
    pub dyn_shadow_us: u64,
    /// Realtime sun cascades this frame: static casters, movers (each drawn
    /// into all three cascades) and the CPU encode time.
    pub csm_static_casters: u64,
    pub csm_movers: u64,
    pub csm_encode_us: u64,
    /// Caster instances the cascades drew after per-cascade culling, and the
    /// cascade pass's smoothed GPU time.
    pub csm_draws: u64,
    pub csm_gpu_ms: f32,
    /// Dynamic shadows drawn through the GPU SDF-silhouette quad
    /// (characters + driven cars).
    pub sdf_shadow_instances: u64,
}

/// Diagnostic settings the host passes in ([`Renderer::set_host_settings`]).
/// The engine reads no environment variables for these: a host maps its own
/// switches onto this struct.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostSettings {
    /// The world shader shows the baked light alone.
    pub lm_debug: bool,
    /// Log the mean particle pass GPU time every 120 frames.
    pub vfx_stats: bool,
}

/// GPU-side caches for one view family: unit shape geometries, the packed
/// static instance slabs, and the terrain mesh. Owns no draw structs — see
/// [`SceneDraws`].
pub struct Renderer {
    // PERF: unit shape geometries, built once (index = Shape::index()).
    shape_geometries: [Option<Geometry>; 5],
    /// Primitive positions in the shadow shader's packed mesh layout.
    shadow_shape_geometries: [Option<Geometry>; 5],
    // PERF: packed static instance data per shape (opaque / alpha passes),
    // valid while slab_rev == world.render_rev.
    static_chunks: Vec<SlabChunk>,
    /// Per-frame visibility scratch, index-aligned with `static_chunks`;
    /// reused so a steady scene does not reallocate.
    chunk_visible: Vec<bool>,
    /// [`static_slab_key`] — the slabs carry the entities' colours AND the
    /// baked light in those colours, so a repaint (`paint_rev`) and a
    /// rebake invalidate them exactly like a world edit does. The light
    /// bake itself keys on [`lightmap_world_key`], which a repaint never
    /// moves.
    slab_key: Option<(u64, u64, u64)>,
    slab_instance_count: u64,
    /// GPU mesh for the smooth terrain, rebuilt when the revision changes.
    terrain_tiles: Vec<TerrainTile>,
    terrain_revision: u64,
    /// GPU meshes for voxel terrain chunks (mix.md T2/T3), keyed by chunk
    /// and mesh revision — re-uploaded per chunk when a dig remeshes it.
    /// Drawn through the SAME terrain shader/lightmap path as the tiles.
    voxel_tiles: Vec<VoxelTile>,
    /// One flat grid per `game.water` volume (W1), displaced in the vertex
    /// shader by the sim's wave sum. Rebuilt when `WaterView::rev` moves.
    water_tiles: Vec<WaterTile>,
    water_rev: Option<u64>,
    /// REST meshes for GPU-skinned rigs — geometry plus the rig's rest-pose
    /// AO chart atlas — keyed by rig id and uploaded ONCE
    /// ([`Self::upload_skin_rig`]). Skinning happens in the vertex shader
    /// against the frame's joint-palette texture, so a character's per-frame
    /// upload is its palette, not its vertices (see skin.rs).
    skin_rig_geometries: Vec<(u64, std::rc::Rc<Geometry>, Texture)>,
    skin_material_draws: std::collections::HashMap<u64,Vec<UploadedSkinMaterial>>,
    skin_lods:std::collections::HashMap<u64,Vec<(f32,UploadedSkinRig)>>,
    skin_morphs:std::collections::HashMap<u64,crate::asset_morph::UploadedMorph>,
    skin_prepared_sdf: std::collections::HashMap<u64,Option<(Texture,SdfMeta)>>,
    /// Every character's joint palette for this frame, packed into one
    /// RGBA32F texture ([`crate::skin::palette_texels`]); instances carry
    /// their first texel as `joint_base`. Kept at power-of-two height so
    /// texel-centre uvs are exact.
    skin_palette_tex: Option<Texture>,
    /// That texture's capacity in texels.
    skin_palette_texels: usize,
    /// This frame's per-item first palette texel, parallel to the skinned
    /// batch items (-1 = no palette). Packed ONCE before the GPU lightmap
    /// runs ([`Self::pack_skin_palettes`]) so the bake's skinned depth
    /// passes and the visible skinned draw read the same texture.
    skin_joint_bases: Vec<f32>,
    /// Static stock props, uploaded ONCE and keyed by asset id. A prop's
    /// vertices never change, so its per-frame cost is one instance. See
    /// model.rs.
    static_models: Vec<(String, LoadedModel)>,
    preview_originals: std::collections::HashMap<String, (LoadedModel, Option<String>)>,
    preview_new: std::collections::HashSet<String>,
    /// The map-sky shader, built lazily from its script type default (the
    /// baker owns its passes the same way). Owned rather than lent through
    /// [`SceneDraws`] because a map's sky has nothing for a host to theme,
    /// and every host would have had to adopt a new field to get one.
    sky_draw: Option<Box<DrawSceneSkyMap>>,
    /// Engine-owned bullet-hole shader. A host gets the default marks merely
    /// by drawing a World; no widget field or per-game wiring is needed.
    decal_draw: Option<Box<DrawSceneDecal>>,
    /// One-shot diagnostic guard for the only state in which a parsed sky
    /// can still disappear before submission: its lazy shader could not be
    /// constructed because the script VM was busy. The first miss is loud;
    /// subsequent frames keep retrying without flooding the app log.
    sky_draw_wait_logged: bool,
    /// Seconds of sky time — what the scrolling layers of a Quake sky ride.
    /// Advanced by [`Renderer::tick_sky`]; deliberately not wall-clock, so a
    /// paused game has a still sky and a capture is reproducible.
    sky_time: f32,
    /// Per model id: does this model cast into the GPU bake's sun-depth
    /// passes when it has no AO layout of its own? Absent = decided by size
    /// ([`casts_as_caster_only`]). The explicit answer exists because only
    /// the host knows a "model" is really a whole imported LEVEL, which must
    /// never shadow its own interior.
    model_casts_shadow: std::collections::BTreeMap<String, bool>,
    /// LOD chains of separately built models: base id -> (distance, id of
    /// the model drawn from that distance on). Folded into the base's own
    /// `lods` whenever the members are resident ([`Self::set_model_lod_chain`]).
    model_lod_chains: std::collections::BTreeMap<String, Vec<(f32, String)>>,
    model_lod_chains_dirty: bool,
    /// Where every triggered anim part currently is, keyed by what the host
    /// addressed ([`ModelTarget`]) and the part's node name. Absent = the
    /// part sits in its model's default state, so an untouched scene costs
    /// nothing and behaves exactly as it did before doors existed.
    model_anim_state: ModelStates,
    /// Stock props placed for this frame, set by the host before drawing.
    placed_models: Vec<ModelInstance>,
    /// Static placed-model layers that have no lightmap layout of their own.
    /// They are registered when the placed scene changes and feed Realtime
    /// CSM directly, rather than waiting for an atlas layout that may never
    /// exist (an imported editor scene commonly has none).
    csm_static_casters: Vec<crate::gpu_lightmap::GpuBakeMesh>,
    /// Runs of `csm_static_casters` with their bounds (rebuilt with it).
    csm_static_blocks: Vec<crate::gpu_lightmap::CasterBlock>,
    /// The streamed world (stream.rs / renderer/stream_draw.rs), its build
    /// workers (kept across sources) and this frame's streamed shadow
    /// casters, appended to `csm_static_casters` for the cascades.
    stream: Option<Box<stream_draw::StreamState>>,
    stream_workers: Option<stream_draw::Workers>,
    stream_casters: Vec<crate::gpu_lightmap::GpuBakeMesh>,
    /// Actor-attached presentation meshes in world space. They use the
    /// ordinary world material/depth path, but are deliberately outside the
    /// placed scene's identity, bakes, caster lists, lamp harvest and blob
    /// shadows. This is the third-person counterpart to `view_models`.
    world_attachments: Vec<ModelInstance>,
    /// Device/view-local presentation meshes, submitted separately from the
    /// world so an FPS held model can never enter scene identity, lightmap or
    /// shadow-caster ownership. Drawn in a late, depth-overlay layer.
    view_models: Vec<ModelInstance>,
    /// Eased sun visibility at the first-person camera (set_view_model_sun).
    view_model_sun: f32,
    /// Deterministic identity of the placed scene relevant to static
    /// lighting. Unlike the old length-only check this includes every
    /// static model, transform, depth order, and list slot. Dynamic
    /// transforms are deliberately excluded: cars move every frame without
    /// changing the baked scene.
    placed_scene_signature: Option<u64>,
    /// `rebuild_models_with_prefix`: the host's key for the list's leading
    /// copies, how many they are and their signature.
    placed_prefix: Option<(u64, usize, u64)>,
    /// How this device projects the world (flat / VR 1:1 / MR diorama).
    /// Applied as the scene draw list's view transform, so it costs one
    /// uniform and never invalidates the static slabs. See stage.rs.
    stage: Stage,
    /// How many casters get a projected silhouette before the rest fall
    /// back to blobs. A per-device dial: a Quest can afford fewer than a PC
    /// even though both cost one instance, because the projection math runs
    /// per caster per frame on the CPU.
    shadow_budget: usize,
    /// Device-local particles and decals for this frame (particles.rs,
    /// vfx/). Set by the host before drawing; never simulation state,
    /// never replicated.
    vfx: crate::vfx::VfxState,
    /// Those atlases on the GPU, keyed the same way.
    ao_textures: Vec<(String, Texture)>,
    /// Which pack each loaded model belongs to, for binding its atlas.
    model_pack: Vec<(String, String)>,
    /// The placed and attached lists' per-frame draw order (draw_models.rs).
    model_orders: [draw_models::ModelOrder; 2],
    /// The placed list's static copies grouped by place (draw_models.rs).
    placed_blocks: draw_models::PlacedBlocks,
    /// One shell per entry — this is the whole per-frame firework upload.
    firework_instances: Vec<crate::firework::FireworkInstance>,
    /// This frame's smoke clouds (see [`Renderer::set_smoke_volumes`]).
    smoke_volumes: Vec<crate::smoke::SmokeVolume>,
    smoke_draw: Option<Box<crate::smoke::DrawSceneSmoke>>,
    /// The shared spark sheet: SPARKS_PER_SHELL quads, built once and indexed
    /// by every shell.
    spark_geometry: Option<Geometry>,
    /// Scratch for this frame's silhouette shadow mesh; reused so a steady
    /// scene does not reallocate. Uploaded as one geometry, drawn once.
    shadow_mesh: ShadowMeshBuilder,
    /// Static box tops that catch draped shadows (road slabs, platforms),
    /// rebuilt with the static shadows and shared with the per-frame paths.
    receiver_boxes: Vec<(Vec3f, Vec3f)>,
    /// The LIGHTMAP's occluder boxes: every visible opaque static, whatever
    /// its shape class. Distinct from `receiver_boxes` on purpose — that
    /// list answers "what may a drape LAND on" (wide flat slabs), and
    /// borrowing it as the shadow-caster set punched box-shaped HOLES into
    /// the baked shadows of composite structures (only their flat members
    /// passed the receiver filter).
    occluder_boxes: Vec<(Vec3f, Vec3f)>,
    /// The baked static-light atlas (lightmap.rs): A = sun-visibility SDF,
    /// RGB = lamp light. None until the first background bake delivers.
    lightmap: Option<Texture>,
    /// 1x1 "no lightmap yet": fully sunlit, zero lamp light — bound wherever
    /// a real atlas isn't, so the shader samples unconditionally.
    lm_fallback: Option<Texture>,
    /// 1x1 mean-127 gray for props with no Q3/Unreal detail overlay.
    detail_fallback: Option<Texture>,
    /// 1x1 white for materials whose metallic/roughness are factors only.
    orm_fallback: Option<Texture>,
    /// The specular lane's shader, created on first use like `sky_draw`: a
    /// host that never places a shiny model never builds it, and the two
    /// hosts that do (VJ, sandbox) get it without adding a field to their
    /// `SceneDraws`. Boxed for the same reason `sky_draw` is — it is taken
    /// out of `self` for the duration of the draw so the loop can still
    /// borrow the model tables.
    pbr_draw: Option<Box<DrawScenePbr>>,
    custom_draws: std::collections::BTreeMap<String, Box<CustomMaterial>>,
    /// Image-based lighting for Splash materials (renderer/ibl.rs).
    ibl: ibl::IblState,
    /// The world's generic items on the model lanes (renderer/items.rs).
    items: items::ItemState,
    /// The streamed world's surface shader, created on first streamed draw.
    city_draw: Option<Box<crate::shaders::DrawSceneCity>>,
    /// The level's grass field (grass.rs), its shader and this frame's
    /// visible patches per ring.
    grass: Option<crate::grass::GrassGpu>,
    grass_draw: Option<Box<crate::shaders::DrawSceneGrass>>,
    /// The swaying-foliage lane's shader, and whether it can draw this
    /// frame (else foliage stays on the PBR lane).
    foliage_draw: Option<Box<crate::shaders::DrawSceneFoliageLit>>,
    foliage_ready: bool,
    grass_rings: [Vec<(f32, f32)>; 3],
    /// The PBR pipeline can draw this frame (`draw_models_inner`).
    pbr_ready: bool,
    /// The diffuse and PBR lanes' no-discard variants (opaque.rs).
    opaque_shaders: [opaque::OpaqueShader; 4],
    /// Whether shiny loaded models use the PBR material lane. Enabled by
    /// default so existing hosts keep their rendering unchanged; CAD-style
    /// views can temporarily request the diffuse textured lane instead.
    pbr_materials_enabled: bool,
    /// A host's screen-space AO output (ssao.rs) and its strength, bound to
    /// BOTH model lanes' `ssao_map` / `ssao_ctl` each frame. `None` (the
    /// default, and what every game host leaves it at) writes the strength
    /// OFF, so the shaders never sample the slot.
    ssao: Option<(Texture, f32)>,
    /// Follow-camera occluder fade (occluder_fade.rs); off unless a host
    /// sets a focus each frame.
    occluder: occluder_fade::OccluderFade,
    /// Per placed-model uv remap into the atlas, parallel to placed_models
    /// (zero = unmapped, the shader's disable signal). Rebuilt per delivery.
    lm_remaps: Vec<Vec4f>,
    /// The terrain's atlas region and the world xz rect it covers — ONE
    /// planar region serves every terrain tile, because tile uvs derive from
    /// world position rather than per-tile data.
    lm_ground: Option<(Vec4f, Vec4f)>,
    /// The shadow-top height plane (lightmap.rs `top_pixels`): R8, same
    /// texel layout as the atlas so the ground-region uv addresses it
    /// unchanged, plus the (base, range) that decodes a byte back to an
    /// absolute world height. Rides the same bake delivery.
    lm_top: Option<(Texture, f32, f32)>,
    /// 1x1 "no blocker measured" stand-in, bound wherever the real plane
    /// isn't so shaders sample unconditionally.
    lm_top_fallback: Option<Texture>,
    /// Host setting `HostSettings::lm_debug`: shader shows the lightmap alone.
    lm_debug: f32,
    /// The model lanes' shading space: 0 writes the game's display-referred
    /// product raw, 1 shades in linear and finishes through ACES + gamma.
    /// See [`Renderer::set_display_transform`].
    display_transform: f32,
    /// Instances the last GI snapshot left out for budget (fast_gi/scene.rs).
    gi_snapshot_omitted: usize,
    /// Content-keyed GPU texture cache shared by every static model and
    /// chunk (see `PreparedTexture::upload_cached`); pruned to what loaded
    /// models still reference whenever a model is replaced or unloaded.
    texture_cache: std::collections::HashMap<u64, Texture>,
    /// See [`Renderer::set_shadow_debug`].
    shadow_debug: bool,
    /// See [`Renderer::set_hdr_output`].
    hdr_output: bool,
    /// This frame's metered exposure (HDR output only).
    hdr_exposure: f32,
    /// See [`Renderer::set_fxaa`].
    fxaa: bool,
    /// The HDR post chain (bloom + auto-exposure), see [`Renderer::run_post`].
    post: makepad_render_graph::BloomPass,
    /// See [`Renderer::set_bloom`] / [`Renderer::set_auto_exposure`].
    bloom: f32,
    auto_exposure: bool,
    /// See [`Renderer::set_grade`].
    grade: makepad_scene::ColorGrade,
    /// The baked lightmap's KILL SWITCH (F9 in the sandbox,
    /// `MAKEPAD_LIGHTMAP=off` at launch). Off binds the 1x1 "fully sunlit,
    /// no lamps" stand-in instead of the atlas, so every static falls back
    /// to the purely analytic path — the same picture the world shows in
    /// the seconds before its first bake lands. The bake keeps running, so
    /// turning it back on is instant, and the pair is a built-in A/B for
    /// exactly the baked-vs-provisional comparison a lighting bug needs.
    lightmap_enabled: bool,
    /// Night-sky star panorama (equirectangular, decoded once via
    /// set_star_map_png), uploaded lazily on the first sky draw.
    star_map: Option<ImageBuffer>,
    star_texture: Option<Texture>,
    /// The GPU-resident light bake (gpu_lightmap.rs): fragment-shader
    /// passes, zero readback — THE bake path. OnChange re-bakes on the
    /// settle kick; Realtime (opt-in) re-bakes dirty regions per frame.
    gpu_baker: crate::gpu_lightmap::GpuLightmapBaker,
    /// Explicit static lights (set_static_lights). Empty = harvest lamp
    /// props automatically at bake time.
    lm_lights: Vec<crate::lightmap::LmLight>,
    /// This frame's dynamic light list: harvested lamps FIRST (the first
    /// `frame_baked_count` entries — baked into the static atlas, so only
    /// dynamics may sum them analytically), then transients (firework
    /// flashes, host lights). Rebuilt per frame into the same buffer.
    frame_lights: Vec<crate::lightmap::LmLight>,
    frame_baked_count: usize,
    /// Host-supplied transient lights ([`Self::add_frame_lights`]), drained
    /// into `frame_lights` each frame.
    host_lights: Vec<crate::lightmap::LmLight>,
    host_asset_lights:Vec<crate::lightmap::LmLight>,
    /// Cars whose installed exterior supplies its own punctual fixtures.
    model_headlight_owners: Vec<u64>,
    asset_light_error:Option<String>,
    /// This frame's eye, for ranking lights past the authored budget.
    light_eye: Vec3f,
    /// See [`Renderer::set_camera_relative`].
    camera_relative: bool,
    /// Cached [`Self::harvest_lamps`] output — the harvest walks strings, so
    /// it reruns only when the placed-prop list changes.
    lamp_cache: Vec<crate::lightmap::LmLight>,
    /// (`models_rev`, `lamp_daylight_key`) the cache was built for.
    lamp_cache_rev: Option<(u64, u32)>,
    /// Precomputed static-light selection grid (light_grid.rs): per cell,
    /// the ≤8 strongest lamps pre-packed in the shader's uniform layout.
    /// Rebuilt with `lamp_cache` — never on the hot path. Runtime selection
    /// is a cell lookup + copy, independent of how many lights exist.
    light_grid: LightGrid,
    clustered: crate::clustered::ClusteredLights,
    gi: crate::fast_gi::FastGi,
    clustered_enabled: bool,
    clustered_frames: u64,
    /// Scratch for the transient merge / world selection; reused per frame.
    light_rank: Vec<(f32, usize)>,
    light_sel: Vec<usize>,
    light_block_scratch: [f32; LIGHT_BLOCK_FLOATS],
    /// Positional hysteresis for per-instance cell lookups: which grid cell
    /// each dynamic object (characters by key, model dynamics by placed
    /// index, actor attachments by their own slot namespace) last homed to.
    /// An object dithering on a cell line keeps its old block until it moves
    /// a real margin into the neighbour — the no-flicker dead-band. Cleared
    /// when the grid rebuilds.
    light_cell_memory: std::collections::HashMap<u64, (i32, i32)>,
    /// Per-character / per-dynamic-model ground heights for this frame
    /// (receiver-sampled once on the CPU): the shader projects the baked
    /// sun-shadow sample along the sun ray from the vertex down to this
    /// plane. Parallel to the batch items / placed models / attachments.
    char_ground: Vec<f32>,
    model_ground: Vec<f32>,
    world_attachment_ground: Vec<f32>,
    /// The flares' shared one-quad billboard geometry, built once.
    flare_geometry: Option<Geometry>,
    /// This frame's SDF-quad records — the ENTIRE per-frame cost of a
    /// dynamic caster's shadow. Reused; sorted by atlas at draw so one
    /// rig's crowd shares a draw item.
    sdf_instances: Vec<SdfInstance>,
    /// Baked silhouette-SDF atlases per character rig, uploaded once at
    /// delivery. `None` payload = the rig baked to nothing; its characters
    /// keep the blob tier.
    sdf_atlas_tex: Vec<(u64, Option<(Texture, SdfMeta)>)>,
    /// Baked yaw-only silhouette-SDF atlases per dynamic MODEL (the
    /// driveable cars), keyed by asset id — per model, never per instance.
    /// `None` payload = no loadable sidecar for this model under the
    /// current sun; its instances keep the blob tier.
    model_sdf_tex: std::collections::HashMap<String, Option<(Texture, SdfMeta)>>,
    /// `.shadowsdf` bytes handed in WITH a model (streamed from an asset
    /// store beside its GLB); consulted before the `models_root` sidecar.
    model_sdf_bytes: std::collections::HashMap<String, Vec<u8>>,
    /// The sun elevation (`SunLight::shadow_len_per_unit`) the loaded SDF
    /// atlases are valid for. An explicit sun change drops the caches and
    /// re-tries the sidecars — there is NO runtime silhouette baking:
    /// OnChange runs a pinned sun, and a caster whose sidecar disagrees
    /// with it falls to the blob tier rather than to a wrong-length shadow.
    sdf_baked_sun_len: f32,
    /// The world's sun hour last frame, and whether it has MOVED since the
    /// realm began: a running clock takes the analytic sky even under an
    /// authored palette (`analytic_sky_frame`); a fixed hour keeps it.
    sky_hour: Option<f32>,
    sky_clock: bool,
    /// Last (render_rev, models_rev, daylight quantum) a GPU lightmap job
    /// was scheduled for: in Realtime mode a sun-only change must NOT
    /// re-kick the whole job — the baker follows the sun per frame on its
    /// own — UNLESS it moved the lamps' daylight-headroom scale, which is
    /// baked into the atlas RGB and cannot follow anything per frame.
    lm_kick_key: Option<(u64, u64, u32)>,
    lm_kick_sun: Option<Vec3f>,
    shadow_geometry: Option<Geometry>,
    last_dynamic_shadow_tris: usize,
    shadow_points: Vec<Vec3f>,
    /// Debounce for the world-settle work (receiver-box refresh + the
    /// lightmap kick): an edit burst pays one refresh at rest.
    shadow_gate: ShadowRebuildGate,
    /// Bumped when the placed-prop list changes, so it can join the settle
    /// cache key.
    models_rev: u64,
    /// CPU-baked occlusion (bake.rs), folded into instance colours. Renderer
    /// state by construction: the sim has no field for it, so a device may
    /// bake at a different quality than its peers without diverging.
    bake: LightBake,
    /// Adaptive quality (thermometer.rs). Renderer state for the same reason
    /// the bake is: a Quest can run three levels leaner than the PC beside it
    /// and the two stay in lockstep, because nothing it dials is simulated.
    /// Dormant until the host calls [`Renderer::report_frame_ms`].
    thermometer: Thermometer,
    /// Orbit-preview look-at depth for Realtime CSM. `Some` fits cascade 0
    /// to that plane so shadow texels stay roughly one screen pixel under
    /// zoom; `None` keeps the village-scale ladder (walk / game).
    csm_focus: Option<f32>,
    /// Optional host-owned scene bound for CSM fitting. Imported editor
    /// models may deliberately submit as per-frame casters and therefore
    /// own no lightmap layout from which the baker could infer this bound.
    csm_scene_bounds: Option<(Vec3f, Vec3f)>,
}

mod draw_items;
mod prepared;
mod model_instance;
mod frame_util;
mod realm;
mod world_geometry;
mod model_load;
mod lights;
mod settings;
mod bake_passes;
mod model_query;
mod draw_models;
mod occluder_fade;
mod opaque;
mod skinned;
mod frame;
mod frame_layers;
mod gi;
mod stream_draw;
mod grass_draw;
mod vfx_draw;
mod ibl;
mod items;
pub use items::{splash_material_name, GeometryData, TransformTint, LAYOUT_TRANSFORM_TINT};

pub use draw_items::*;
pub use prepared::*;
pub use model_instance::*;
pub use frame_util::*;
pub use stream_draw::StreamStats;

impl Default for Renderer {
    fn default() -> Self {
        Self {
            shape_geometries: Default::default(),
            shadow_shape_geometries: Default::default(),
            static_chunks: Vec::new(),
            chunk_visible: Vec::new(),
            slab_key: None,
            slab_instance_count: 0,
            terrain_tiles: Vec::new(),
            terrain_revision: 0,
            voxel_tiles: Vec::new(),
            water_tiles: Vec::new(),
            water_rev: None,
            skin_rig_geometries: Vec::new(),
            skin_material_draws: Default::default(),
            skin_lods:Default::default(),
            skin_morphs: Default::default(),
            skin_prepared_sdf: Default::default(),
            skin_palette_tex: None,
            skin_palette_texels: 0,
            skin_joint_bases: Vec::new(),
            static_models: Vec::new(),
            preview_originals: std::collections::HashMap::new(),
            preview_new: std::collections::HashSet::new(),
            sky_draw: None,
            decal_draw: None,
            sky_draw_wait_logged: false,
            sky_time: 0.0,
            model_casts_shadow: std::collections::BTreeMap::new(),
            model_lod_chains: std::collections::BTreeMap::new(),
            model_lod_chains_dirty: false,
            model_anim_state: ModelStates::default(),
            placed_models: Vec::new(),
            csm_static_casters: Vec::new(),
            csm_static_blocks: Vec::new(),
            stream: None,
            stream_workers: None,
            stream_casters: Vec::new(),
            world_attachments: Vec::new(),
            view_models: Vec::new(),
            view_model_sun: 1.0,
            placed_scene_signature: None,
            placed_prefix: None,
            stage: Stage::default(),
            shadow_budget: DEFAULT_SHADOW_BUDGET,
            vfx: Default::default(),
            ao_textures: Vec::new(),
            model_pack: Vec::new(),
            model_orders: Default::default(),
            placed_blocks: Default::default(),
            firework_instances: Vec::new(),
            smoke_volumes: Vec::new(),
            smoke_draw: None,
            spark_geometry: None,
            // 60Hz until the host says otherwise, and dormant regardless
            // until someone reports a frame time.
            thermometer: Thermometer::new(60.0),
            shadow_mesh: ShadowMeshBuilder::default(),
            receiver_boxes: Vec::new(),
            occluder_boxes: Vec::new(),
            lightmap: None,
            lm_fallback: None,
            detail_fallback: None,
            orm_fallback: None,
            pbr_draw: None,
            custom_draws: Default::default(),
            ibl: Default::default(),
            items: Default::default(),
            city_draw: None,
            grass: None,
            grass_draw: None,
            foliage_draw: None,
            foliage_ready: false,
            grass_rings: Default::default(),
            pbr_ready: false,
            opaque_shaders: Default::default(),
            pbr_materials_enabled: true,
            ssao: None,
            occluder: Default::default(),
            lm_remaps: Vec::new(),
            lm_ground: None,
            lm_top: None,
            lm_top_fallback: None,
            lightmap_enabled: !matches!(
                std::env::var("MAKEPAD_LIGHTMAP").as_deref(),
                Ok("off") | Ok("0") | Ok("false")
            ),
            lm_debug: 0.0,
            display_transform: 0.0,
            gi_snapshot_omitted: 0,
            shadow_debug: false,
            texture_cache: Default::default(),
            hdr_output: false,
            hdr_exposure: 1.0,
            fxaa: true,
            post: Default::default(),
            bloom: DEFAULT_BLOOM,
            auto_exposure: true,
            grade: Default::default(),
            star_map: None,
            star_texture: None,
            gpu_baker: {
                // MAKEPAD_GPU_LM_MODE=realtime starts the baker in Realtime
                // (unattended runs can't press the F8 debug toggle).
                let mut b = crate::gpu_lightmap::GpuLightmapBaker::default();
                if std::env::var("MAKEPAD_GPU_LM_MODE")
                    .map(|v| v == "realtime")
                    .unwrap_or(false)
                {
                    b.set_mode(crate::gpu_lightmap::GpuLightmapMode::Realtime);
                }
                b
            },
            lm_lights: Vec::new(),
            frame_lights: Vec::new(),
            frame_baked_count: 0,
            host_lights: Vec::new(),
            host_asset_lights:Vec::new(),asset_light_error:None,light_eye:Vec3f::default(),camera_relative:false,
            model_headlight_owners: Vec::new(),
            lamp_cache: Vec::new(),
            lamp_cache_rev: None,
            light_grid: LightGrid::default(),
            clustered: crate::clustered::ClusteredLights::default(),
            gi: crate::fast_gi::FastGi::default(),
            clustered_enabled: !matches!(std::env::var("MAKEPAD_CLUSTERED").as_deref(), Ok("0" | "off")),
            clustered_frames: 0,
            light_rank: Vec::new(),
            light_sel: Vec::new(),
            light_block_scratch: [0.0; LIGHT_BLOCK_FLOATS],
            light_cell_memory: std::collections::HashMap::new(),
            char_ground: Vec::new(),
            model_ground: Vec::new(),
            world_attachment_ground: Vec::new(),
            flare_geometry: None,
            sdf_instances: Vec::new(),
            sdf_atlas_tex: Vec::new(),
            model_sdf_tex: std::collections::HashMap::new(),
            model_sdf_bytes: std::collections::HashMap::new(),
            sdf_baked_sun_len: 0.0,
            sky_hour: None,
            sky_clock: false,
            lm_kick_key: None,
            lm_kick_sun: None,
            shadow_geometry: None,
            last_dynamic_shadow_tris: 0,
            shadow_points: Vec::new(),
            shadow_gate: ShadowRebuildGate::default(),
            models_rev: 0,
            bake: LightBake::default(),
            csm_focus: None,
            csm_scene_bounds: None,
        }
    }
}

#[cfg(test)]
mod shell_bucket_tests;
#[cfg(test)]
mod realm_lifecycle_tests;
#[cfg(test)]
mod sun_tests;
#[cfg(test)]
mod cull_tests;
#[cfg(test)]
mod part_attachment_tests;
#[cfg(test)]
mod chunk_tests;
#[cfg(test)]
mod light_tests;
#[cfg(test)]
mod water_sheet_tests;
#[cfg(test)]
mod shadow_sdf_sidecar_tests;
/// The rigid-part state machine: a door's whole behaviour, tested without a
/// device. The GPU side is one extra draw with the matrix these produce.
#[cfg(test)]
mod anim_part_tests;
/// Which statics may cast into the baked sun shadows. A prop casts onto the
/// world; a whole imported level IS the world, and casting it shadows its own
/// rooms.
#[cfg(test)]
mod caster_only_tests;
/// The map-sky lane's device-free half: the clock the scrolling layers ride
/// and the queries a host asks before it has loaded anything.
#[cfg(test)]
mod sky_lane_tests;
#[cfg(test)]
mod prepared_static_preview_tests;
