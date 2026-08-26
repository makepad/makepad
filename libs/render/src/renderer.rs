//! The world scene pass, moved verbatim from gamemaker's game_view.rs:
//! sky dome → terrain mesh → opaque per-shape batches → alpha per-shape
//! batches. Statics come from packed instance slabs cached against
//! `world.render_rev`; dynamics re-pack every frame.

use makepad_draw::*;
use makepad_game_sim::{
    entity_index_sorted, BodyKind, ChunkKey, Entity, GameWorld, Shape, Terrain, TerrainMaterials,
    VoxelField, WaterState, WaterVolume, MAX_WAVES,
};

use crate::bake::{BakeSettings, BakeStats, LightBake};
use crate::geometry::shape_geometry_data;
use crate::light_grid::{
    merge_transients_into_block, LightBlock, LightGrid, LIGHT_BLOCK_FLOATS, LIGHT_GRID_CELL,
};
use crate::shaders::{
    DrawSceneAlpha, DrawSceneCube, DrawSceneFlare, DrawSceneScreen, DrawSceneShadow,
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

/// One world-space upright quad. `pos.xyz` is the centre, `pos.w` is yaw
/// (camera yaw faces the orbit/walk camera). `size.xy` is width/height in
/// world units; `size.zw` is the TEXTURE's pixel size, which the shader needs
/// to sample on texel centres (crisp magnification, filtered minification).
/// Leave `zw` zero and the quad falls back to a plain bilinear fetch.
#[derive(Clone)]
pub struct ScreenInstance {
    pub texture: Texture,
    pub pos: Vec4f,
    pub size: Vec4f,
    /// Sub-rectangle of the texture this quad shows, `(u0, v0, u1, v1)`.
    /// `u0 > u1` draws it X-mirrored. A whole-texture quad passes
    /// `(0, 0, 1, 1)`.
    pub uv: Vec4f,
}

/// Per-frame render counters, handed back for the host's profiler.
#[derive(Default, Clone, Copy)]
pub struct RenderStats {
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
    /// Dynamic shadows drawn through the GPU SDF-silhouette quad
    /// (characters + driven cars).
    pub sdf_shadow_instances: u64,
}

/// GPU-side caches for one view family: unit shape geometries, the packed
/// static instance slabs, and the terrain mesh. Owns no draw structs — see
/// [`SceneDraws`].
pub struct Renderer {
    // PERF: unit shape geometries, built once (index = Shape::index()).
    shape_geometries: [Option<Geometry>; 5],
    // PERF: packed static instance data per shape (opaque / alpha passes),
    // valid while slab_rev == world.render_rev.
    static_chunks: Vec<SlabChunk>,
    /// Per-frame visibility scratch, index-aligned with `static_chunks`;
    /// reused so a steady scene does not reallocate.
    chunk_visible: Vec<bool>,
    /// `(world.render_rev, bake.generation())` — the slabs carry baked light
    /// in their colours, so a rebake invalidates them exactly like a world
    /// edit does.
    slab_key: Option<(u64, u64)>,
    slab_instance_count: u64,
    /// GPU mesh for the smooth terrain, rebuilt when the revision changes.
    terrain_tiles: Vec<TerrainTile>,
    terrain_revision: u64,
    /// GPU meshes for voxel terrain chunks (mix.md T2/T3), keyed by chunk
    /// and mesh revision — re-uploaded per chunk when a dig remeshes it.
    /// Drawn through the SAME terrain shader/lightmap path as the tiles.
    voxel_tiles: Vec<VoxelTile>,
    /// One flat grid per `game.water` volume (W1), displaced in the vertex
    /// shader by the sim's wave sum. Rebuilt when `WaterState::rev` moves.
    water_tiles: Vec<WaterTile>,
    water_rev: Option<u64>,
    /// REST meshes for GPU-skinned rigs — geometry plus the rig's rest-pose
    /// AO chart atlas — keyed by rig id and uploaded ONCE
    /// ([`Self::upload_skin_rig`]). Skinning happens in the vertex shader
    /// against the frame's joint-palette texture, so a character's per-frame
    /// upload is its palette, not its vertices (see skin.rs).
    skin_rig_geometries: Vec<(u64, Geometry, Texture)>,
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
    /// The map-sky shader, built lazily from its script type default (the
    /// baker owns its passes the same way). Owned rather than lent through
    /// [`SceneDraws`] because a map's sky has nothing for a host to theme,
    /// and every host would have had to adopt a new field to get one.
    sky_draw: Option<Box<DrawSceneSkyMap>>,
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
    /// Actor-attached presentation meshes in world space. They use the
    /// ordinary world material/depth path, but are deliberately outside the
    /// placed scene's identity, bakes, caster lists, lamp harvest and blob
    /// shadows. This is the third-person counterpart to `view_models`.
    world_attachments: Vec<ModelInstance>,
    /// Device/view-local presentation meshes, submitted separately from the
    /// world so an FPS held model can never enter scene identity, lightmap or
    /// shadow-caster ownership. Drawn in a late, depth-overlay layer.
    view_models: Vec<ModelInstance>,
    /// Deterministic identity of the placed scene relevant to static
    /// lighting. Unlike the old length-only check this includes every
    /// static model, transform, depth order, and list slot. Dynamic
    /// transforms are deliberately excluded: cars move every frame without
    /// changing the baked scene.
    placed_scene_signature: Option<u64>,
    /// How this device projects the world (flat / VR 1:1 / MR diorama).
    /// Applied as the scene draw list's view transform, so it costs one
    /// uniform and never invalidates the static slabs. See stage.rs.
    stage: Stage,
    /// How many casters get a projected silhouette before the rest fall
    /// back to blobs. A per-device dial: a Quest can afford fewer than a PC
    /// even though both cost one instance, because the projection math runs
    /// per caster per frame on the CPU.
    shadow_budget: usize,
    /// Device-local particles for this frame (particles.rs). Set by the
    /// host before drawing; never simulation state, never replicated.
    particle_instances: Vec<ParticleInstance>,
    /// Those atlases on the GPU, keyed the same way.
    ao_textures: Vec<(String, Texture)>,
    /// Which pack each loaded model belongs to, for binding its atlas.
    model_pack: Vec<(String, String)>,
    /// One shell per entry — this is the whole per-frame firework upload.
    firework_instances: Vec<crate::firework::FireworkInstance>,
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
    /// Whether shiny loaded models use the PBR material lane. Enabled by
    /// default so existing hosts keep their rendering unchanged; CAD-style
    /// views can temporarily request the diffuse textured lane instead.
    pbr_materials_enabled: bool,
    /// A host's screen-space AO output (ssao.rs) and its strength, bound to
    /// BOTH model lanes' `ssao_map` / `ssao_ctl` each frame. `None` (the
    /// default, and what every game host leaves it at) writes the strength
    /// OFF, so the shaders never sample the slot.
    ssao: Option<(Texture, f32)>,
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
    /// SANDBOX_LM_DEBUG=1: shader shows the lightmap alone.
    lm_debug: f32,
    /// The model lanes' shading space: 0 writes the game's display-referred
    /// product raw, 1 shades in linear and finishes through ACES + gamma.
    /// See [`Renderer::set_display_transform`].
    display_transform: f32,
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
    /// Last (render_rev, models_rev, daylight quantum) a GPU lightmap job
    /// was scheduled for: in Realtime mode a sun-only change must NOT
    /// re-kick the whole job — the baker follows the sun per frame on its
    /// own — UNLESS it moved the lamps' daylight-headroom scale, which is
    /// baked into the atlas RGB and cannot follow anything per frame.
    lm_kick_key: Option<(u64, u64, u32)>,
    shadow_geometry: Option<Geometry>,
    last_dynamic_shadow_tris: usize,
    shadow_points: Vec<Vec3f>,
    /// Debounce for the world-settle work (receiver-box refresh + the
    /// lightmap kick): an edit burst pays one refresh at rest.
    shadow_gate: ShadowRebuildGate,
    /// Bumped when the placed-prop list changes, so it can join the settle
    /// cache key.
    models_rev: u64,
    /// Unit box in the packed-mesh layout, position lanes only: Realtime's
    /// stand-in caster geometry for primitive ENTITY bodies (crates), which
    /// have no mesh of their own to rasterize into the bake's depth passes.
    lm_box_geometry: Option<Geometry>,
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

/// One GPU-skinned character instance for [`Renderer::draw_scene_full`].
/// The per-frame payload is the joint `palette`; the rig's rest mesh is
/// resident on the GPU ([`Renderer::upload_skin_rig`]).
pub struct SkinnedDraw {
    /// Stable per-character id — the rotational-shadow keyframe cache key.
    pub key: u64,
    /// Which resident rest mesh this character wears. Characters of one rig
    /// share the geometry and batch into one draw item.
    pub rig: u64,
    pub transform: Mat4f,
    /// Per-character wash over the model's own colours, so one rig can furnish
    /// a village without every villager being the same figure.
    pub tint: Vec4f,
    /// Index into [`SkinnedBatch::textures`]. Characters from different packs
    /// carry different atlases, and binding one character's atlas to another
    /// does not fail — it renders, wrongly, looking like a shading bug.
    pub texture: usize,
    /// This frame's joint palette ([`crate::skin::SkinnedModel::palette`]).
    pub palette: Vec<Mat4f>,
    /// Posed model-space bounds for frustum culling
    /// ([`crate::skin::SkinnedModel::posed_bounds`]) — no posed vertices
    /// exist on the CPU to measure any more.
    pub bounds: Option<(Vec3f, Vec3f)>,
    /// Walk-cycle phase, 0..1 through the clip — picks the pose-phase atlas
    /// row pair so the shadow breathes with the stride.
    pub gait_phase: f32,
    /// 0 = idle stance, 1 = full walk; mixes the idle row toward the
    /// phase rows exactly the way the pose blend does.
    pub gait_blend: f32,
    /// Where this rig's offline `.shadowsdf` sidecar lives: the rig's
    /// source GLB path plus its [`crate::skin::SkinnedModel::rest_hash`]
    /// (the `.skinao` keying scheme). The ONLY source of a rig's SDF
    /// shadow atlas — tools/ao_bake writes `<glb>.shadowsdf`, the runtime
    /// loads it or the rig's characters blob.
    pub sdf_sidecar: Option<(String, u64)>,
}

impl SkinnedDraw {
    /// Untinted: the model's own colours, unchanged.
    pub fn new(key: u64, rig: u64, transform: Mat4f) -> Self {
        Self {
            key,
            rig,
            transform,
            tint: vec4(1.0, 1.0, 1.0, 1.0),
            texture: 0,
            palette: Vec::new(),
            bounds: None,
            gait_phase: 0.0,
            gait_blend: 0.0,
            sdf_sidecar: None,
        }
    }

    pub fn with_tint(mut self, tint: Vec4f) -> Self {
        self.tint = tint;
        self
    }

    /// Which atlas this character samples.
    pub fn with_texture(mut self, texture: usize) -> Self {
        self.texture = texture;
        self
    }

    pub fn with_palette(mut self, palette: Vec<Mat4f>) -> Self {
        self.palette = palette;
        self
    }

    pub fn with_bounds(mut self, bounds: Option<(Vec3f, Vec3f)>) -> Self {
        self.bounds = bounds;
        self
    }

    /// Where the rig's offline `.shadowsdf` sidecar lives (see
    /// [`Self::sdf_sidecar`]): the source GLB path — the sidecar is
    /// `<glb>.shadowsdf` beside it — and the rig's rest hash to key it.
    pub fn with_sdf_sidecar(mut self, glb_path: &str, source_hash: u64) -> Self {
        self.sdf_sidecar = Some((glb_path.to_string(), source_hash));
        self
    }

    /// This frame's walk-cycle phase (0..1) and idle-to-walk blend, driving
    /// the pose-phase shadow atlas rows.
    pub fn with_gait(mut self, phase: f32, blend: f32) -> Self {
        self.gait_phase = phase;
        self.gait_blend = blend;
        self
    }
}

/// The skinned characters for one frame, drawn between the opaque and alpha
/// passes (so blob shadows and sensor ghosts blend over them correctly).
pub struct SkinnedBatch<'a> {
    pub skinned: &'a mut DrawSceneSkinnedGpu,
    /// One atlas per distinct character pack; items index into it. A village
    /// of Kenney civilians shares a single entry (one `colormap.png` per
    /// pack), so the common case stays one texture and one bind.
    pub textures: Vec<&'a Texture>,
    pub items: Vec<SkinnedDraw>,
}

/// One caster's SDF-silhouette shadow record, mirroring
/// [`crate::shaders::DrawSceneShadowSdf`]'s instance fields. `atlas` picks
/// the texture at draw: a rig id for characters, a model index for cars —
/// both mapped through [`SdfAtlasKey`].
struct SdfInstance {
    atlas: SdfAtlasKey,
    a: Vec4f,
    b: Vec4f,
    c: Vec4f,
    d: Vec4f,
    e: Vec4f,
}

/// Which uploaded atlas an [`SdfInstance`] draws with. Ord so the draw loop
/// can sort the frame's instances and share one draw item per atlas.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SdfAtlasKey {
    Rig(u64),
    Model(String),
}

/// What the runtime needs to address a rig's uploaded SDF atlas: the shared
/// window every cell maps (shadow_sdf.rs `rect`), the pose rows, and the
/// encode band for converting edge softness into encoded-d units.
#[derive(Clone, Copy)]
struct SdfMeta {
    rect: Vec4f,
    rows: usize,
    band_world: f32,
    len_per_unit: f32,
}

/// SDF shadow edge softness, sprite units: the base half-width at the feet
/// and the extra per unit of source height (contact hardening — the tip of
/// a shadow is cast by the head and blurs wider than the feet's contact).
const SDF_SOFT_BASE: f32 = 0.05;
const SDF_SOFT_HARDEN: f32 = 0.10;

/// Car-tilt gate for the SDF quad: the atlas bakes the car FLAT, and the
/// quad follows the local ground plane, which reads fine up to moderate
/// tilt. Beyond this cosine of body-up vs world-up (~37 degrees) the flat
/// sprite lies — a rolled or launched car falls back to the plain blob.
const SDF_CAR_TILT_MIN_UP: f32 = 0.8;

/// Airborne gate: above this many units of clear air under the wheels the
/// flat silhouette stops being the car's shadow (mid-jump, off a ramp
/// lip) — blob until it lands. Characters keep their sprite when jumping
/// (limbs read even in the air); a car's roof-line does not.
const SDF_CAR_AIR_MAX: f32 = 1.5;

/// May a car draw its flat SDF sprite, or must it fall back to the blob?
/// `up_y` = cosine of body-up against world-up (the transform's normalised
/// y basis), `air` = clear units between the wheels and the receiver.
fn car_sprite_allowed(up_y: f32, air: f32) -> bool {
    up_y >= SDF_CAR_TILT_MIN_UP && air <= SDF_CAR_AIR_MAX
}

/// A character shadow's resolved landing (see
/// [`Renderer::character_shadow_anchor`]).
struct ShadowAnchor {
    /// FOOT-END landing point, y at the receiver surface: the caster's
    /// ground contact, moved only by the HEIGHT-driven part of the offset
    /// policy (a jump slides the whole shadow off the feet — that is the
    /// point of a jump shadow). A lamp lean NEVER moves this end: a shadow
    /// roots at the feet and its body sweeps away from the light, so a
    /// grounded caster's root is always their own contact point. The
    /// silhouette quad pins its window origin (the bake's ground anchor)
    /// here.
    root: Vec3f,
    /// The owning light's full lean, world xz: the direction the
    /// silhouette POINTS (away from the light), magnitude the light's
    /// mid-body projection. Supplies the quad's long-axis direction and
    /// nothing else — leaning translates the BODY of the shadow, not its
    /// contact.
    lean: Vec2f,
    /// Normal + slope bias to clear the receiver.
    lift: f32,
    /// Final shadow alpha: drop fade × height fade × lamp boost.
    alpha: f32,
    /// Sprite scale multiplier (swells with height).
    size_mul: f32,
    /// 0..1 — how much the dominant lamp owns this shadow.
    lamp_w: f32,
}

/// The whole character-shadow anchor policy as a free function, so the GPU
/// fan, the CPU fallback AND the unit tests all read the same numbers —
/// see [`Renderer::character_shadow_anchor`] for the policy prose.
fn character_shadow_anchor(
    feet: Vec3f,
    receiver: &Receiver,
    sun: &SunLight,
    lights: &[crate::lightmap::LmLight],
) -> Option<ShadowAnchor> {
    let (g0, _) = receiver.sample(feet.x, feet.z);
    let h = (feet.y - g0).max(0.0);
    let sy = sun.dir.y.max(0.2);
    // Strongest lamp at the feet — same attenuation/cone math as the
    // shader term, so what lights the character is what owns its shadow.
    let sun_i = sun.color.x.max(sun.color.y).max(sun.color.z)
        * (sun.dir.y * 4.0).clamp(0.0, 1.0);
    let mut best: Option<(f32, &crate::lightmap::LmLight)> = None;
    for l in lights {
        if l.radius <= 0.0 {
            continue;
        }
        let (dx, dy, dz) = (feet.x - l.pos.x, feet.y - l.pos.y, feet.z - l.pos.z);
        let d2 = dx * dx + dy * dy + dz * dz;
        if d2 >= l.radius * l.radius {
            continue;
        }
        let d = d2.sqrt().max(1.0e-4);
        let att = 1.0 - d / l.radius;
        let cone = (((l.pos.y - feet.y) / d + 0.35) / 1.35).clamp(0.0, 1.0);
        let s = light_intensity(l) * att * att * (cone * cone * l.spot + (1.0 - l.spot));
        if best.as_ref().is_none_or(|(bs, _)| s > *bs) {
            best = Some((s, l));
        }
    }
    let mut lamp_w = 0.0;
    if let Some((lamp_i, _)) = best {
        let ratio = (lamp_i * 2.0) / (lamp_i * 2.0 + sun_i + 1.0e-4);
        lamp_w = ((ratio - 0.35) / 0.4).clamp(0.0, 1.0);
    }
    // The lean policy, as a function of caster height `hh`: sun projection
    // blended toward the dominant lamp's true mid-body projection.
    let lean_at = |hh: f32| -> Vec2f {
        let mut off = vec2f(-sun.dir.x / sy * hh, -sun.dir.z / sy * hh);
        if lamp_w > 0.0 {
            let (_, l) = best.expect("lamp_w > 0 implies a best lamp");
            let (hx, hz) = (feet.x - l.pos.x, feet.z - l.pos.z);
            let rho = (hx * hx + hz * hz).sqrt();
            // The LEAN saturates faster than the dominance itself: sqrt
            // eases the blend toward the lamp's TRUE projection while a
            // meaningful share of the light is the lamp's. Linear was
            // measured too timid in the day village — dominance ~0.7 beside
            // a lamp halved a ~0.7-unit true offset into a sub-half-unit
            // nudge that read as nothing. Darkening keeps the LINEAR weight:
            // alpha is about energy, the lean is about where the silhouette
            // POINTS, and only the second one was invisible.
            let off_w = lamp_w.sqrt();
            if rho > 0.15 {
                // Shadow of a mid-body occluder thrown by the bulb.
                const MID: f32 = 0.9;
                let denom = (l.pos.y - g0 - MID).max(0.5);
                let m = (rho * (MID + hh) / denom).min(2.5);
                let (lx, lz) = (hx / rho * m, hz / rho * m);
                off = vec2f(off.x + (lx - off.x) * off_w, off.y + (lz - off.y) * off_w);
            } else {
                // Dead under the bulb: no direction — it only darkens.
                off = vec2f(off.x * (1.0 - off_w), off.y * (1.0 - off_w));
            }
        }
        off
    };
    let lean = lean_at(h);
    // The ROOT moves only with the height-driven part of the lean: the
    // full lean minus what the policy would say for the same caster
    // standing on the ground. Grounded (h = 0) the two cancel exactly and
    // the root IS the contact point — a lamp lean redirects the body of
    // the shadow but never detaches it from the boots (the floating-
    // sideways-silhouette report this parameterisation replaces). Airborne
    // the difference is the true projection growth with height, sun and
    // lamp alike, so a jump still slides the whole shadow.
    let ground_lean = lean_at(0.0);
    let root_x = feet.x + lean.x - ground_lean.x;
    let root_z = feet.z + lean.y - ground_lean.y;
    let probe = vec3f(root_x, feet.y, root_z);
    let (ground, normal, alpha, _) = shadow_drop_params(probe, receiver, sun)?;
    let slope = (1.0 - normal.y.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Some(ShadowAnchor {
        root: vec3f(root_x, ground, root_z),
        lean,
        lift: SHADOW_NORMAL_BIAS + SHADOW_SLOPE_BIAS * slope,
        alpha: (alpha * (1.0 + 0.7 * lamp_w)).min(0.6)
            * (1.0 - h / 4.0).clamp(0.0, 1.0),
        // Height swells the sprite (occluder distance); a dominant lamp
        // COMPRESSES it toward the root instead. The silhouette is baked
        // against the SUN, so under a bulb it still carries the sun's
        // elongation — at dusk a whole body-length of it — and a near-
        // vertical light must pin the shadow small and dark at the feet.
        // The quad scales about its window origin — the pinned foot end —
        // (the SDF quad's sdf_b.z is this same number), so scaling shortens
        // the FAR end toward the feet, which is exactly "compressed toward
        // the feet". 0.4x at full dominance.
        size_mul: (1.0 + 0.25 * h) * (1.0 - 0.6 * lamp_w),
        lamp_w,
    })
}

/// The plane an SDF shadow quad should lie on: the anchor's own landing
/// height, RAISED to the highest receiver surface under the silhouette's
/// down-sun run. The quad is one flat plane; when the silhouette crosses
/// onto a HIGHER receiver top — a road slab 8 cm proud of the grass its
/// caster stands on — a plane anchored at the caster's ground sits UNDER
/// that top and the whole silhouette depth-buries: the shadow vanishes
/// entirely at exactly the headings that lay it across the slab (and
/// flickers with camera angle at the slab's silhouette edge). A shadow
/// floating a few centimetres above the lower surface is invisible; a
/// buried one is a missing shadow, so raising is always the right trade.
/// `(gx, gz)` is the unit axis TOWARD the light, `len` the silhouette's
/// world-unit reach away from it (the atlas window's down-sun extent x
/// the sprite scale).
fn sdf_quad_ground(anchor: Vec3f, receiver: &Receiver, gx: f32, gz: f32, len: f32) -> f32 {
    let mut y = anchor.y;
    for f in [0.5f32, 1.0] {
        let (sy, _) = receiver.sample(anchor.x - gx * len * f, anchor.z - gz * len * f);
        if sy > y {
            y = sy;
        }
    }
    y
}

/// One draw layer's metallic-roughness material, resident.
///
/// Neutral (`metallic 0, roughness 1, orm_on false`) is exactly the shading
/// the diffuse lane gives, which is what a layer gets when its own material
/// carries no shininess — including inside a model that IS on the PBR lane
/// because some OTHER layer is shiny. That matters: the glTF default is
/// `metallic 1`, and a layer taken literally at that would lose its whole
/// diffuse lobe and render black.
#[derive(Clone)]
struct LayerMaterial {
    metallic: f32,
    roughness: f32,
    /// The metallicRoughness map, or a white 1x1 when the material is
    /// factors-only (`orm_on` false folds it out of both products anyway).
    orm: Texture,
    orm_on: bool,
}

/// Which shader a model pass drives.
///
/// The two lanes share every line of [`Renderer::draw_models_inner`]:
/// [`DrawScenePbr`] derefs [`DrawSceneSkinned`], so all the per-instance
/// state — transform, lightmap window, dynamic-light gate, AO binding — is
/// written through one type, and the lanes differ only in the material the
/// PBR shader additionally reads. A model picks its lane at LOAD time
/// (`LoadedModel::wants_pbr`), so a frame is at most two passes over the
/// instance list regardless of how the scene is mixed.
enum ModelDraw<'a> {
    Diffuse(&'a mut DrawSceneSkinned),
    Pbr(&'a mut DrawScenePbr),
}

impl ModelDraw<'_> {
    fn base(&mut self) -> &mut DrawSceneSkinned {
        match self {
            ModelDraw::Diffuse(d) => d,
            ModelDraw::Pbr(d) => &mut d.skinned,
        }
    }

    fn is_pbr(&self) -> bool {
        matches!(self, ModelDraw::Pbr(_))
    }

    /// Bind one layer's metallic-roughness. A no-op on the diffuse lane,
    /// which has no such lanes to bind — that is the whole reason the two
    /// shaders are siblings.
    fn set_material(&mut self, m: &LayerMaterial) {
        if let ModelDraw::Pbr(d) = self {
            d.metallic = m.metallic;
            d.roughness = m.roughness;
            d.orm_on = if m.orm_on { 1.0 } else { 0.0 };
            // Texture slots are DECLARATION order. DrawSceneSkinned's
            // inherited set ends at elem_map (6) and ssao_map (7); orm_map
            // is DrawScenePbr's own declaration after the spread, slot 8.
            // (This was still binding 6 from before elem_map existed, which
            // parked the ORM texture over the element lookup and left
            // orm_map on the fallback.)
            d.skinned.draw_vars.set_texture(8, &m.orm);
        }
    }
}

/// A stock prop resident on the GPU: geometry uploaded once, plus the pack
/// atlas it samples. Thousands of models share a few dozen atlases, which is
/// what keeps a whole pack's worth of props cheap to draw.
struct LoadedModel {
    geometry: Geometry,
    texture: Texture,
    detail: Texture,
    detail_scale: [f32; 2],
    /// Extra (geometry, albedo, detail, scale, material) draws for world GLBs
    /// that embed one PNG per tile. The first layer is `geometry`/`texture`.
    extra_draws: Vec<(Geometry, Texture, Texture, [f32; 2], LayerMaterial)>,
    /// Layer 0's material (the merged stream's, for a single-layer model).
    material: LayerMaterial,
    /// Draw this model through [`DrawScenePbr`] instead of
    /// [`DrawSceneSkinned`]. Decided ONCE here, at load, from the glTF
    /// material: true when the model is not prelit and some layer's material
    /// carries real shininess ([`crate::model::PbrMaterial::is_shiny`]).
    ///
    /// Per model rather than per layer because the shader is a property of
    /// the draw item, and splitting one prop across two shaders would double
    /// its draw calls to give a matte layer a lobe it cannot show anyway.
    wants_pbr: bool,
    /// COLOR_0 is a baked lightmap — skip the analytic sun multiply.
    prelit: bool,
    triangles: usize,
    /// Model-space bounds, kept so a caller can build a collider without
    /// re-parsing the GLB. A prop the player walks through is not in the
    /// world, it is painted on it.
    min: Vec3f,
    max: Vec3f,
    /// Physics collider boxes in model space — triangle-derived voxel boxes
    /// (model.rs voxel_collider_boxes): legs, decks, braces, openings.
    collider_parts: Vec<(Vec3f, Vec3f)>,
    /// Light-bake occluder boxes — the OLD curated primitive parts: few and
    /// face-aligned. Voxel boxes as occluders smeared streaks over every
    /// sloped roof (a stepped AABB pokes through the surface) and tripled
    /// the bake; physics and light need DIFFERENT simplifications.
    occluder_parts: Vec<(Vec3f, Vec3f)>,
    /// The light baker's view of this model — triangles + grid + chart uvs —
    /// present only when the model has its OWN AO layout (the layout is what
    /// gives every placed copy a lightmap parameterisation for free).
    lm_source: Option<std::sync::Arc<crate::lightmap::LmMeshSource>>,
    /// Rigid parts the game drives between named states (doors, lifts). Not
    /// in `geometry`: they move, so they are neither baked into the static
    /// lightmap nor part of the model's collider.
    anim_parts: Vec<LoadedAnimPart>,
    /// Rigid parts whose complete model-space pose is supplied by each
    /// ModelInstance (vehicle wheels are the first consumer).
    driven_parts: Vec<LoadedDrivenPart>,
    /// The map's sky surfaces, drawn by view direction instead of lit. Also
    /// out of `geometry`: the bake must never see them (they would shadow
    /// the whole level from above) and the sun must never shade them.
    sky: Option<LoadedSky>,
    /// The model's own triangles in MODEL space, kept after the GPU upload
    /// consumed the packed stream. This is the level-collision source
    /// (`level.rs` builds its BVH from it) — 16 bytes a vertex-and-index
    /// against re-parsing a 100 MB GLB to answer "what did the player walk
    /// into". Anim parts are deliberately absent: they move, and a BVH built
    /// over them would be wrong the moment a door opened.
    mesh_positions: Vec<Vec3f>,
    mesh_indices: Vec<u32>,
    /// The GPU light bake's variant of `geometry`: identical positions and
    /// chart uvs, but the normal lane holds the FLAT WINDING face normal.
    /// The CPU bake's `sun_bit` classified every texel against the winding
    /// normal of its owning triangle (a dedup'd double-sided kit face whose
    /// kept twin was authored inward carries a vertex normal OPPOSITE its
    /// winding — the crypt's paver-floor sheets), so the gather must see the
    /// winding, not the shading normal. Present iff `lm_source` is.
    bake_geometry: Option<Geometry>,
}

/// One resident [`crate::model::AnimPart`]: its definition (states, clip,
/// bounds) plus the uploaded geometry it draws through.
struct LoadedAnimPart {
    def: crate::model::AnimPart,
    /// (geometry, albedo, detail, detail scale) per texture, mirroring the
    /// static model's own layer list so a part draws through the same code.
    draws: Vec<(Geometry, Texture, Texture, [f32; 2])>,
    /// Node-local collider boxes, derived once at load.
    collider: Vec<(Vec3f, Vec3f)>,
}

struct LoadedDrivenPart {
    def: crate::model::DrivenPart,
    draws: Vec<(Geometry, Texture, Texture, [f32; 2])>,
}

/// Whether a sky projection's layer images get a mip chain.
///
/// A sky is sampled by DIRECTION, so `atan2` puts a branch cut in the
/// longitude: at one heading, two neighbouring pixels differ by a whole
/// texture period in u. The colour is fine — the sampler repeats — but the
/// hardware picks the mip level from exactly that derivative, so those
/// pixels collapse to the smallest level and the sky wears a one-pixel line
/// down the seam. (Measured on Doom E1M1: a dark column in the middle of the
/// sky, wherever the cut happened to be.)
///
/// A CYLINDER strip (Doom, Duke) is never minified — 256 texels x 4 wraps is
/// under 3 texels per degree against 8+ pixels per degree in any view that
/// exists — so its chain buys nothing and costs the seam. It ships without
/// one, and the seam has nowhere to come from.
///
/// The other two keep theirs: QUAKE_SCROLL has no cut at all (its uv is a
/// smooth function of the direction) and its zenith swirl genuinely
/// minifies; CUBE (Q3 equirect) minifies hard at the poles, where dropping
/// mips would trade a hairline for sparkle. Cube still has the cut — the
/// real fix there is an explicit LOD from ray-space derivatives, which needs
/// a repeat sampler that takes a lod.
fn sky_wants_mips(projection: crate::model::SkyProjection) -> bool {
    !matches!(projection, crate::model::SkyProjection::Cylinder)
}

/// A resident [`crate::model::SkyPart`]: the faces plus their layer images.
struct LoadedSky {
    part: crate::model::SkyPart,
    geometry: Geometry,
    /// Layer 0 and layer 1 textures. Layer 1 is a 1x1 transparent stand-in
    /// unless the map is a two-layer Quake sky, so the shader samples
    /// unconditionally and every projection stays one draw.
    tex0: Texture,
    tex1: Texture,
    /// Model-space triangles, kept for the same reason `mesh_positions` is:
    /// a sky face is still a WALL to a walker (Doom's sky brushes are
    /// solid), even though it is never lit or shadowed.
    positions: Vec<Vec3f>,
    indices: Vec<u32>,
}

/// Which placed geometry a model-state command addresses.
///
/// An imported level is ONE placed instance, so naming the model id is the
/// natural handle for its doors; a prop placed many times needs the slot.
/// `Instance` wins over `Model` when both are set for the same part.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelTarget {
    /// Every placed copy of this model id.
    Model(String),
    /// One slot in the [`Renderer::set_models`] list.
    Instance(usize),
}

impl From<&str> for ModelTarget {
    fn from(id: &str) -> Self {
        ModelTarget::Model(id.to_string())
    }
}
impl From<&String> for ModelTarget {
    fn from(id: &String) -> Self {
        ModelTarget::Model(id.clone())
    }
}
impl From<String> for ModelTarget {
    fn from(id: String) -> Self {
        ModelTarget::Model(id)
    }
}
impl From<usize> for ModelTarget {
    fn from(slot: usize) -> Self {
        ModelTarget::Instance(slot)
    }
}

/// One triggered part's clock. `time` is where the part IS, `target` where it
/// is heading; `speed` is clip-seconds per real second, fixed when the
/// command landed so a mid-move reversal takes the same wall-clock time as
/// the move it interrupts.
#[derive(Clone, Copy)]
struct AnimPartRuntime {
    state: usize,
    time: f32,
    target: f32,
    speed: f32,
}

/// Where every triggered part of every addressed model currently is.
///
/// Deliberately free of GPU state: a door's motion is a clock over a clip,
/// and keeping it separable is what lets the whole reversible state machine
/// be tested without a device.
#[derive(Default)]
struct ModelStates {
    map: std::collections::BTreeMap<(ModelTarget, String), AnimPartRuntime>,
}

impl ModelStates {
    /// Aim `def` at `state`. False (and no change) when the part has no such
    /// state. `blend_secs` times THIS move: the speed is fixed here, from the
    /// distance still to cover, so an interrupted move reverses at a
    /// comparable pace instead of snapping or crawling.
    fn set(
        &mut self,
        target: ModelTarget,
        def: &crate::model::AnimPart,
        state: &str,
        blend_secs: f32,
    ) -> bool {
        let Some(index) = def.state_index(state) else {
            return false;
        };
        let goal = def.state_time(index);
        let key = (target, def.name.clone());
        let now = self
            .map
            .get(&key)
            .map(|r| r.time)
            .unwrap_or_else(|| def.state_time(def.default));
        let speed = if blend_secs > 0.0 {
            ((goal - now).abs() / blend_secs).max(1.0e-6)
        } else {
            f32::INFINITY
        };
        let time = if speed.is_finite() { now } else { goal };
        self.map
            .insert(key, AnimPartRuntime { state: index, time, target: goal, speed });
        true
    }

    /// Advance every clock by `dt` seconds, linearly, stopping on target.
    fn tick(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        for run in self.map.values_mut() {
            if !run.speed.is_finite() {
                run.time = run.target;
                continue;
            }
            let step = run.speed * dt;
            if (run.target - run.time).abs() <= step {
                run.time = run.target;
            } else if run.target > run.time {
                run.time += step;
            } else {
                run.time -= step;
            }
        }
    }

    /// Drop every clock addressed at `id`, plus the listed placed slots —
    /// what a model going away means for its doors.
    fn forget_model(&mut self, id: &str, slots: &[usize]) {
        self.map.retain(|(target, _), _| match target {
            ModelTarget::Model(m) => m != id,
            ModelTarget::Instance(i) => !slots.contains(i),
        });
    }

    /// Drop every per-slot clock. Slot numbers only mean anything against one
    /// placed list, so a scene whose identity changed takes its per-slot
    /// commands with it; per-MODEL commands (how an imported level addresses
    /// its own doors) survive.
    fn forget_slots(&mut self) {
        self.map
            .retain(|(target, _), _| matches!(target, ModelTarget::Model(_)));
    }

    /// (state index, clip time, target time) for one part. A per-slot command
    /// wins over a per-model one; with neither, the part sits in the pose the
    /// file authored as its default.
    fn clock(
        &self,
        target: &ModelTarget,
        model_id: &str,
        def: &crate::model::AnimPart,
    ) -> (usize, f32, f32) {
        let by_slot = matches!(target, ModelTarget::Instance(_))
            .then(|| self.map.get(&(target.clone(), def.name.clone())))
            .flatten();
        let run = by_slot.or_else(|| {
            self.map
                .get(&(ModelTarget::Model(model_id.to_string()), def.name.clone()))
        });
        match run {
            Some(r) => (r.state, r.time, r.target),
            None => {
                let t = def.state_time(def.default);
                (def.default, t, t)
            }
        }
    }
}

/// A part's current state, as reported by [`Renderer::model_states`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModelPartState {
    /// The glTF node name — the handle `set_model_state` takes.
    pub part: String,
    /// Index into the part's `states`, and its name.
    pub state: usize,
    pub state_name: String,
    /// Where the part is on its clip, and where it is heading.
    pub time: f32,
    pub target_time: f32,
    /// The move has finished: the part sits exactly on `state`.
    pub settled: bool,
}

/// One placed instance's anim part, resolved into WORLD space for this
/// moment — what a walker collides with. Returned by
/// [`Renderer::anim_part_boxes`].
#[derive(Clone, Debug)]
pub struct AnimPartBox {
    /// Slot in the placed-model list, and the model id in it.
    pub instance: usize,
    pub model: String,
    pub part: String,
    /// `extras.kind` (`door`), for a caller that treats kinds differently.
    pub kind: Option<String>,
    pub state: usize,
    pub state_name: String,
    /// World-space collider boxes, already moved to where the part is now.
    pub boxes: Vec<(Vec3f, Vec3f)>,
    /// World AABB over `boxes` — a cheap broad-phase reject.
    pub min: Vec3f,
    pub max: Vec3f,
}

/// One placed stock prop. `model` is the asset id it was loaded under, e.g.
/// `kenney/car-kit/ambulance`.
#[derive(Clone)]
pub struct ModelInstance {
    pub model: String,
    pub transform: Mat4f,
    /// True for instances that follow a moving body (`on_body`). Dynamic
    /// instances are EXCLUDED from the baked static shadows — a driveable
    /// car's parked silhouette otherwise stays behind as a stain — and get a
    /// per-frame blob until their model's SDF silhouette atlas lands.
    pub dynamic: bool,
    /// Depth-tie order among coplanar stacked statics: pieces stay
    /// geometrically FLUSH and this feeds the shader's depth_bias
    /// (order * 1e-3 view-space scale) so the later-placed piece wins the
    /// z-tie. 0 for anything that never stacks (dynamics, lone props).
    pub depth_order: f32,
    /// Optional complete model-space poses for externally-driven rigid parts,
    /// keyed by their source-neutral connection name. Missing entries sit in
    /// the authored rest pose, so generic viewers need no special handling.
    pub part_poses: Vec<ModelPartPose>,
}

#[derive(Clone)]
pub struct ModelPartPose {
    pub connection: String,
    pub transform: Mat4f,
}

/// Read-only connection metadata exposed to game object loaders. All values
/// are in authored model space; callers apply the same uniform scale and
/// model-to-body basis they use for the visible instance.
#[derive(Clone, Debug)]
pub struct DrivenPartInfo {
    pub connection: String,
    pub pivot: Vec3f,
    pub anchor: Vec3f,
    pub radius: f32,
    pub width: f32,
    pub rest_transform: Mat4f,
}

/// Stable FNV-1a signature for the subset of a placed-model frame that can
/// invalidate static derived state. `DefaultHasher` is intentionally not
/// used: its algorithm is not a persistence/identity contract.
/// Longest world-space side, in metres, at which a static that owns no AO
/// layout stops being treated as a shadow caster in the bake.
///
/// A prop casts onto the world around it; a whole IMPORTED LEVEL is the world
/// around it, so feeding one into the sun-depth passes shadows its own
/// interior and every room goes black. 40 m is comfortably above any prop
/// (the biggest kit building measures ~20) and far below a map.
const CASTER_ONLY_MAX_SPAN: f32 = 40.0;

/// Does a static WITHOUT its own AO layout join the bake's sun-depth passes?
///
/// `explicit` is the caller's own answer for this model id
/// ([`Renderer::set_model_casts_shadow`]) and always wins — a host that knows
/// it is loading a world says so, and no heuristic overrules it. Otherwise a
/// prelit model (its light is already in COLOR_0, so the sun does not reach
/// it) and anything level-sized stay out.
fn casts_as_caster_only(explicit: Option<bool>, prelit: bool, min: Vec3f, max: Vec3f) -> bool {
    if let Some(answer) = explicit {
        return answer;
    }
    if prelit {
        return false;
    }
    let size = max - min;
    size.x.max(size.y).max(size.z) <= CASTER_ONLY_MAX_SPAN
}

/// Local collider boxes through a world matrix, plus the AABB over them.
///
/// Each box is re-fitted around its eight transformed corners rather than
/// rotated as a box: a level's parts are axis-aligned, where this is exact,
/// and a rotated one still gets a collider that fully contains it — a door
/// swinging on a hinge is never LESS solid than it looks.
fn world_boxes(m: &Mat4f, boxes: &[(Vec3f, Vec3f)]) -> (Vec<(Vec3f, Vec3f)>, Vec3f, Vec3f) {
    let mut out = Vec::with_capacity(boxes.len());
    let mut lo = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = vec3f(f32::MIN, f32::MIN, f32::MIN);
    for (bmin, bmax) in boxes {
        let mut blo = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut bhi = vec3f(f32::MIN, f32::MIN, f32::MIN);
        for x in [bmin.x, bmax.x] {
            for y in [bmin.y, bmax.y] {
                for z in [bmin.z, bmax.z] {
                    let p = m
                        .transform_vec4(Vec4f { x, y, z, w: 1.0 })
                        .to_vec3f();
                    blo.x = blo.x.min(p.x);
                    blo.y = blo.y.min(p.y);
                    blo.z = blo.z.min(p.z);
                    bhi.x = bhi.x.max(p.x);
                    bhi.y = bhi.y.max(p.y);
                    bhi.z = bhi.z.max(p.z);
                }
            }
        }
        lo.x = lo.x.min(blo.x);
        lo.y = lo.y.min(blo.y);
        lo.z = lo.z.min(blo.z);
        hi.x = hi.x.max(bhi.x);
        hi.y = hi.y.max(bhi.y);
        hi.z = hi.z.max(bhi.z);
        out.push((blo, bhi));
    }
    if out.is_empty() {
        (out, Vec3f::default(), Vec3f::default())
    } else {
        (out, lo, hi)
    }
}

fn placed_scene_signature(instances: &[ModelInstance]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    fn bytes(hash: &mut u64, input: &[u8]) {
        for byte in input {
            *hash ^= *byte as u64;
            *hash = hash.wrapping_mul(PRIME);
        }
    }

    let mut hash = OFFSET;
    bytes(&mut hash, &(instances.len() as u64).to_le_bytes());
    for instance in instances {
        bytes(&mut hash, &(instance.model.len() as u64).to_le_bytes());
        bytes(&mut hash, instance.model.as_bytes());
        bytes(&mut hash, &[u8::from(instance.dynamic)]);
        if !instance.dynamic {
            for value in instance.transform.v {
                bytes(&mut hash, &value.to_bits().to_le_bytes());
            }
            bytes(&mut hash, &instance.depth_order.to_bits().to_le_bytes());
        }
    }
    hash
}

impl ModelInstance {
    /// Hang a model off a moving body, anchored by the MODEL's own measured
    /// bounds rather than by the body's collision box.
    ///
    /// Kenney authors a model with its origin on the surface it stands on:
    /// every vehicle in `car-kit`, `racing`, `retro-urban-kit` and
    /// `toy-car-kit` measures `min.y == 0` with the tyres at zero, and a prop
    /// puts its feet there the same way. Some kits do not — track and road
    /// pieces carry a skirt below zero, ground tiles a slab — so the anchor
    /// is READ from `bounds`, never assumed. `min.y` is the model's floor
    /// wherever the exporter left it.
    ///
    /// `frame` is the body's world rotation and position with no scale (see
    /// [`Renderer::rigid_transform`]). `drop` is how far below the body's
    /// origin, **along the body's own down axis**, that floor should sit —
    /// i.e. where the ground is relative to the body. For anything resting
    /// directly on its box that is the box's half height; for a wheeled body
    /// it is not, because the box never touches the road (see
    /// `blocks::Car::contact_drop`). Applying it in body space rather than
    /// world Y is what keeps the mesh bolted on when the body pitches.
    ///
    /// # Facing: the anchor turns the model 180°
    ///
    /// glTF defines an asset's FRONT as **+Z** (the spec's own words), and
    /// the vehicle kits follow it — measured on `car-kit/ambulance`, whose
    /// windshield and headlights sit on the model's +Z face. The engine's
    /// forward is **−Z** (`sim::heading` — every fixture assumes it). Passing
    /// the model's axes straight through therefore bolts every vehicle on
    /// TAIL-FIRST: parked it is invisible (a parked car has no "supposed"
    /// heading), but the moment it drives, throttle pulls it out of its own
    /// visual rear and a correctly-yawing car seen driving backwards reads as
    /// mirrored steering — reported as "flippers reversed AND steering
    /// reversed", one root for both. So the anchor rotates the model half a
    /// turn about Y, once, here — the boundary where authored-model space
    /// meets body space — rather than as a sign negated at some call site.
    pub fn on_body(
        model: String,
        bounds: (Vec3f, Vec3f),
        scale: f32,
        drop: f32,
        frame: &Mat4f,
    ) -> Self {
        let (min, max) = bounds;
        // Model space → body space: yaw π (glTF +Z front → engine −Z
        // forward) times uniform scale — for a half turn about Y that is
        // exactly negated x/z columns — then move the model's own floor to
        // `drop` below the origin and its own horizontal centre onto the
        // body's axis. Floor and centre are measured, so a kit that authors
        // its origin in a corner of the scene grid still lands on the body.
        let mut local = Mat4f::identity();
        local.v[0] = -scale;
        local.v[5] = scale;
        local.v[10] = -scale;
        local.v[12] = (min.x + max.x) * 0.5 * scale;
        local.v[13] = -drop - min.y * scale;
        local.v[14] = (min.z + max.z) * 0.5 * scale;
        Self {
            model,
            transform: Mat4f::mul(frame, &local),
            dynamic: true,
            depth_order: 0.0,
            part_poses: Vec::new(),
        }
    }
}

fn perf_us(t0: std::time::Instant) -> u64 {
    t0.elapsed().as_micros() as u64
}

/// Fold a baked shade multiplier into an instance colour.
///
/// Emissive surfaces opt out in proportion to their glow: a beacon in a
/// tunnel is the light source, so darkening it reads as a bug rather than
/// as occlusion. Alpha is never touched — for the alpha batch that would
/// change coverage, not brightness.
fn shade_color(color: Vec4f, shade: f32, glow: f32) -> Vec4f {
    let k = shade + (1.0 - shade) * glow.clamp(0.0, 1.0);
    vec4(color.x * k, color.y * k, color.z * k, color.w)
}

/// Hard cap on per-draw dynamic lights — 8 pairs of vec4 uniforms, the
/// Quest budget line. Selection below never returns more.
pub const MAX_DYNAMIC_LIGHTS: usize = 8;

/// Brightest component — the "how strong is this light" scalar the
/// selection ranks by.
fn light_intensity(l: &crate::lightmap::LmLight) -> f32 {
    l.color.x.max(l.color.y).max(l.color.z)
}

/// The transient flash a bursting firework shell throws on the world.
///
/// `None` while the shell is climbing (negative age) or after its sparks
/// die. Colour comes from the shell; intensity pops hard at the burst (the
/// same quarter-second "heat" window the spark shader whitens with) and
/// decays quadratically over the shell's life. Radius is generous — a shell
/// bursts 30-46 units up and the pool it throws on the street below is the
/// whole point.
pub fn firework_flash_light(
    f: &crate::firework::FireworkInstance,
) -> Option<crate::lightmap::LmLight> {
    if f.age < 0.0 || f.age >= f.life || f.life <= 0.0 {
        return None;
    }
    let t = (f.age / f.life).clamp(0.0, 1.0);
    let fade = (1.0 - t) * (1.0 - t);
    let heat = (1.0 - f.age * 4.0).clamp(0.0, 1.0);
    let s = fade * (1.2 + 1.8 * heat);
    Some(crate::lightmap::LmLight::omni(
        f.origin,
        vec3f(f.color.x * s, f.color.y * s, f.color.z * s),
        55.0,
    ))
}

/// Rank `lights` by their strongest contribution to ANY anchor point and
/// append up to `max` indices to `out`, strongest first, skipping indices
/// already in `out`. Score is intensity × (1 - d/radius)² at the nearest
/// anchor; lights whose radius reaches no anchor are rejected outright.
/// `range` restricts which light indices compete (so lamps and transients
/// can be ranked separately). No allocation in the steady state — `rank`
/// and `out` are caller-owned scratch.
pub fn select_strongest_lights(
    lights: &[crate::lightmap::LmLight],
    range: std::ops::Range<usize>,
    anchors: &[Vec3f],
    max: usize,
    rank: &mut Vec<(f32, usize)>,
    out: &mut Vec<usize>,
) {
    rank.clear();
    for i in range {
        let l = &lights[i];
        if l.radius <= 0.0 || out.contains(&i) {
            continue;
        }
        let mut best_d2 = f32::MAX;
        for a in anchors {
            let (dx, dy, dz) = (a.x - l.pos.x, a.y - l.pos.y, a.z - l.pos.z);
            best_d2 = best_d2.min(dx * dx + dy * dy + dz * dz);
        }
        if best_d2 >= l.radius * l.radius {
            continue; // early radius reject: reaches no anchor
        }
        let att = 1.0 - best_d2.sqrt() / l.radius;
        rank.push((light_intensity(l) * att * att, i));
    }
    rank.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, i) in rank.iter().take(max.min(MAX_DYNAMIC_LIGHTS)) {
        out.push(*i);
    }
}

/// Selection for the WORLD-SPANNING batches (cube slabs, terrain): there is
/// no meaningful batch anchor, so lights are kept when their sphere touches
/// the frustum and ranked by intensity with a soft distance-to-camera bias
/// (no hard radius reject — a shell bursting far from the camera still
/// lights the visible ground beneath it). v1 per-frame selection; a chunked
/// world would want per-chunk lists.
fn select_lights_for_world(
    lights: &[crate::lightmap::LmLight],
    range: std::ops::Range<usize>,
    camera: Vec3f,
    frustum: Option<&Frustum>,
    rank: &mut Vec<(f32, usize)>,
    out: &mut Vec<usize>,
) {
    rank.clear();
    out.clear();
    for i in range {
        let l = &lights[i];
        if l.radius <= 0.0 {
            continue;
        }
        if let Some(f) = frustum {
            if !f.intersects_sphere(l.pos, l.radius) {
                continue;
            }
        }
        let (dx, dy, dz) = (camera.x - l.pos.x, camera.y - l.pos.y, camera.z - l.pos.z);
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        rank.push((light_intensity(l) * l.radius / (l.radius + d), i));
    }
    rank.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, i) in rank.iter().take(MAX_DYNAMIC_LIGHTS) {
        out.push(*i);
    }
}

/// Write a selected light list into a shader's `dl_*` uniforms: 8 pairs of
/// vec4 (pos.xyz + radius, rgb + spot), unfilled slots zeroed so the shader
/// rejects them on radius. `split` is how many leading slots hold TRANSIENT
/// lights — DrawSceneSkinned's per-instance static gate reads it as
/// `dl_split`; shaders without that uniform ignore the write.
///
/// The shader-side spot term assumes emission straight DOWN (the harvested
/// street-lamp convention); a spot light with any other `dir` will still
/// light, but its cone will point down. Fine for v1 — lamps are the only
/// spot sources.
/// Write a pre-packed light block (light_grid.rs layout — 8 × pos/col vec4
/// pairs) straight into a shader's `dl_*` uniforms. The per-instance path:
/// the append test compares dyn uniforms, so instances sharing a block still
/// merge into one draw item and instances in different cells split — which
/// is the point (a car must never flicker because a batch-shared ranking
/// churned under it).
fn write_light_block(
    cx: &Cx,
    dv: &mut DrawVars,
    packed: &[f32; LIGHT_BLOCK_FLOATS],
    split: usize,
) {
    let pos_ids = [
        live_id!(dl_pos0),
        live_id!(dl_pos1),
        live_id!(dl_pos2),
        live_id!(dl_pos3),
        live_id!(dl_pos4),
        live_id!(dl_pos5),
        live_id!(dl_pos6),
        live_id!(dl_pos7),
    ];
    let col_ids = [
        live_id!(dl_col0),
        live_id!(dl_col1),
        live_id!(dl_col2),
        live_id!(dl_col3),
        live_id!(dl_col4),
        live_id!(dl_col5),
        live_id!(dl_col6),
        live_id!(dl_col7),
    ];
    for slot in 0..MAX_DYNAMIC_LIGHTS {
        dv.set_uniform(cx, pos_ids[slot], &packed[slot * 8..slot * 8 + 4]);
        dv.set_uniform(cx, col_ids[slot], &packed[slot * 8 + 4..slot * 8 + 8]);
    }
    dv.set_uniform(cx, live_id!(dl_split), &[split as f32]);
}

fn write_light_uniforms(
    cx: &Cx,
    dv: &mut DrawVars,
    lights: &[crate::lightmap::LmLight],
    sel: &[usize],
    split: usize,
) {
    let pos_ids = [
        live_id!(dl_pos0),
        live_id!(dl_pos1),
        live_id!(dl_pos2),
        live_id!(dl_pos3),
        live_id!(dl_pos4),
        live_id!(dl_pos5),
        live_id!(dl_pos6),
        live_id!(dl_pos7),
    ];
    let col_ids = [
        live_id!(dl_col0),
        live_id!(dl_col1),
        live_id!(dl_col2),
        live_id!(dl_col3),
        live_id!(dl_col4),
        live_id!(dl_col5),
        live_id!(dl_col6),
        live_id!(dl_col7),
    ];
    for slot in 0..MAX_DYNAMIC_LIGHTS {
        let (p, c) = match sel.get(slot).map(|i| &lights[*i]) {
            Some(l) => (
                [l.pos.x, l.pos.y, l.pos.z, l.radius],
                [l.color.x, l.color.y, l.color.z, l.spot],
            ),
            None => ([0.0; 4], [0.0; 4]),
        };
        dv.set_uniform(cx, pos_ids[slot], &p);
        dv.set_uniform(cx, col_ids[slot], &c);
    }
    dv.set_uniform(cx, live_id!(dl_split), &[split as f32]);
}

/// World-unit slack on every cull test. The bounds themselves are measured
/// (mesh min/max, entity half extents), so this only has to absorb float
/// rounding across the frustum math — but a skipped visible object is a real
/// bug and an extra drawn one is not, so it is sized generously.
const CULL_MARGIN: f32 = 0.5;

/// Side of a world-space culling cell. The tradeoff: smaller cells cull
/// tighter but cost more per-frame AABB tests and — for the chunked shadow
/// geometry and terrain tiles, which draw one item per visible cell — more
/// draw items on a tiler. 48 keeps the village in a handful of cells and a
/// Zelda-scale map under ~100, while a ground-level camera still rejects
/// most of them. (The static instance SLABS are chunked too but share the
/// per-shape draw item, so their count is unaffected by this value.)
pub const CHUNK_SIZE: f32 = 48.0;

/// Which grid cell a world-space point lives in. Floor, not truncation:
/// negative coordinates must not fold cell -1 onto cell 0.
fn chunk_cell(x: f32, z: f32) -> (i32, i32) {
    (
        (x / CHUNK_SIZE).floor() as i32,
        (z / CHUNK_SIZE).floor() as i32,
    )
}

/// The view frustum in world space — the space instance transforms live in,
/// which is why the clip matrix handed to [`Frustum::from_clip_matrix`] must
/// include the stage transform (shaders compute
/// `projection * view * view_transform * transform`).
///
/// Planes come from the rows of the clip matrix (Gribb-Hartmann) and point
/// INWARD: a point is inside a plane when its signed distance is >= 0. Every
/// test below is conservative — it only rejects what is provably beyond ONE
/// plane, so anything straddling or spanning the frustum always draws.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [Vec4f; 6],
}

impl Frustum {
    /// `clip` maps world space to clip space, column-vector convention as
    /// `Mat4f` stores it — row `i` is `v[i], v[i+4], v[i+8], v[i+12]`.
    pub fn from_clip_matrix(clip: &Mat4f) -> Self {
        let row = |i: usize| vec4(clip.v[i], clip.v[i + 4], clip.v[i + 8], clip.v[i + 12]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        // Near uses the GL convention (-w <= z). A backend that clips at
        // 0 <= z has a tighter near plane, so testing against the looser one
        // only ever keeps more — conservative in the direction that matters.
        let mut planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2];
        // Normalised so caller margins are world units.
        for p in planes.iter_mut() {
            let len = vec3f(p.x, p.y, p.z).length();
            if len > 1.0e-8 {
                *p = *p * (1.0 / len);
            }
        }
        Self { planes }
    }

    fn distance(p: Vec4f, v: Vec3f) -> f32 {
        p.x * v.x + p.y * v.y + p.z * v.z + p.w
    }

    /// Sphere-vs-frustum; false only when the sphere is fully beyond a plane.
    pub fn intersects_sphere(&self, center: Vec3f, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| Self::distance(*p, center) >= -(radius + CULL_MARGIN))
    }

    /// World-axis-aligned box vs the frustum, via the positive-vertex trick:
    /// per plane, only the corner most inside can decide "fully outside".
    /// Same conservatism as the corner tests — straddlers always pass.
    pub fn intersects_aabb(&self, min: Vec3f, max: Vec3f) -> bool {
        self.planes.iter().all(|p| {
            let v = vec3f(
                if p.x >= 0.0 { max.x } else { min.x },
                if p.y >= 0.0 { max.y } else { min.y },
                if p.z >= 0.0 { max.z } else { min.z },
            );
            Self::distance(*p, v) >= -CULL_MARGIN
        })
    }

    /// Model-space AABB under `transform` vs the frustum: the 8 transformed
    /// corners, rejected only when all of them sit beyond one plane. That is
    /// exact for "entirely outside plane P" and never false-culls, because a
    /// box with any part inside keeps at least one corner inside every plane's
    /// reject test.
    pub fn intersects_obb(&self, min: Vec3f, max: Vec3f, transform: &Mat4f) -> bool {
        let t = &transform.v;
        let mut corners = [Vec3f::default(); 8];
        for (i, c) in corners.iter_mut().enumerate() {
            let l = vec3f(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            *c = vec3f(
                t[0] * l.x + t[4] * l.y + t[8] * l.z + t[12],
                t[1] * l.x + t[5] * l.y + t[9] * l.z + t[13],
                t[2] * l.x + t[6] * l.y + t[10] * l.z + t[14],
            );
        }
        for p in &self.planes {
            if corners
                .iter()
                .all(|c| Self::distance(*p, *c) < -CULL_MARGIN)
            {
                return false;
            }
        }
        true
    }
}

/// One world-grid cell's packed static instances, per shape and pass. The
/// bounds are CONTENT bounds — the union of what is actually packed here
/// (height included), never the cell footprint — so a tall tower culls by
/// what it is, and a cell whose content leans over the border still tests
/// correctly. Chunks exist only when something packed into them, so an
/// empty cell x shape combination costs nothing anywhere.
struct SlabChunk {
    cell: (i32, i32),
    min: Vec3f,
    max: Vec3f,
    slab: [Vec<f32>; 5],
    slab_alpha: [Vec<f32>; 5],
}

/// Settle delay before the world-settle work (receiver refresh + lightmap
/// kick) runs after an edit. A burst of mutations (a game spawning
/// platforms, a dragged kinematic) pays ONE refresh once the world has been
/// still this long — "only re-render the shadows when an object stops
/// moving" — instead of one per mutation.
const SHADOW_SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

/// Coalescing debounce for the settle work. Pure state machine — the caller
/// supplies the clock — so the burst behaviour is testable without threads
/// or sleeps. However many times the key moves while pending, at most one
/// rebuild fires, for the LATEST key, once it has been stable for the
/// settle window; the very first build runs immediately, because there is
/// no stale state to keep showing in its place.
#[derive(Default)]
struct ShadowRebuildGate {
    /// The key the current chunks were built for. None = never built.
    built: Option<(u64, u64, u64, u32)>,
    /// Latest key seen since `built`, and when it last CHANGED.
    pending: Option<((u64, u64, u64, u32), std::time::Instant)>,
}

impl ShadowRebuildGate {
    /// True when the caller should rebuild for `key` NOW. Call `mark_built`
    /// after doing so, or the gate will keep saying yes.
    fn should_rebuild(
        &mut self,
        key: (u64, u64, u64, u32),
        now: std::time::Instant,
        settle: std::time::Duration,
    ) -> bool {
        if self.built == Some(key) {
            self.pending = None;
            return false;
        }
        if self.built.is_none() {
            return true;
        }
        match self.pending {
            Some((k, since)) if k == key => now.duration_since(since) >= settle,
            // New key (first change, or changed again mid-wait): the settle
            // clock restarts — the world is still being edited.
            _ => {
                self.pending = Some((key, now));
                false
            }
        }
    }

    fn mark_built(&mut self, key: (u64, u64, u64, u32)) {
        self.built = Some(key);
        self.pending = None;
    }
}

/// One tile of the terrain mesh: the same triangles the whole-mesh builder
/// emitted for its cell range, regrouped so offscreen tiles skip the draw.
struct TerrainTile {
    min: Vec3f,
    max: Vec3f,
    geometry: Geometry,
}

/// One uploaded voxel chunk mesh. The vertex layout is the terrain tiles'
/// PbrVertex (the sim mesher emits it directly), so DrawSceneTerrain draws
/// both without knowing which is which.
struct VoxelTile {
    key: ChunkKey,
    rev: u64,
    min: Vec3f,
    max: Vec3f,
    geometry: Geometry,
}

/// One `game.water` volume's flat sheet grid plus its packed wave uniforms.
/// The grid is built at the STILL level; every displacement happens in the
/// vertex shader, so the bounds carry the amplitude headroom for culling.
struct WaterTile {
    min: Vec3f,
    max: Vec3f,
    geometry: Geometry,
    waves_a: [[f32; 4]; MAX_WAVES],
    waves_b: [[f32; 4]; MAX_WAVES],
}

/// Pack a volume's wave list into the shader's uniform slots — the RAW
/// WaterWave fields, bit-for-bit, no unit conversion: this function being
/// trivial IS the CPU/GPU agreement story (the pin test below holds the
/// shader to the same expression over these same numbers). Unused slots are
/// zero; a zero amplitude contributes nothing in the shader.
pub fn pack_wave_uniforms(
    volume: &WaterVolume,
) -> ([[f32; 4]; MAX_WAVES], [[f32; 4]; MAX_WAVES]) {
    let mut a = [[0.0f32; 4]; MAX_WAVES];
    let mut b = [[0.0f32; 4]; MAX_WAVES];
    for (i, w) in volume.waves.iter().take(MAX_WAVES).enumerate() {
        a[i] = [w.dir_x, w.dir_z, w.k, w.omega];
        b[i] = [w.amp, w.phase, w.group, 0.0];
    }
    (a, b)
}

/// Build one volume's sheet grid (PbrVertex layout, indexed). Resolution
/// follows the shortest wavelength — eight cells per wave is enough for the
/// crest to read as a curve — clamped so a huge bay stays a few thousand
/// triangles.
fn water_sheet_data(volume: &WaterVolume) -> (Vec<f32>, Vec<u32>, Vec3f, Vec3f) {
    let span_x = (volume.max.x - volume.min.x).max(0.01);
    let span_z = (volume.max.z - volume.min.z).max(0.01);
    let shortest = volume
        .waves
        .iter()
        .map(|w| std::f32::consts::TAU / w.k.max(1.0e-3))
        .fold(f32::MAX, f32::min);
    let target = if shortest == f32::MAX {
        // Still water: a coarse sheet is enough.
        span_x.max(span_z) / 8.0
    } else {
        shortest / 8.0
    }
    .clamp(0.5, 16.0);
    let nx = ((span_x / target).ceil() as usize).clamp(1, 128);
    let nz = ((span_z / target).ceil() as usize).clamp(1, 128);
    let level = volume.level();
    let color = volume.color;
    let mut vertices: Vec<f32> = Vec::with_capacity((nx + 1) * (nz + 1) * 16);
    for gz in 0..=nz {
        for gx in 0..=nx {
            let x = volume.min.x + span_x * (gx as f32 / nx as f32);
            let z = volume.min.z + span_z * (gz as f32 / nz as f32);
            // PbrVertex: pos_nx, ny_nz_uv, color, tangent — 16 floats. The
            // normal here is a placeholder; the vertex shader replaces it
            // with the analytic wave normal.
            vertices.extend_from_slice(&[
                x, level, z, 0.0, 1.0, 0.0, 0.0, 0.0, color.x, color.y, color.z, color.w,
                1.0, 0.0, 0.0, 1.0,
            ]);
        }
    }
    let stride = (nx + 1) as u32;
    let mut indices: Vec<u32> = Vec::with_capacity(nx * nz * 6);
    for gz in 0..nz as u32 {
        for gx in 0..nx as u32 {
            let a = gz * stride + gx;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            // Same diagonal split as the terrain, CCW seen from +y.
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let headroom = volume.amp_sum() + 0.5;
    let min = vec3f(volume.min.x, level - headroom, volume.min.z);
    let max = vec3f(volume.max.x, level + headroom, volume.max.z);
    (vertices, indices, min, max)
}

/// Emit terrain triangles for grid cells [gx0..gx1) x [gz0..gz1) — exactly
/// the primitives the single-mesh builder produced for those cells, in the
/// same per-cell order, so tiling regroups the mesh without changing one
/// triangle. Returns (vertices, indices, min, max); bounds come from the
/// emitted vertices, heights included.
fn terrain_tile_data(
    terrain: &Terrain,
    materials: Option<&TerrainMaterials>,
    gx0: usize,
    gx1: usize,
    gz0: usize,
    gz1: usize,
) -> (Vec<f32>, Vec<u32>, Vec3f, Vec3f) {
    let n = terrain.cells;
    let mut vertices: Vec<f32> = Vec::with_capacity((gx1 - gx0) * (gz1 - gz0) * 2 * 3 * 16);
    let mut indices: Vec<u32> = Vec::with_capacity((gx1 - gx0) * (gz1 - gz0) * 6);
    let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
    let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
    let world_pos = |gx: usize, gz: usize| -> Vec3f {
        vec3f(
            terrain.origin + gx as f32 * terrain.cell_size,
            terrain.heights[gz * n + gx],
            terrain.origin + gz as f32 * terrain.cell_size,
        )
    };
    let mut push_tri = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, color: Vec4f| {
        let normal = Vec3f::cross(b - a, c - a).normalize();
        for p in [a, b, c] {
            // PbrVertex: pos_nx, ny_nz_uv, color, tangent — 16 floats.
            vertices.extend_from_slice(&[
                p.x, p.y, p.z, normal.x, normal.y, normal.z, 0.0, 0.0, color.x, color.y,
                color.z, color.w, 1.0, 0.0, 0.0, 1.0,
            ]);
            indices.push(vertices.len() as u32 / 16 - 1);
            min = vec3f(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
            max = vec3f(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
        }
    };
    for gz in gz0..gz1 {
        for gx in gx0..gx1 {
            // 0xFF is the hole value (mix.md T4): the voxel field took over
            // this cell's surface — box3d skips it, and so do we.
            if let Some(m) = materials {
                if m.indices.get(gz * (n - 1) + gx) == Some(&0xFF) {
                    continue;
                }
            }
            let a = world_pos(gx, gz);
            let b = world_pos(gx + 1, gz);
            let c = world_pos(gx, gz + 1);
            let d = world_pos(gx + 1, gz + 1);
            let color_at = |gx: usize, gz: usize| terrain.colors[gz * n + gx];
            let c0 = color_at(gx, gz);
            let c1 = color_at(gx + 1, gz);
            let c2 = color_at(gx, gz + 1);
            let c3 = color_at(gx + 1, gz + 1);
            let avg3 = |x: Vec4f, y: Vec4f, z: Vec4f| {
                vec4(
                    (x.x + y.x + z.x) / 3.0,
                    (x.y + y.y + z.y) / 3.0,
                    (x.z + y.z + z.z) / 3.0,
                    1.0,
                )
            };
            // Same diagonal split as Terrain::height_at, CCW seen from +y.
            push_tri(&mut vertices, &mut indices, a, c, b, avg3(c0, c2, c1));
            push_tri(&mut vertices, &mut indices, b, c, d, avg3(c1, c2, c3));
        }
    }
    (vertices, indices, min, max)
}

/// Write one [`SunLight`] into every game shader. This is the whole of the
/// T7 unification on the game side: before it, cube/terrain/skinned each
/// hardcoded their own ambient/direct split and five script blocks set the
/// light direction by hand. Skinned is applied in `draw_skinned_inner`,
/// which owns that struct.
pub(crate) fn apply_sun(cx: &Cx, draws: &mut SceneDraws, sun: &SunLight, fog_color: Vec3f) {
    // The cube family batches thousands of instances, so its sun and fog
    // colour ride in uniforms; only the direction stays per-instance (it
    // belongs to DrawCube, shared with consumers outside this crate).
    draws.cube.cube.light_dir = sun.dir;
    sun.write_uniforms(cx, &mut draws.cube.cube.draw_vars);
    draws
        .cube
        .cube
        .draw_vars
        .set_uniform(cx, live_id!(fog_color), &[fog_color.x, fog_color.y, fog_color.z]);
    draws.alpha.cube.cube.light_dir = sun.dir;
    sun.write_uniforms(cx, &mut draws.alpha.cube.cube.draw_vars);
    draws.alpha.cube.cube.draw_vars.set_uniform(
        cx,
        live_id!(fog_color),
        &[fog_color.x, fog_color.y, fog_color.z],
    );
    // Terrain and skinned draw one instance each, so there is nothing to
    // save by moving their sun off the instance stream.
    sun.write_into(
        &mut draws.terrain.light_dir,
        &mut draws.terrain.sun_color,
        &mut draws.terrain.sun_sky,
        &mut draws.terrain.sun_ground,
    );
}

/// Does this script-configured sky take the ANALYTIC (Preetham) dome? Yes
/// while the DOME colours are the stock defaults — a script that authored
/// its own top/ground palette keeps the gradient it asked for. The horizon
/// colour is deliberately NOT part of the test: it only ever tinted the fog
/// (the village demo customises exactly that), and analytic mode derives
/// its fog from the model instead.
fn sky_wants_analytic(sky: &makepad_game_sim::SkyConfig) -> bool {
    let d = makepad_game_sim::SkyConfig::default();
    let close = |a: Vec4f, b: Vec4f| {
        (a.x - b.x).abs() < 1.0e-3 && (a.y - b.y).abs() < 1.0e-3 && (a.z - b.z).abs() < 1.0e-3
    };
    close(sky.top, d.top) && close(sky.ground, d.ground) && close(sky.ground_bottom, d.ground_bottom)
}

/// Sky turbidity (haze), MAKEPAD_SKY_TURBIDITY overridable; 2.5 is a clear
/// day with enough aerosol for a warm horizon.
fn sky_turbidity() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MAKEPAD_SKY_TURBIDITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2.5)
    })
}

/// Sky exposure for the Preetham tone-map (MAKEPAD_SKY_EXPOSURE); tuned so
/// the default noon matches the hand-painted dome's brightness.
fn sky_exposure() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MAKEPAD_SKY_EXPOSURE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.1)
    })
}

/// Desktop-class default; see [`Renderer::set_shadow_budget`].
pub const DEFAULT_SHADOW_BUDGET: usize = 24;

/// Palette-texture width in texels. Power of two: `(i + 0.5) / width` is then
/// exact in f32, so nearest sampling at texel centres cannot land on a
/// neighbouring matrix row. 128 texels = 42 joints per row; palettes wrap
/// rows freely because the shader addresses texels by flat index.
const JOINT_TEX_WIDTH: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorldModelLane {
    Placed,
    Attachment,
}

impl WorldModelLane {
    fn is_dynamic(self, inst: &ModelInstance) -> bool {
        self == Self::Attachment || inst.dynamic
    }

    fn light_key(self, slot: usize) -> u64 {
        // Top-bit namespaces are deliberately disjoint from skinned actors
        // (0x8000...) and from each other. Stable slot keys let the light
        // grid's dead-band remember an attached prop without allowing it to
        // disturb a placed car's light selection.
        match self {
            Self::Placed => 0x4000_0000_0000_0000 | slot as u64,
            Self::Attachment => 0xC000_0000_0000_0000 | slot as u64,
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self {
            shape_geometries: Default::default(),
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
            skin_palette_tex: None,
            skin_palette_texels: 0,
            skin_joint_bases: Vec::new(),
            static_models: Vec::new(),
            sky_draw: None,
            sky_time: 0.0,
            model_casts_shadow: std::collections::BTreeMap::new(),
            model_anim_state: ModelStates::default(),
            placed_models: Vec::new(),
            csm_static_casters: Vec::new(),
            world_attachments: Vec::new(),
            view_models: Vec::new(),
            placed_scene_signature: None,
            stage: Stage::default(),
            shadow_budget: DEFAULT_SHADOW_BUDGET,
            particle_instances: Vec::new(),
            ao_textures: Vec::new(),
            model_pack: Vec::new(),
            firework_instances: Vec::new(),
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
            pbr_materials_enabled: true,
            ssao: None,
            lm_remaps: Vec::new(),
            lm_ground: None,
            lm_top: None,
            lm_top_fallback: None,
            lightmap_enabled: !matches!(
                std::env::var("MAKEPAD_LIGHTMAP").as_deref(),
                Ok("off") | Ok("0") | Ok("false")
            ),
            lm_debug: if std::env::var("SANDBOX_LM_DEBUG").is_ok() { 1.0 } else { 0.0 },
            display_transform: 0.0,
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
            lamp_cache: Vec::new(),
            lamp_cache_rev: None,
            light_grid: LightGrid::default(),
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
            lm_kick_key: None,
            shadow_geometry: None,
            last_dynamic_shadow_tris: 0,
            shadow_points: Vec::new(),
            shadow_gate: ShadowRebuildGate::default(),
            models_rev: 0,
            lm_box_geometry: None,
            bake: LightBake::default(),
            csm_focus: None,
            csm_scene_bounds: None,
        }
    }
}

/// One settled bake's atlas signature, and — for every bake after the
/// first — its DIFF against that first one. Re-baking a world nothing has
/// edited must land on the same bytes; a bake that only ever grows the lit
/// set is an accumulator that was not cleared (gpu_lightmap.rs, section 1,
/// carries the measurement this instrument produced).
///
/// Driven by MAKEPAD_GPU_LM_REBAKE. macOS only: it needs a texture readback.
#[cfg(target_os = "macos")]
fn lm_probe_report(
    k: usize,
    w: usize,
    h: usize,
    regions: usize,
    lamps: usize,
    bytes: &[u8],
    rects: &[(crate::lightmap::LmRect, bool)],
) {
    thread_local! {
        static REFERENCE: std::cell::RefCell<Option<Vec<u8>>> =
            const { std::cell::RefCell::new(None) };
    }
    let mut hash = 0xcbf29ce484222325u64;
    let mut sum = [0u64; 4];
    let mut mx = [0u8; 4];
    for px in bytes.chunks_exact(4) {
        for c in 0..4 {
            hash = (hash ^ px[c] as u64).wrapping_mul(0x100000001b3);
            sum[c] += px[c] as u64;
            mx[c] = mx[c].max(px[c]);
        }
    }
    let px_count = (bytes.len() / 4).max(1) as f64;
    log!(
        "lm probe: bake {k} — {w}x{h}, {regions} regions, {lamps} lamp(s), hash {hash:016x}, \
         mean BGRA {:.3} {:.3} {:.3} {:.1}, max {:?}",
        sum[0] as f64 / px_count,
        sum[1] as f64 / px_count,
        sum[2] as f64 / px_count,
        sum[3] as f64 / px_count,
        mx
    );
    REFERENCE.with(|r| {
        let mut r = r.borrow_mut();
        let Some(prev) = r.as_ref() else {
            *r = Some(bytes.to_vec());
            return;
        };
        if prev.len() != bytes.len() {
            log!("lm probe: bake {k} — atlas resized, no diff against bake 0");
            return;
        }
        let (mut diff, mut up, mut down) = (0u64, 0u64, 0u64);
        let mut worst = 0i32;
        let mut worst_at = (0usize, 0usize, 0usize);
        let mut sum_delta = 0i64;
        for (i, (a, b)) in prev.chunks_exact(4).zip(bytes.chunks_exact(4)).enumerate() {
            let mut any = false;
            for c in 0..4 {
                let d = b[c] as i32 - a[c] as i32;
                if d != 0 {
                    any = true;
                    sum_delta += d as i64;
                    if d.abs() > worst.abs() {
                        worst = d;
                        worst_at = (i % w, i / w, c);
                    }
                }
            }
            if any {
                diff += 1;
                let sa: i32 = a[..3].iter().map(|v| *v as i32).sum();
                let sb: i32 = b[..3].iter().map(|v| *v as i32).sum();
                if sb > sa {
                    up += 1;
                } else if sb < sa {
                    down += 1;
                }
            }
        }
        let owner = rects
            .iter()
            .position(|(rc, _)| {
                worst_at.0 >= rc.x
                    && worst_at.0 < rc.x + rc.w
                    && worst_at.1 >= rc.y
                    && worst_at.1 < rc.y + rc.h
            })
            .map(|i| i as i64)
            .unwrap_or(-1);
        log!(
            "lm probe: bake {k} vs 0 — {diff} texels differ (rgb up {up}, down {down}), \
             worst delta {worst} at ({},{}) chan {} region {owner}, sum delta {sum_delta}",
            worst_at.0,
            worst_at.1,
            worst_at.2
        );
    });
}

impl Renderer {
    /// Begin rendering a different realm into this view.
    ///
    /// World-local revision counters restart when a script game is loaded,
    /// so they cannot by themselves distinguish (for example) town terrain
    /// at revision 1 from FPS terrain at revision 1. Call this exactly once
    /// at the successful world-replacement boundary, before submitting the
    /// new realm's first frame. It invalidates every cache derived from the
    /// old world while retaining uploaded asset geometry/textures, shader
    /// and pass pools, stage policy, bake settings, shadow budget, and the
    /// adaptive-quality history owned by this device.
    pub fn enter_realm(&mut self) {
        self.static_chunks.clear();
        self.chunk_visible.clear();
        self.slab_key = None;
        self.slab_instance_count = 0;

        self.terrain_tiles.clear();
        self.terrain_revision = 0;
        self.voxel_tiles.clear();
        self.water_tiles.clear();
        self.water_rev = None;

        // Keep resident rig geometry and the palette texture allocation;
        // only their frame-to-instance mapping belongs to the old realm.
        self.skin_joint_bases.clear();
        self.placed_models.clear();
        self.csm_static_casters.clear();
        self.world_attachments.clear();
        self.view_models.clear();
        self.placed_scene_signature = None;
        self.particle_instances.clear();
        self.firework_instances.clear();

        self.shadow_mesh.clear();
        self.receiver_boxes.clear();
        self.occluder_boxes.clear();
        self.shadow_geometry = None;
        self.last_dynamic_shadow_tris = 0;
        self.shadow_points.clear();
        self.shadow_gate = ShadowRebuildGate::default();

        self.lightmap = None;
        self.lm_remaps.clear();
        self.lm_ground = None;
        self.lm_top = None;
        self.gpu_baker.enter_realm();
        self.csm_scene_bounds = None;
        self.lm_lights.clear();
        self.frame_lights.clear();
        self.frame_baked_count = 0;
        self.host_lights.clear();
        self.lamp_cache.clear();
        self.lamp_cache_rev = None;
        self.light_grid = LightGrid::default();
        self.light_rank.clear();
        self.light_sel.clear();
        self.light_block_scratch.fill(0.0);
        self.light_cell_memory.clear();
        self.char_ground.clear();
        self.model_ground.clear();
        self.world_attachment_ground.clear();
        self.sdf_instances.clear();
        self.lm_kick_key = None;

        // Keep the counter monotonic even though all its consumers above
        // were cleared; this prevents a future cache from reintroducing the
        // same cross-realm numeric-alias bug.
        self.models_rev = self.models_rev.wrapping_add(1);
        self.bake.enter_realm();
    }

    /// How this device projects the world. Changing it is free — the stage
    /// is a draw-list uniform, so nothing cached needs rebuilding, and the
    /// simulation never learns it happened.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// How many casters may use the projected-silhouette tier. Lower it on
    /// mobile/standalone XR; see apps/sandbox/BUDGETS.md.
    pub fn shadow_budget(&self) -> usize {
        self.shadow_budget
    }

    pub fn set_shadow_budget(&mut self, casters: usize) {
        self.shadow_budget = casters;
    }

    /// Feed the governor one frame's cost, in milliseconds. Returns true on
    /// the frames where the quality level actually moved, so a host can log
    /// or surface it without polling.
    ///
    /// This is OPT-IN: a host that never calls it never gets cut, which is
    /// the right default for a tool that removes scenery. Pass the platform's
    /// real frame time — an XR runtime hands you one, and it is the number
    /// that matters because the Quest is fill-bound, not CPU-bound.
    ///
    /// One trap worth naming: do not pass a vsync-locked frame-to-frame
    /// interval. That signal is quantised to the refresh rate, so it reads
    /// ~16.6ms whether the frame took 3ms or 16ms of real work, and a
    /// governor targeting 80% of budget would cut forever without ever
    /// seeing an improvement. If a real measurement is unavailable, leave
    /// this uncalled rather than feeding it a quantised one.
    pub fn report_frame_ms(&mut self, ms: f32) -> bool {
        self.thermometer.frame(ms)
    }

    /// Tell the governor what this device's display budget is. Call on
    /// startup and whenever the refresh changes — a Quest at 72Hz and the
    /// same Quest at 120Hz want budgets nearly twice apart.
    pub fn set_refresh_hz(&mut self, hz: f32) {
        self.thermometer.set_refresh_hz(hz);
    }

    /// The current cuts. The renderer applies `particle_scale`,
    /// `shadow_caster_scale` and `projected_shadows` itself; the remaining
    /// dials (`decor_distance_scale`, `foliage_scale`, `draw_distance_scale`)
    /// describe scenery only the world builder can identify, so a host that
    /// places decoration should read them when deciding what to emit.
    pub fn quality(&self) -> Quality {
        self.thermometer.quality()
    }

    /// 0 = everything on. Higher = leaner. Pair with [`Self::quality_reason`]
    /// when showing this to a player, so a suddenly emptier world reads as a
    /// deliberate trade rather than a bug.
    pub fn quality_level(&self) -> usize {
        self.thermometer.level()
    }

    /// One line describing what the current level gave up.
    pub fn quality_reason(&self) -> &'static str {
        self.thermometer.reason()
    }

    /// Measured p90 frame time, once enough frames have been seen. `None`
    /// while the window is still filling.
    pub fn frame_p90_ms(&self) -> Option<f32> {
        self.thermometer.p90_ms()
    }

    /// Hand this frame's particles to the renderer. They join the alpha
    /// batch, so any number of them still costs zero extra draw calls.
    pub fn set_fireworks(&mut self, instances: Vec<crate::firework::FireworkInstance>) {
        self.firework_instances = instances;
    }

    /// Build the spark sheet once. Its size is fixed by SPARKS_PER_SHELL, so
    /// it never needs rebuilding the way the terrain mesh does.
    fn ensure_spark_geometry(&mut self, cx: &mut Cx) -> GeometryId {
        if let Some(g) = &self.spark_geometry {
            return g.geometry_id();
        }
        let (indices, vertices) = crate::firework::spark_sheet_vertices();
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        self.spark_geometry = Some(geometry);
        id
    }

    /// The flares' shared billboard: ONE quad in the CubeVertex layout
    /// (geom_pos.xy = corner, geom_uv = 0..1), built once — the per-lamp
    /// data rides in the instance stream.
    fn ensure_flare_geometry(&mut self, cx: &mut Cx) -> GeometryId {
        if let Some(g) = &self.flare_geometry {
            return g.geometry_id();
        }
        let mut vertices: Vec<f32> = Vec::with_capacity(4 * 12);
        for (qx, qy) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            // geom_pos(3), geom_id(1), geom_normal(3), geom_pad(1),
            // geom_uv(2), tail_pad(2) — the spark-sheet layout.
            vertices.extend_from_slice(&[
                qx,
                qy,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                qx + 0.5,
                qy + 0.5,
                0.0,
                0.0,
            ]);
        }
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        self.flare_geometry = Some(geometry);
        id
    }

    /// A pack's prebaked `ao_atlas.png`, uploaded as a texture.
    ///
    /// Looked up beside the pack's models, which is where `tools/ao_bake`
    /// writes it. Greyscale is expanded to RGBA because the texture path is
    /// 32-bit; the shader reads .x.
    fn models_root() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/sandbox/resources/models"
        ))
    }

    fn load_pack_ao(cx: &mut Cx, pack: &str) -> Option<Texture> {
        let png = std::fs::read(Self::models_root().join(pack).join("ao_atlas.png")).ok()?;
        Self::gray_png_texture(cx, &png).map(|(t, _, _)| t)
    }

    /// This model's OWN AO atlas, from `tools/ao_bake`, if one was baked.
    /// The dimensions ride along: they are the shape of the model's chart
    /// layout, which is what sizes its lightmap region.
    fn load_model_ao(cx: &mut Cx, id: &str) -> Option<(Texture, usize, usize)> {
        let png = std::fs::read(Self::models_root().join(format!("{id}.ao.png"))).ok()?;
        Self::gray_png_texture(cx, &png)
    }

    /// Decode ao_bake's greyscale PNG straight into an R8 texture.
    ///
    /// Single channel on the GPU, where the old path inflated grey to RGBA —
    /// 4x the memory for three copies of the same byte, on the device (Quest)
    /// where texture memory is the scarce resource. The shader reads `.x`
    /// either way. The decoder handles exactly what the baker writes — 8-bit
    /// greyscale, filter 0, zlib IDAT — and rejects anything else, which is
    /// how a foreign or corrupt file falls back to "no AO" rather than to a
    /// scrambled one.
    fn gray_png_texture(cx: &mut Cx, png: &[u8]) -> Option<(Texture, usize, usize)> {
        if png.len() < 8 + 25 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
            return None;
        }
        let (mut o, mut w, mut h, mut idat) = (8usize, 0usize, 0usize, Vec::new());
        while o + 8 <= png.len() {
            let len = u32::from_be_bytes(png[o..o + 4].try_into().ok()?) as usize;
            let kind = &png[o + 4..o + 8];
            let body = png.get(o + 8..o + 8 + len)?;
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body.get(..4)?.try_into().ok()?) as usize;
                    h = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?) as usize;
                    // 8-bit greyscale, no interlace: bit depth 8, colour type
                    // 0, compression 0, filter 0, interlace 0.
                    if body.get(8..13)? != [8, 0, 0, 0, 0] {
                        return None;
                    }
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            o += 8 + len + 4;
        }
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return None;
        }
        let raw = makepad_fast_inflate::zlib_decompress_vec(&idat).ok()?;
        if raw.len() != (w + 1) * h {
            return None;
        }
        let mut pixels = Vec::with_capacity(w * h);
        for row in raw.chunks_exact(w + 1) {
            if row[0] != 0 {
                return None;
            }
            pixels.extend_from_slice(&row[1..]);
        }
        Some((
            Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: w,
                    height: h,
                    data: Some(pixels),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            ),
            w,
            h,
        ))
    }

    /// The prebaked mesh for `id`, if `tools/ao_bake` has produced one.
    fn load_aomesh(id: &str) -> Option<StaticModel> {
        let path = Self::models_root().join(format!("{id}.aomesh"));
        StaticModel::from_aomesh(&std::fs::read(path).ok()?)
    }

    pub fn set_particles(&mut self, instances: Vec<ParticleInstance>) {
        self.particle_instances = instances;
    }

    /// Rebuild the static half of Realtime's CSM caster list.
    ///
    /// Models with a lightmap source already become `BakeState` meshes when
    /// their atlas is realized. The list here is the complementary case:
    /// uncharted static geometry, including every material layer. Keeping it
    /// next to placed-scene identity makes a scene upload the registration
    /// boundary instead of turning static architecture into a per-frame
    /// "mover" merely to get a shadow.
    fn rebuild_csm_static_casters(&mut self) {
        self.csm_static_casters.clear();
        for inst in self.placed_models.iter().filter(|instance| !instance.dynamic) {
            let Some((_, model)) = self
                .static_models
                .iter()
                .find(|(id, _)| *id == inst.model)
            else {
                continue;
            };
            if model.lm_source.is_some() {
                continue;
            }
            let (min, max) = crate::lightmap::world_bounds(&inst.transform, (model.min, model.max));
            if !casts_as_caster_only(
                self.model_casts_shadow.get(&inst.model).copied(),
                model.prelit,
                min,
                max,
            ) {
                continue;
            }
            for geometry in std::iter::once(&model.geometry)
                .chain(model.extra_draws.iter().map(|(geometry, ..)| geometry))
            {
                self.csm_static_casters.push(crate::gpu_lightmap::GpuBakeMesh {
                    geometry: geometry.geometry_id(),
                    transform: inst.transform,
                    min,
                    max,
                });
            }
        }
    }

    /// Hand this frame's stock props to the renderer. Draw submission is
    /// batched by model, but list order is still scene identity: lightmap
    /// remaps are indexed by placed slot. Producers should therefore keep a
    /// stable order so a harmless reorder does not request a new bake.
    pub fn set_models(&mut self, instances: Vec<ModelInstance>) {
        let signature = placed_scene_signature(&instances);
        let scene_changed = self.placed_scene_signature != Some(signature);
        // Statics are cached against a key; any meaningful placed-scene
        // change must break it or an equal-length replacement would retain
        // the previous realm's lamps, lightmap remaps, and shadows.
        if scene_changed {
            self.models_rev = self.models_rev.wrapping_add(1);
            // The lightmap remaps are indexed by placed position — a changed
            // list makes them point at the wrong props. Drop them; the
            // models_rev bump re-kicks the bake once the world settles.
            self.lm_remaps.clear();
            self.placed_scene_signature = Some(signature);
            // Per-slot door commands are indexed the same way and go stale
            // the same way. Per-model ones do not — an imported level keeps
            // its doors open across a harmless list rebuild.
            self.model_anim_state.forget_slots();
        }
        self.placed_models = instances;
        if scene_changed {
            self.rebuild_csm_static_casters();
        }
    }

    /// Hand this frame's actor-attached world props to the renderer.
    ///
    /// Attachments receive ordinary world depth, fog, sunlight, AO and
    /// dynamic lights. Their transforms may change every frame without
    /// touching placed-scene identity, lightmap scheduling, CSM caster
    /// capture, shadow-mesh generation, lamp harvesting or replication.
    /// They are always lit as dynamic geometry regardless of the supplied
    /// [`ModelInstance::dynamic`] value; that flag remains meaningful only
    /// in the placed-model lane.
    pub fn set_world_attachments(&mut self, instances: Vec<ModelInstance>) {
        self.world_attachments = instances;
    }

    /// Hand this view's transient presentation meshes to the renderer.
    /// These are visible geometry only: unlike [`Self::set_models`], this
    /// list never participates in scene revision, baking, CSM mover capture,
    /// blob/SDF shadows, collision, or replication.
    pub fn set_view_models(&mut self, instances: Vec<ModelInstance>) {
        self.view_models = instances;
    }

    /// How much CPU the light bake may spend (bake.rs). Lower `ao_rays` and
    /// `max_probes` on standalone XR; see apps/sandbox/BUDGETS.md. Setting
    /// `ao_strength` and `shadow_strength` to 0 turns the bake off visually
    /// while leaving the rest of the pipeline untouched.
    pub fn bake_settings(&self) -> BakeSettings {
        self.bake.settings()
    }

    pub fn set_bake_settings(&mut self, settings: BakeSettings) {
        self.bake.set_settings(settings);
    }

    /// Baked shade multiplier for a moving object, from the probe lattice.
    /// Public so a host can tint something the renderer does not own (the
    /// skinned characters go through this).
    pub fn dynamic_shade(&self, p: Vec3f) -> f32 {
        self.bake.dynamic_shade(p)
    }

    /// Ground height under a caster: the terrain, or the tallest static box
    /// top it stands over. `None` when it is over a hole.
    fn ground_under(world: &GameWorld, e: &Entity) -> Option<f32> {
        let mut ground: Option<f32> = world
            .terrain
            .as_ref()
            .and_then(|t| t.floor_under(e.pos, e.half));
        let feet = e.pos.y - e.half.y;
        for s in world.entities.iter() {
            if s.sensor || s.hidden || !matches!(s.kind, BodyKind::Static | BodyKind::Kinematic) {
                continue;
            }
            let top = s.pos.y + s.half.y;
            if top <= feet + 0.01
                && (e.pos.x - s.pos.x).abs() < s.half.x
                && (e.pos.z - s.pos.z).abs() < s.half.z
            {
                ground = Some(ground.map_or(top, |g: f32| g.max(top)));
            }
        }
        ground
    }

    /// Casters for this frame, tagged with which shadow tier they get.
    ///
    /// Hero rule: rigid bodies (crates, vehicle chassis) and anything
    /// person-sized or larger reads as a real object and earns a projected
    /// silhouette; small scurrying movers get blobs, which is all a blob was
    /// ever good for. Ties are broken by camera distance so the budget
    /// spends itself on what fills the screen.
    fn shadow_casters<'w>(
        world: &'w GameWorld,
        camera_pos: Vec3f,
        budget: usize,
    ) -> Vec<(&'w Entity, f32, bool)> {
        const HERO_HEIGHT: f32 = 0.5;
        let mut heroes: Vec<(&Entity, f32, f32)> = Vec::new();
        let mut small: Vec<(&Entity, f32)> = Vec::new();
        for e in world.entities.iter() {
            if !matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                || e.sensor
                || e.hidden
                || e.attached_to != 0
            {
                continue;
            }
            let Some(ground) = Self::ground_under(world, e) else {
                continue;
            };
            let hero = e.kind == BodyKind::Rigid || e.half.y * e.scale.y * 2.0 >= HERO_HEIGHT;
            if hero {
                let d = e.pos - camera_pos;
                heroes.push((e, ground, d.length_squared()));
            } else {
                small.push((e, ground));
            }
        }
        heroes.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut out: Vec<(&Entity, f32, bool)> = Vec::with_capacity(heroes.len() + small.len());
        for (i, (e, ground, _)) in heroes.into_iter().enumerate() {
            out.push((e, ground, i < budget));
        }
        out.extend(small.into_iter().map(|(e, ground)| (e, ground, false)));
        out
    }

    /// Ground-plane extent of the world's content, in world units: the
    /// centre and half-width of everything the diorama's shadow should
    /// catch. `None` when there is nothing to stand on.
    fn world_footprint(world: &GameWorld) -> Option<(Vec3f, f32)> {
        let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
        let mut any = false;
        for e in world.entities.iter() {
            let (p, h) = (e.pos, e.half);
            min.x = min.x.min(p.x - h.x);
            min.y = min.y.min(p.y - h.y);
            min.z = min.z.min(p.z - h.z);
            max.x = max.x.max(p.x + h.x);
            max.z = max.z.max(p.z + h.z);
            any = true;
        }
        if let Some(t) = world.terrain.as_ref() {
            let half = t.cell_size * (t.cells.saturating_sub(1)) as f32 * 0.5;
            let c = t.origin + half;
            min.x = min.x.min(c - half);
            min.z = min.z.min(c - half);
            max.x = max.x.max(c + half);
            max.z = max.z.max(c + half);
            any = true;
        }
        if !any {
            return None;
        }
        let center = vec3f((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
        let radius = ((max.x - min.x).max(max.z - min.z) * 0.5).max(0.5);
        Some((center, radius))
    }

    /// Rebuild the terrain GPU tiles when the world's terrain revision moved.
    /// Godot-style triangles (terrain_tile_data), regrouped into CHUNK_SIZE
    /// tiles so an offscreen stretch of hills skips its draw item — the same
    /// primitives as the old single mesh, just delivered in pieces.
    fn ensure_terrain_tiles(
        &mut self,
        cx: &mut Cx,
        terrain: &Terrain,
        materials: Option<&TerrainMaterials>,
    ) {
        if !self.terrain_tiles.is_empty() && self.terrain_revision == terrain.revision {
            return;
        }
        self.terrain_tiles.clear();
        let n = terrain.cells;
        let cells_per_tile = ((CHUNK_SIZE / terrain.cell_size.max(1.0e-6)) as usize).max(1);
        let mut gz0 = 0;
        while gz0 < n - 1 {
            let gz1 = (gz0 + cells_per_tile).min(n - 1);
            let mut gx0 = 0;
            while gx0 < n - 1 {
                let gx1 = (gx0 + cells_per_tile).min(n - 1);
                let (vertices, indices, min, max) =
                    terrain_tile_data(terrain, materials, gx0, gx1, gz0, gz1);
                if !indices.is_empty() {
                    let geometry = Geometry::new(cx);
                    geometry.update(cx, indices, vertices);
                    self.terrain_tiles.push(TerrainTile { min, max, geometry });
                }
                gx0 = gx1;
            }
            gz0 = gz1;
        }
        self.terrain_revision = terrain.revision;
    }

    /// Rebuild the water sheets when the world's water revision moved
    /// (volume added, wave added by `game.surf_spot`, re-eval).
    fn ensure_water_tiles(&mut self, cx: &mut Cx, water: Option<&WaterState>) {
        let rev = water.map(|w| w.rev);
        if rev == self.water_rev {
            return;
        }
        self.water_tiles.clear();
        if let Some(water) = water {
            for volume in &water.volumes {
                let (vertices, indices, min, max) = water_sheet_data(volume);
                if indices.is_empty() {
                    continue;
                }
                let geometry = Geometry::new(cx);
                geometry.update(cx, indices, vertices);
                let (waves_a, waves_b) = pack_wave_uniforms(volume);
                self.water_tiles.push(WaterTile {
                    min,
                    max,
                    geometry,
                    waves_a,
                    waves_b,
                });
            }
        }
        self.water_rev = rev;
    }

    /// Mirror the voxel field's chunk meshes into GPU geometries: a merge
    /// over two sorted sequences, re-uploading only chunks whose mesh
    /// revision moved (a dig re-uploads its own chunks, nothing else).
    fn ensure_voxel_tiles(&mut self, cx: &mut Cx, voxel: Option<&VoxelField>) {
        let empty = std::collections::BTreeMap::new();
        let meshes = voxel.map_or(&empty, |v| &v.meshes);
        if self.voxel_tiles.is_empty() && meshes.is_empty() {
            return;
        }
        let mut old: std::collections::BTreeMap<ChunkKey, VoxelTile> = std::mem::take(&mut self.voxel_tiles)
            .into_iter()
            .map(|t| (t.key, t))
            .collect();
        let mut out = Vec::with_capacity(meshes.len());
        for (key, mesh) in meshes {
            match old.remove(key) {
                Some(tile) if tile.rev == mesh.rev => out.push(tile),
                _ => {
                    let geometry = Geometry::new(cx);
                    geometry.update(cx, mesh.indices.clone(), mesh.verts.clone());
                    out.push(VoxelTile {
                        key: *key,
                        rev: mesh.rev,
                        min: mesh.min,
                        max: mesh.max,
                        geometry,
                    });
                }
            }
        }
        // Whatever is left in `old` lost its chunk; the Geometry drops.
        self.voxel_tiles = out;
    }

    /// Unit geometry for a shape, built once and shared by every instance
    /// (index = Shape::index()). All shapes span [-0.5, 0.5] so `cube_size`
    /// scales them exactly like the built-in cube.
    fn ensure_shape_geometry(&mut self, cx: &mut Cx, shape: Shape) -> GeometryId {
        let slot = &mut self.shape_geometries[shape.index()];
        if let Some(geometry) = slot {
            return geometry.geometry_id();
        }
        let (vertices, indices) = shape_geometry_data(shape);
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        *slot = Some(geometry);
        id
    }

    /// Rotation part of an entity's transform. Rigids carry a full box3d
    /// orientation quat (M1a); everything else rotates by visual yaw exactly
    /// as before. Column-major, same layout as Mat4f::rotation.
    pub fn rigid_transform(e: &Entity) -> Mat4f {
        let mut m = Self::entity_rotation(e);
        m.v[12] = e.pos.x;
        m.v[13] = e.pos.y;
        m.v[14] = e.pos.z;
        m
    }

    fn entity_rotation(e: &Entity) -> Mat4f {
        // Hitscan tracers are replicated sensor entities, so their velocity
        // reaches every peer through the ordinary volatile entity state. Use
        // that shared direction as local +Z: a pitched shot becomes a thin
        // 3D streak along the exact gameplay ray rather than a yaw-only box.
        if e.tag == "tracer" {
            let speed_sq = e.vel.length_squared();
            if speed_sq > 1.0e-8 {
                let f = e.vel * (1.0 / speed_sq.sqrt());
                let up_hint = if f.y.abs() > 0.99 {
                    vec3f(1.0, 0.0, 0.0)
                } else {
                    vec3f(0.0, 1.0, 0.0)
                };
                let r = Vec3f::cross(up_hint, f).normalize();
                let u = Vec3f::cross(f, r);
                let mut m = Mat4f::identity();
                m.v[0] = r.x;
                m.v[1] = r.y;
                m.v[2] = r.z;
                m.v[4] = u.x;
                m.v[5] = u.y;
                m.v[6] = u.z;
                m.v[8] = f.x;
                m.v[9] = f.y;
                m.v[10] = f.z;
                return m;
            }
        }
        if e.kind == BodyKind::Rigid {
            let (x, y, z, w) = (e.orient.x, e.orient.y, e.orient.z, e.orient.w);
            let mut m = Mat4f::identity();
            m.v[0] = 1.0 - 2.0 * (y * y + z * z);
            m.v[1] = 2.0 * (x * y + w * z);
            m.v[2] = 2.0 * (x * z - w * y);
            m.v[4] = 2.0 * (x * y - w * z);
            m.v[5] = 1.0 - 2.0 * (x * x + z * z);
            m.v[6] = 2.0 * (y * z + w * x);
            m.v[8] = 2.0 * (x * z + w * y);
            m.v[9] = 2.0 * (y * z - w * x);
            m.v[10] = 1.0 - 2.0 * (x * x + y * y);
            m
        } else {
            Mat4f::rotation(vec3f(0.0, e.yaw, 0.0))
        }
    }

    /// PERF: pack one instance in the exact slice layout `DrawCube::draw`
    /// emits (DrawVars::as_slice covers the trailing glow/fog instance
    /// fields), so slab content and immediate draws are indistinguishable.
    /// Instances land in the world-grid chunk under their centre; the
    /// chunk's content bounds grow by the rotation-invariant half-diagonal,
    /// so any orientation of the box stays inside them.
    fn pack_cube_instance(
        &mut self,
        draws: &mut SceneDraws,
        alpha: bool,
        out_index: usize,
        transform: Mat4f,
        size: Vec3f,
        color: Vec4f,
        glow: f32,
    ) {
        let center = vec3f(transform.v[12], transform.v[13], transform.v[14]);
        let r = size.length() * 0.5;
        let cell = chunk_cell(center.x, center.z);
        let at = match self.static_chunks.iter().position(|c| c.cell == cell) {
            Some(at) => at,
            None => {
                self.static_chunks.push(SlabChunk {
                    cell,
                    min: vec3f(f32::MAX, f32::MAX, f32::MAX),
                    max: vec3f(f32::MIN, f32::MIN, f32::MIN),
                    slab: Default::default(),
                    slab_alpha: Default::default(),
                });
                self.static_chunks.len() - 1
            }
        };
        let chunk = &mut self.static_chunks[at];
        chunk.min = vec3f(
            chunk.min.x.min(center.x - r),
            chunk.min.y.min(center.y - r),
            chunk.min.z.min(center.z - r),
        );
        chunk.max = vec3f(
            chunk.max.x.max(center.x + r),
            chunk.max.y.max(center.y + r),
            chunk.max.z.max(center.z + r),
        );
        if alpha {
            draws.alpha.cube.cube.transform = transform;
            draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.alpha.cube.cube.cube_size = size;
            draws.alpha.cube.cube.color = color;
            draws.alpha.cube.cube.depth_clip = 1.0;
            draws.alpha.cube.glow = glow;
            let slice = draws.alpha.cube.cube.draw_vars.as_slice();
            chunk.slab_alpha[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        } else {
            draws.cube.cube.transform = transform;
            draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.cube.cube.cube_size = size;
            draws.cube.cube.color = color;
            draws.cube.cube.depth_clip = 1.0;
            draws.cube.glow = glow;
            let slice = draws.cube.cube.draw_vars.as_slice();
            chunk.slab[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        }
    }

    /// PERF: rebuild the packed static instance slabs. Only runs when
    /// `world.render_rev` moved — the world bumps it on every mutation that
    /// changes what static content looks like (see mark_render_dirty).
    fn rebuild_static_slabs(&mut self, draws: &mut SceneDraws, world: &GameWorld) {
        // Chunks are dropped, not cleared: a vacated cell must not linger as
        // an empty entry the per-frame loops keep testing. Rebuilds run at
        // edit cadence, so the reallocation is not a per-frame cost.
        self.static_chunks.clear();
        self.slab_instance_count = 0;
        // Static entities (opaque and sensor/alpha).
        for e in world
            .entities
            .iter()
            .filter(|e| e.kind == BodyKind::Static && !e.hidden)
        {
            let mut transform = Mat4f::rotation(vec3f(0.0, e.yaw, 0.0));
            transform.v[12] = e.pos.x;
            transform.v[13] = e.pos.y;
            transform.v[14] = e.pos.z;
            let size = vec3(
                e.half.x * 2.0 * e.scale.x,
                e.half.y * 2.0 * e.scale.y,
                e.half.z * 2.0 * e.scale.z,
            );
            let mut color = e.color;
            if e.sensor && color.w >= 0.99 {
                color.w = 0.35;
            }
            // Baked occlusion rides in the colour we were already sending —
            // no extra instance field, no shader work, no draw call.
            color = shade_color(color, self.bake.static_shade(e.id), e.glow);
            self.pack_cube_instance(draws, e.sensor, e.shape.index(), transform, size, color, e.glow);
        }
        // Settled parts of static owners.
        for p in world.parts.iter().filter(|p| !p.anim_active) {
            // Entity ids are spawn-ordered, so the list stays sorted; the
            // shared sim helper owns (and debug-asserts) that invariant.
            let Some(owner) = entity_index_sorted(&world.entities, p.owner)
                .map(|i| &world.entities[i])
                .filter(|e| e.kind == BodyKind::Static)
            else {
                continue;
            };
            let mut owner_frame = Mat4f::rotation(vec3f(0.0, owner.yaw, 0.0));
            owner_frame.v[12] = owner.pos.x;
            owner_frame.v[13] = owner.pos.y;
            owner_frame.v[14] = owner.pos.z;
            let mut local = Mat4f::rotation(p.rot);
            local.v[12] = p.offset.x * owner.scale.x;
            local.v[13] = p.offset.y * owner.scale.y;
            local.v[14] = p.offset.z * owner.scale.z;
            let transform = Mat4f::mul(&owner_frame, &local);
            let size = vec3(
                p.half.x * 2.0 * owner.scale.x,
                p.half.y * 2.0 * owner.scale.y,
                p.half.z * 2.0 * owner.scale.z,
            );
            // A part inherits its owner's bake — it is bolted to it.
            let color = shade_color(p.color, self.bake.static_shade(owner.id), p.glow);
            self.pack_cube_instance(draws, false, p.shape.index(), transform, size, color, p.glow);
        }
    }

    /// Refresh the receiver/occluder box sets after the world settles. The
    /// receiver boxes place every dynamic shadow (SDF quad landing heights,
    /// blob drapes, character ground samples); the occluder boxes feed the
    /// GPU lightmap's depth passes.
    ///
    /// This is all that remains of the old static shadow-mesh rebuild: sun
    /// shadows for statics live in the baked LIGHTMAP, ambient grounding in
    /// each model's own AO atlas — the merged silhouette geometry this used
    /// to build (30ms per world settle) drew nothing but artifacts on top
    /// of them and was deleted.
    fn refresh_shadow_receivers(&mut self, world: &GameWorld) {
        // Tops that can catch a draped shadow: visible, solid static boxes —
        // road slabs, platforms, crate lids. TALL hidden colliders stay
        // excluded (their tops are the invisible lids the roof surfaces
        // replaced; draping onto those floats shadows mid-air), but THIN
        // hidden slabs are floor stand-ins for visible model floors — a
        // generated dungeon's tiles. Without them shadows draped onto the
        // terrain UNDER the floors and surfaced coplanar with the visible
        // ground: the arena's floor-wide z-fighting sheets.
        self.occluder_boxes = world
            .entities
            .iter()
            .filter(|e| {
                e.kind == BodyKind::Static
                    && !e.sensor
                    && !e.hidden
                    && e.shape == Shape::Box
                    && e.color.w >= 0.99
            })
            .map(|e| {
                let h = vec3f(
                    e.half.x * e.scale.x,
                    e.half.y * e.scale.y,
                    e.half.z * e.scale.z,
                );
                (
                    vec3f(e.pos.x - h.x, e.pos.y - h.y, e.pos.z - h.z),
                    vec3f(e.pos.x + h.x, e.pos.y + h.y, e.pos.z + h.z),
                )
            })
            .collect();
        self.receiver_boxes = world
            .entities
            .iter()
            .filter(|e| {
                // Receivers: what a draped shadow may land on. Visible solid
                // statics, PLUS the mesh voxel boxes that are genuinely
                // FLOORS — wide flat slabs (a generated dungeon's walkable
                // surface lives in the mesh at its true height; the old
                // mask stand-ins sat 10cm below the drawn floor and every
                // draped shadow was buried under it). Curbs, decks and
                // braces stay excluded: wide+flat only.
                let h = (
                    e.half.x * e.scale.x,
                    e.half.y * e.scale.y,
                    e.half.z * e.scale.z,
                );
                let flat_slab = h.1 <= 0.25 && h.0 >= 0.8 && h.2 >= 0.8;
                e.kind == BodyKind::Static
                    && !e.sensor
                    && e.shape == Shape::Box
                    && (!e.hidden || flat_slab)
            })
            .map(|e| {
                let h = vec3f(
                    e.half.x * e.scale.x,
                    e.half.y * e.scale.y,
                    e.half.z * e.scale.z,
                );
                (
                    vec3f(e.pos.x - h.x, e.pos.y - h.y, e.pos.z - h.z),
                    vec3f(e.pos.x + h.x, e.pos.y + h.y, e.pos.z + h.z),
                )
            })
            .collect();
    }

    /// Load a stock prop onto the GPU under `id`, idempotent. `png` is the
    /// pack atlas, shared by every model in that pack.
    pub fn load_model(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
    ) -> Result<usize, String> {
        self.load_model_with_ao(cx, id, glb, png, None, None)
    }

    /// [`load_model_with_ao`] plus the offline `.shadowsdf` bytes, for a
    /// model whose whole bake travelled with it (an asset-store manifest:
    /// render GLB + AoMesh + AoTexture + ShadowSdf roles). The SDF is kept
    /// per model id and resolved against the sun like the disk sidecar.
    pub fn load_model_with_bake(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
        aomesh: Option<&[u8]>,
        ao_png: Option<&[u8]>,
        shadow_sdf: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(sdf) = shadow_sdf {
            if !self.model_sdf_bytes.contains_key(id) {
                self.model_sdf_bytes.insert(id.to_string(), sdf.to_vec());
            }
        }
        self.load_model_with_ao(cx, id, glb, png, aomesh, ao_png)
    }

    /// Same as [`load_model`], but the caller may hand the offline AO pair
    /// (`.aomesh` + `.ao.png` bytes) instead of relying on `models_root`.
    /// Import thumbnails use this so a Kenney kit looks like the baked game
    /// mesh, not a raw unlit GLB.
    pub fn load_model_with_ao(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
        aomesh: Option<&[u8]>,
        ao_png: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            return Ok(self.static_models[at].1.triangles);
        }
        // An `.aomesh` sidecar carries the bake's mesh but never the atlas
        // ("by URI" — the pack's colormap beside a checkout GLB). A GLB with
        // its atlas EMBEDDED (every pack import, every generated model) has
        // no such file: without this the sidecar lane drew the model white
        // under its AO. The embedded base colour is the atlas, exactly as
        // parse_glb would have read it.
        let embedded_png = if png.is_none() && aomesh.is_some() {
            crate::model::embedded_base_color_png(glb)
        } else {
            None
        };
        let png = png.or(embedded_png.as_deref());
        // Prefer the prebaked sidecar. It carries the exact mesh the atlas was
        // baked against — including the ao_uv lane, which the runtime has no
        // way to reconstruct — so loading it is what makes the AO texture mean
        // anything. Absent or stale, the plain glb still renders, just unlit by
        // AO, which is the correct outcome for an unbaked library.
        let baked = aomesh
            .and_then(StaticModel::from_aomesh)
            .or_else(|| Self::load_aomesh(id));
        let model = match baked {
            Some(mut body) if glb.windows(b"vehicle_wheel".len()).any(|w| w == b"vehicle_wheel") => {
                // The AO sidecar intentionally serializes only the flattened
                // body stream. Driven parts stay in the original GLB because
                // their per-frame pose makes baked AO invalid. Reattach those
                // definitions without giving up the body's baked chart.
                let mut source = StaticModel::parse_glb(glb)?;
                body.driven_parts = std::mem::take(&mut source.driven_parts);
                body
            }
            Some(body) => body,
            None => StaticModel::parse_glb(glb)?,
        };
        self.load_model_parsed(cx, id, model, png, ao_png)
    }

    /// The GPU half of [`load_model_with_ao`], over a model somebody ELSE
    /// parsed.
    ///
    /// Parsing is the expensive half and it needs no `Cx`: a Doom E1M1 GLB
    /// measured 29.6ms of parse against 3.4ms of upload, and on the UI
    /// thread that is two dropped frames every time a world is cued. A
    /// caller with a worker thread (`apps/vj`) runs
    /// [`StaticModel::parse_glb`] there and hands the result here, so only
    /// the buffer/texture creation — which genuinely cannot happen off the
    /// UI thread — is paid in the frame.
    ///
    /// Identical in every other respect to [`load_model_with_ao`], including
    /// the resident-id early return, so the two paths cannot drift.
    pub fn load_model_parsed(
        &mut self,
        cx: &mut Cx,
        id: &str,
        mut model: StaticModel,
        png: Option<&[u8]>,
        ao_png: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            return Ok(self.static_models[at].1.triangles);
        }
        // NO BAKE AT LOAD. AO is an offline product (tools/ao_bake); the game
        // loads it or goes without. Baking here cost every launch for an
        // answer that never changes.
        let pack: String = id.split('/').take(2).collect::<Vec<_>>().join("/");
        // Taken out first: the parts are uploaded on their own below, and
        // everything after this point sees the model exactly as it did
        // before doors and skies existed.
        let anim_defs = std::mem::take(&mut model.anim_parts);
        let driven_defs = std::mem::take(&mut model.driven_parts);
        let sky_def = model.sky.take();
        let triangles = model.triangle_count();
        let (min, max) = (model.min, model.max);
        // Triangle-derived voxel boxes are the collider truth: measured
        // against the real kits, per-primitive parts are usually ONE merged
        // blob (see model.rs real_asset_tests). Primitive curation only as
        // the degenerate-mesh fallback — and always as the OCCLUDER set,
        // where few clean boxes beat many exact ones.
        let occluder_parts = model.collider_parts();
        let collider_parts = {
            let v = model.voxel_collider_boxes();
            if v.is_empty() { occluder_parts.clone() } else { v }
        };
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        // The light baker's raycaster triangles, captured BEFORE the GPU
        // upload consumes the packed stream. Only `lm_source` holders use
        // them, but whether this model holds one is not known until its AO
        // atlas resolves below.
        let lm_positions: Vec<Vec3f> = (0..model.vertices.len() / stride)
            .map(|i| {
                vec3f(
                    model.vertices[i * stride],
                    model.vertices[i * stride + 1],
                    model.vertices[i * stride + 2],
                )
            })
            .collect();
        let lm_indices = model.indices.clone();
        // The light baker's per-vertex lanes, pulled out of the packed stream
        // BEFORE the GPU upload consumes it. ao_uv is the chart layout the
        // lightmap region reuses; the tint bytes stand in for albedo in the
        // bounce pass.
        let lm_ao_uv: Vec<[f32; 2]> = (0..model.vertices.len() / stride)
            .map(|i| crate::model::unpack_ao_uv(model.vertices[i * stride + 6]))
            .collect();
        let lm_albedo: Vec<Vec3f> = (0..model.vertices.len() / stride)
            .map(|i| {
                // pack_unorm8x4 order: r | g<<8 | b<<16 | ao<<24.
                let bits = model.vertices[i * stride + 5].to_bits();
                vec3f(
                    (bits & 255) as f32 / 255.0,
                    ((bits >> 8) & 255) as f32 / 255.0,
                    ((bits >> 16) & 255) as f32 / 255.0,
                )
            })
            .collect();
        // The bake variant's vertex stream, captured BEFORE the render
        // upload consumes the model: per-triangle flat winding normals in
        // the nrm lane (see `LoadedModel::bake_geometry`).
        let bake_stream = {
            let mut verts = Vec::with_capacity(model.indices.len() * stride);
            let mut idx = Vec::with_capacity(model.indices.len());
            for tri in model.indices.chunks_exact(3) {
                let p = |i: u32| {
                    let o = i as usize * stride;
                    vec3f(
                        model.vertices[o],
                        model.vertices[o + 1],
                        model.vertices[o + 2],
                    )
                };
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
                let n = Vec3f::cross(b - a, c - a);
                let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                let n = if l > 1.0e-12 {
                    vec3f(n.x / l, n.y / l, n.z / l)
                } else {
                    vec3f(0.0, 1.0, 0.0)
                };
                let (ox, oy) = crate::skin::oct_encode(n);
                let packed_n = makepad_draw::pack_pair_f16(ox, oy);
                for i in tri {
                    let o = *i as usize * stride;
                    idx.push(verts.len() as u32 / stride as u32);
                    verts.extend_from_slice(&model.vertices[o..o + stride]);
                    let at = verts.len() - stride + 3;
                    verts[at] = packed_n;
                }
            }
            (idx, verts)
        };
        let geometry = Geometry::new(cx);
        let multi = model.draw_layers.len() > 1;
        if multi {
            geometry.update(
                cx,
                model.draw_layers[0].indices.clone(),
                model.draw_layers[0].vertices.clone(),
            );
        } else {
            geometry.update(cx, model.indices, model.vertices);
        }
        // Two Kenney conventions, one path. A pack that UV-maps into an atlas
        // needs that atlas — missing it would render white, which reads as a
        // broken model, so it is an error. A pack that carries no texture at
        // all (nature-kit: flat per-material colours) is correct with a white
        // 1x1, because model.rs baked those colours into the vertex tint and
        // the shader multiplies the two. Self-contained GLBs (generated
        // meshes with a baked atlas) embed their base color in the BIN chunk
        // — used when the caller supplies no atlas.
        //
        // Multi-tile worlds ignore the caller's single PNG override: that is
        // always image 0, and using it for every layer is what made the walk
        // view and the GPU thumbnail smear one tile across the map.
        let main_png = if multi {
            model.draw_layers[0].texture_png.as_deref()
        } else {
            png.or(model.texture_png.as_deref())
        };
        let texture = match (main_png, model.texture_uri.as_deref()) {
            (Some(bytes), _) => ImageBuffer::from_png(bytes)
                .map_err(|e| format!("{id}: atlas decode failed: {e:?}"))?
                .into_new_mip_repeat_texture(cx),
            (None, None) => {
                let mut white = ImageBuffer::default();
                white.width = 1;
                white.height = 1;
                white.data = vec![0xFFFF_FFFF];
                white.into_new_texture(cx)
            }
            (None, Some(uri)) => {
                return Err(format!(
                    "{id}: atlas {uri} missing — run apps/sandbox/download_assets.sh"
                ))
            }
        };
        // Which shader this model draws with, decided here and never again.
        // A prelit map keeps the diffuse lane unconditionally: its COLOR_0 IS
        // the light, and laying a sun-driven highlight over a baked Quake
        // lightmap would put a moving specular on a wall the file already
        // finished lighting.
        let wants_pbr = !model.prelit
            && (model.pbr.is_shiny() || model.draw_layers.iter().any(|l| l.pbr.is_shiny()));
        let extra_draws = if multi {
            let mut extra = Vec::with_capacity(model.draw_layers.len() - 1);
            for (i, layer) in model.draw_layers.iter().enumerate().skip(1) {
                if layer.indices.len() < 3 {
                    continue;
                }
                let mat = self.upload_material(cx, &layer.pbr);
                let tex = match layer.texture_png.as_deref() {
                    Some(bytes) => ImageBuffer::from_png(bytes)
                        .map_err(|e| format!("{id}: layer {i} atlas decode failed: {e:?}"))?
                        .into_new_mip_repeat_texture(cx),
                    None => {
                        let mut white = ImageBuffer::default();
                        white.width = 1;
                        white.height = 1;
                        white.data = vec![0xFFFF_FFFF];
                        white.into_new_texture(cx)
                    }
                };
                let (det, dscale) = self.upload_detail(
                    cx,
                    layer.detail_png.as_deref(),
                    layer.detail_scale,
                );
                let g = Geometry::new(cx);
                g.update(cx, layer.indices.clone(), layer.vertices.clone());
                extra.push((g, tex, det, dscale, mat));
            }
            extra
        } else {
            Vec::new()
        };
        let (main_detail, main_detail_scale) = if multi {
            let layer0 = &model.draw_layers[0];
            self.upload_detail(cx, layer0.detail_png.as_deref(), layer0.detail_scale)
        } else {
            self.upload_detail(cx, model.detail_png.as_deref(), model.detail_scale)
        };
        let main_material = if multi {
            self.upload_material(cx, &model.draw_layers[0].pbr)
        } else {
            self.upload_material(cx, &model.pbr)
        };
        // Anim parts (doors, lifts). Their layers go through the same upload
        // as the model's own, but a part usually re-uses a texture the level
        // already has — a door is skinned with one of the level's tiles — so
        // an identical PNG binds the SAME texture rather than a second copy.
        // That keeps a part in its parent's batch instead of splitting it.
        let anim_parts: Vec<LoadedAnimPart> = {
            let mut out = Vec::with_capacity(anim_defs.len());
            for def in anim_defs {
                let mut draws = Vec::with_capacity(def.layers.len());
                for (li, layer) in def.layers.iter().enumerate() {
                    if layer.indices.len() < 3 {
                        continue;
                    }
                    let resident = if multi {
                        model
                            .draw_layers
                            .iter()
                            .position(|l| l.texture_png == layer.texture_png)
                            .and_then(|k| match k {
                                0 => Some(texture.clone()),
                                k => extra_draws.get(k - 1).map(|(_, t, _, _, _)| t.clone()),
                            })
                    } else if main_png.is_some() && main_png == layer.texture_png.as_deref() {
                        Some(texture.clone())
                    } else {
                        None
                    };
                    let tex = match resident {
                        Some(t) => t,
                        None => match layer.texture_png.as_deref() {
                            Some(bytes) => ImageBuffer::from_png(bytes)
                                .map_err(|e| {
                                    format!(
                                        "{id}: part {} layer {li} atlas decode failed: {e:?}",
                                        def.name
                                    )
                                })?
                                .into_new_mip_repeat_texture(cx),
                            None => {
                                let mut white = ImageBuffer::default();
                                white.width = 1;
                                white.height = 1;
                                white.data = vec![0xFFFF_FFFF];
                                white.into_new_texture(cx)
                            }
                        },
                    };
                    let (det, dscale) =
                        self.upload_detail(cx, layer.detail_png.as_deref(), layer.detail_scale);
                    let g = Geometry::new(cx);
                    g.update(cx, layer.indices.clone(), layer.vertices.clone());
                    draws.push((g, tex, det, dscale));
                }
                if draws.is_empty() {
                    continue;
                }
                let collider = def.collider_boxes();
                out.push(LoadedAnimPart { def, draws, collider });
            }
            out
        };
        let driven_parts: Vec<LoadedDrivenPart> = {
            let mut out = Vec::with_capacity(driven_defs.len());
            for def in driven_defs {
                let mut draws = Vec::with_capacity(def.layers.len());
                for (li, layer) in def.layers.iter().enumerate() {
                    if layer.indices.len() < 3 {
                        continue;
                    }
                    let resident = if multi {
                        model
                            .draw_layers
                            .iter()
                            .position(|l| l.texture_png == layer.texture_png)
                            .and_then(|k| match k {
                                0 => Some(texture.clone()),
                                k => extra_draws.get(k - 1).map(|(_, t, _, _, _)| t.clone()),
                            })
                    } else if main_png.is_some() && main_png == layer.texture_png.as_deref() {
                        Some(texture.clone())
                    } else {
                        None
                    };
                    let tex = match resident {
                        Some(t) => t,
                        None => match layer.texture_png.as_deref() {
                            Some(bytes) => ImageBuffer::from_png(bytes)
                                .map_err(|e| {
                                    format!(
                                        "{id}: driven part {} layer {li} atlas decode failed: {e:?}",
                                        def.connection
                                    )
                                })?
                                .into_new_mip_repeat_texture(cx),
                            None => {
                                let mut white = ImageBuffer::default();
                                white.width = 1;
                                white.height = 1;
                                white.data = vec![0xFFFF_FFFF];
                                white.into_new_texture(cx)
                            }
                        },
                    };
                    let (det, dscale) =
                        self.upload_detail(cx, layer.detail_png.as_deref(), layer.detail_scale);
                    let g = Geometry::new(cx);
                    g.update(cx, layer.indices.clone(), layer.vertices.clone());
                    draws.push((g, tex, det, dscale));
                }
                if !draws.is_empty() {
                    out.push(LoadedDrivenPart { def, draws });
                }
            }
            out
        };
        // The map's sky faces: one geometry, up to two layer images. A
        // missing or undecodable layer falls back to a 1x1 rather than
        // failing the whole map — a wrong sky is a wrong sky, but no map is
        // better than a wrong sky only in a unit test.
        let sky = match sky_def {
            Some(part) => {
                let stride = crate::model::MODEL_VERTEX_FLOATS;
                let positions: Vec<Vec3f> = (0..part.vertices.len() / stride)
                    .map(|i| {
                        vec3f(
                            part.vertices[i * stride],
                            part.vertices[i * stride + 1],
                            part.vertices[i * stride + 2],
                        )
                    })
                    .collect();
                let indices = part.indices.clone();
                let g = Geometry::new(cx);
                g.update(cx, part.indices.clone(), part.vertices.clone());
                let mips = sky_wants_mips(part.projection);
                let mut layer = |i: usize, fallback: u32| -> Texture {
                    part.images
                        .get(i)
                        .and_then(|png| ImageBuffer::from_png(png).ok())
                        .map(|img| {
                            if mips {
                                img.into_new_mip_repeat_texture(cx)
                            } else {
                                // Explicitly mip-FREE (not `into_new_texture`,
                                // which builds a chain on some backends).
                                Texture::new_with_format(
                                    cx,
                                    TextureFormat::VecBGRAu8_32 {
                                        width: img.width,
                                        height: img.height,
                                        data: Some(img.data),
                                        updated: TextureUpdated::Full,
                                    },
                                )
                            }
                        })
                        .unwrap_or_else(|| {
                            let mut flat = ImageBuffer::default();
                            flat.width = 1;
                            flat.height = 1;
                            flat.data = vec![fallback];
                            flat.into_new_texture(cx)
                        })
                };
                Some(LoadedSky {
                    tex0: layer(0, 0xFFFF_FFFF),
                    // Transparent: the front layer keys the back one through
                    // its alpha, so an absent second layer must add nothing.
                    tex1: layer(1, 0x0000_0000),
                    geometry: g,
                    part,
                    positions,
                    indices,
                })
            }
            None => None,
        };
        // The pack's prebaked atlas, if tools/ao_bake has produced one. No
        // atlas simply means no AO for that pack, not an error: a game must
        // still run against a library that was never baked.
        // A PER-MODEL atlas wins over the pack's when one exists.
        //
        // Sharing one 1024x1024 atlas across a 40-model pack leaves each model
        // about 26k texels — a 162x162 square for a whole house. Measured, that
        // puts window-frame and door trim at a quarter of a texel across, and
        // sub-texel geometry reads its neighbour's occlusion instead of its
        // own. A model baked alone gets the entire texture, which is 40x the
        // density on exactly the small features where the AO was wrong.
        //
        // Costs a texture bind per model rather than per pack, so this is for
        // models that earn it, not a blanket default — absent a per-model file
        // the pack atlas is still used and batching is unchanged.
        let mut model_ao_dims: Option<(usize, usize)> = None;
        let ao_key = match ao_png
            .and_then(|png| Self::gray_png_texture(cx, png))
            .or_else(|| Self::load_model_ao(cx, id))
        {
            Some((t, w, h)) => {
                if !self.ao_textures.iter().any(|(k, _)| k == id) {
                    self.ao_textures.push((id.to_string(), t));
                }
                model_ao_dims = Some((w, h));
                id.to_string()
            }
            None => {
                if !self.ao_textures.iter().any(|(k, _)| *k == pack) {
                    if let Some(t) = Self::load_pack_ao(cx, &pack) {
                        self.ao_textures.push((pack.clone(), t));
                    }
                }
                pack.clone()
            }
        };
        self.model_pack.push((id.to_string(), ao_key));

        // Only a model with its OWN AO layout gets a lightmap source: a
        // pack-shared layout would make every instance's region the size of
        // the whole pack atlas, mostly empty.
        let lm_source = model_ao_dims.map(|(ao_w, ao_h)| {
            std::sync::Arc::new(crate::lightmap::LmMeshSource {
                caster: crate::ao::MeshRaycaster::new(
                    lm_positions.clone(),
                    lm_indices.clone(),
                    min,
                    max,
                ),
                ao_uv: lm_ao_uv,
                albedo: lm_albedo,
                ao_w,
                ao_h,
            })
        });
        // Only models the light baker will see pay for the flat-normal
        // variant (lm_source holders get regions; everything else casts via
        // positions alone, where the render geometry serves).
        let bake_geometry = lm_source.is_some().then(|| {
            let g = Geometry::new(cx);
            g.update(cx, bake_stream.0, bake_stream.1);
            g
        });

        self.static_models.push((
            id.to_string(),
            LoadedModel {
                geometry,
                texture,
                detail: main_detail,
                detail_scale: main_detail_scale,
                extra_draws,
                material: main_material,
                wants_pbr,
                prelit: model.prelit,
                triangles,
                min,
                max,
                collider_parts,
                occluder_parts,
                anim_parts,
                driven_parts,
                sky,
                mesh_positions: lm_positions,
                mesh_indices: lm_indices,
                lm_source,
                bake_geometry,
            },
        ));
        self.rebuild_csm_static_casters();
        Ok(triangles)
    }

    /// Drop a resident model's caches so the NEXT `load_model*` call for
    /// `id` re-parses its GLB and re-reads its bake sidecars instead of
    /// hitting the early-return at the top of `load_model_with_ao`. For a
    /// republished asset-store revision: the caller drops its own RAM byte
    /// memo FIRST (so the reload streams fresh bytes rather than reusing
    /// the stale ones), then this drops the GPU-resident geometry/AO/SDF —
    /// mirroring every per-id side table `load_model_with_ao` writes.
    ///
    /// `placed_models` is untouched: a still-placed instance of `id` simply
    /// stops drawing (its geometry lookup fails) until the caller's next
    /// `load_model*` call for the same id lands, typically the same frame.
    ///
    /// A per-model AO texture (`ao_textures` keyed by `id` itself) is
    /// dropped too; a PACK-shared atlas (keyed by the pack prefix) is left
    /// alone — other resident models from the same pack still bind it.
    ///
    /// Returns whether `id` was actually resident.
    pub fn unload_model(&mut self, id: &str) -> bool {
        let had = if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            self.static_models.remove(at);
            true
        } else {
            false
        };
        // Its doors go with it: a reload re-parses the GLB, and a clock aimed
        // at a state index of the OLD parse has no meaning against the new
        // one.
        let slots: Vec<usize> = self
            .placed_models
            .iter()
            .enumerate()
            .filter(|(_, m)| m.model == id)
            .map(|(i, _)| i)
            .collect();
        self.model_anim_state.forget_model(id, &slots);
        self.model_pack.retain(|(k, _)| k != id);
        self.ao_textures.retain(|(k, _)| k != id);
        self.model_sdf_bytes.remove(id);
        self.model_sdf_tex.remove(id);
        if had {
            // Any placed instance of this id now points at a stale
            // lightmap chart region (a re-baked AO mesh may lay its
            // ao_uv out differently) — force the next scene-identity
            // check to re-kick the bake rather than trusting the old
            // remap, the same way `set_models` treats any other
            // placed-scene-relevant change.
            self.lm_remaps.clear();
            self.placed_scene_signature = None;
            self.models_rev = self.models_rev.wrapping_add(1);
            self.rebuild_csm_static_casters();
        }
        had
    }

    /// Last frame's dynamic shadow-mesh triangles (entity hull drapes +
    /// pre-sidecar blobs) — the budget the unattended debug cycle watches.
    pub fn dynamic_shadow_triangles(&self) -> usize {
        self.last_dynamic_shadow_tris
    }

    /// Explicit static lights for the light baker. Empty (the default)
    /// harvests lamp props automatically at bake time.
    pub fn set_static_lights(&mut self, lights: Vec<crate::lightmap::LmLight>) {
        self.lm_lights = lights;
        // The lamp cache and its selection grid mirror this set.
        self.lamp_cache_rev = None;
    }

    /// Street lights from the placed props: any static model whose id reads
    /// as a lamp gets a warm downlight at its head — the bulb sits near the
    /// top of the model, so the anchor is measured from its bounds rather
    /// than assumed.
    ///
    /// # Photometry comes from the FIXTURE, not from the mesh scale
    ///
    /// Only the bulb's POSITION follows the placement transform: the mesh was
    /// scaled, so the bulb really did move. Strength and reach are solved
    /// from the mount height instead
    /// ([`lamp_photometry`](crate::lightmap::lamp_photometry)) so the pool on
    /// the street is the same size and the same brightness whether the kit
    /// was placed at ×1, ×2 or the road kits' canonical ×8.
    ///
    /// The flat `color: 2.0, radius: 8.0` this replaced looked
    /// scale-independent and was not: the gather's falloff is
    /// `(1 - d/radius)²`, so pinning the reach while the transform lifted the
    /// bulb made delivered brightness a function of mesh scale. A 1.56 m
    /// lantern at ×2 put 0.87 on the ground — MORE than the noon sun's 0.72
    /// direct term — which is the white plaza pool this rewrite fixes; the
    /// same lantern at ×8 lit nothing at all, its bulb hanging outside its
    /// own 8 m reach.
    fn harvest_lamps(&self) -> Vec<crate::lightmap::LmLight> {
        use crate::lightmap::lamp_photometry;
        /// Warm street-lamp tint, normalised so its brightest component is 1
        /// — `lamp_photometry` supplies the strength it is scaled by.
        const TINT: Vec3f = Vec3f { x: 1.0, y: 0.775, z: 0.475 };
        let mut out = Vec::new();
        for inst in &self.placed_models {
            if inst.dynamic {
                continue;
            }
            let name = inst.model.rsplit('/').next().unwrap_or(&inst.model);
            if !(name.contains("lamp") || name.contains("lantern") || name.contains("light")) {
                continue;
            }
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let m = &self.static_models[at].1;
            let mid_x = (m.min.x + m.max.x) * 0.5;
            let mid_z = (m.min.z + m.max.z) * 0.5;
            let head = m.max.y - (m.max.y - m.min.y) * 0.12;
            let at_world = |y: f32| {
                inst.transform
                    .transform_vec4(Vec4f { x: mid_x, y, z: mid_z, w: 1.0 })
                    .to_vec3f()
            };
            let p = at_world(head);
            // The pole's own foot IS the ground it stands on, whatever the
            // terrain does — measured under the same transform, so the mount
            // height is exact for any scale, yaw or tilt.
            let mount = p.y - at_world(m.min.y).y;
            let (radius, strength) = lamp_photometry(mount);
            out.push(crate::lightmap::LmLight {
                pos: p,
                color: TINT * strength,
                radius,
                // A street light is a downlight: full spot kills the glow
                // it was painting on the roof BESIDE its own head.
                dir: vec3f(0.0, -1.0, 0.0),
                spot: 1.0,
            });
        }
        out
    }

    /// THE static light list for this sun: harvested fixtures (or the
    /// host's hand-set lights) with both rails applied, exactly once.
    ///
    /// One entry point on purpose — [`Self::rail_lamp_pools`]'s daylight
    /// scale is a MULTIPLIER, so applying it twice would square it. The
    /// bake and the per-frame analytic list must both come through here, or
    /// a static and a character standing on the same texel disagree about
    /// how bright the lamp above them is.
    fn static_lights_for(&self, sun: &SunLight) -> Vec<crate::lightmap::LmLight> {
        let mut lights = if self.lm_lights.is_empty() {
            self.harvest_lamps()
        } else {
            self.lm_lights.clone()
        };
        Self::rail_lamp_pools(&mut lights, sun);
        lights
    }

    /// The sanity rails on static lights, applied wherever a lamp list is
    /// built so the baked atlas and the analytic per-frame term never
    /// disagree. Two of them, in order:
    ///
    /// 1. **Daylight headroom** — a lamp may only add the light the sky is
    ///    not already delivering
    ///    ([`lamp_daylight_scale`](crate::lightmap::lamp_daylight_scale)).
    ///    This is the rail on the SUM that reaches the screen, and the one
    ///    that stops a 0.30 pool painting a near-white plaza to 1.26 under a
    ///    noon sun.
    /// 2. **Atlas saturation** — no single light may clip the light atlas
    ///    over more than
    ///    [`LM_LAMP_SAT_TEXELS`](crate::lightmap::LM_LAMP_SAT_TEXELS) ground
    ///    texels. `harvest_lamps` sizes its fixtures so this never fires;
    ///    hand-set lights (`set_static_lights`) go through no such solve, and
    ///    one of those must still not be able to paint a plaza white.
    ///
    /// A light's implied mount is what its reach leaves over the pool it is
    /// meant to cover — exact for anything `lamp_photometry` sized.
    fn rail_lamp_pools(lights: &mut [crate::lightmap::LmLight], sun: &SunLight) {
        use crate::lightmap::{cap_lamp_pool, LM_LAMP_POOL_RADIUS, LM_LAMP_SAT_DENSITY};
        let day = Self::lamp_daylight_scale(sun);
        if day < 1.0 {
            for l in lights.iter_mut() {
                l.color = l.color * day;
            }
        }
        let mut capped = 0usize;
        for l in lights.iter_mut() {
            let mount = (l.radius - LM_LAMP_POOL_RADIUS).max(0.25);
            if cap_lamp_pool(l, mount, LM_LAMP_SAT_DENSITY) < 1.0 {
                capped += 1;
            }
        }
        if capped > 0 {
            log!(
                "lamp bake: {capped} of {} static lights dimmed — their pool clipped the light atlas over more than {} texels",
                lights.len(),
                crate::lightmap::LM_LAMP_SAT_TEXELS as u32
            );
        }
    }

    /// This sun's daylight headroom factor for every static lamp — the one
    /// number that ties the lamp list to the sky. Read by the rail and by
    /// the bake/lamp-cache keys, so a sun change that MOVES it re-kicks the
    /// bake and a sun change that does not costs nothing.
    fn lamp_daylight_scale(sun: &SunLight) -> f32 {
        crate::lightmap::lamp_daylight_scale(crate::lightmap::daylight_on_ground(
            sun.dir, sun.color, sun.sky,
        ))
    }

    /// The daylight scale as a cache key: quantized to 1/32 so a day cycle
    /// re-bakes when the lamps CHANGE STRENGTH and never on a
    /// floating-point wobble of the sun. One step is 3% of the pool peak —
    /// 0.009 of light, under a byte on the brightest albedo there is.
    fn lamp_daylight_key(sun: &SunLight) -> u32 {
        (Self::lamp_daylight_scale(sun) * 32.0).round() as u32
    }

    /// Transient lights for THIS frame, on top of the per-frame list the
    /// renderer builds itself (street lamps + firework flashes). For hosts:
    /// muzzle flashes, spell impacts, anything that lives a few frames.
    /// Consumed by the next `draw_scene` call; never baked, so statics
    /// receive these analytically too.
    pub fn add_frame_lights(&mut self, lights: Vec<crate::lightmap::LmLight>) {
        self.host_lights.extend(lights);
    }

    /// Rebuild this frame's dynamic light list: harvested lamps first (they
    /// are the `frame_baked_count` prefix — already in the baked atlas, so
    /// only dynamic geometry may add them analytically), then transients.
    fn build_frame_lights(&mut self, sun: &SunLight) {
        // The SUN is part of the key: the daylight-headroom rail makes a
        // lamp's strength a function of the sky, so a day cycle that dims
        // the pools must dim them for dynamics too — the analytic term and
        // the baked atlas are the same lamp seen twice and may never
        // disagree.
        let key = (self.models_rev, Self::lamp_daylight_key(sun));
        if self.lamp_cache_rev != Some(key) {
            // The same list the bake snapshots, through the same rails.
            self.lamp_cache = self.static_lights_for(sun);
            self.lamp_cache_rev = Some(key);
            // The static-light selection grid lives and dies with the lamp
            // set — rebuilt HERE, on the settle path, never per frame. This
            // is what keeps runtime selection O(1) at any light count.
            self.light_grid = LightGrid::build(&self.lamp_cache, LIGHT_GRID_CELL);
            self.light_cell_memory.clear();
        }
        self.frame_lights.clear();
        self.frame_lights.extend(self.lamp_cache.iter().cloned());
        self.frame_baked_count = self.frame_lights.len();
        for f in &self.firework_instances {
            if let Some(l) = firework_flash_light(f) {
                self.frame_lights.push(l);
            }
        }
        self.frame_lights.append(&mut self.host_lights);
    }

    /// Snapshot the static scene and schedule the GPU bake. Called on the
    /// same settle-debounced cadence as the static shadow rebuild — a burst
    /// of edits pays one bake, after the world goes still. A newer kick
    /// replaces a pending one wholesale (the baker re-plans the layout).
    fn kick_lightmap_bake(
        &mut self,
        world: &GameWorld,
        sun: &SunLight,
        trigger: crate::gpu_lightmap::BakeTrigger,
    ) {
        let mut meshes = Vec::new();
        let mut mesh_map = Vec::new();
        let mut mesh_geometry = Vec::new();
        let mut casters_only = Vec::new();
        for (pi, inst) in self.placed_models.iter().enumerate() {
            if inst.dynamic {
                continue;
            }
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let Some(src) = &self.static_models[at].1.lm_source else {
                // No AO layout, no region — but it still casts: sun-depth
                // passes take its render geometry at its transform.
                let m = &self.static_models[at].1;
                let (lo, hi) = crate::lightmap::world_bounds(&inst.transform, (m.min, m.max));
                if casts_as_caster_only(
                    self.model_casts_shadow.get(&inst.model).copied(),
                    m.prelit,
                    lo,
                    hi,
                ) {
                    casters_only.push(crate::gpu_lightmap::GpuBakeMesh {
                        geometry: m.geometry.geometry_id(),
                        transform: inst.transform,
                        min: lo,
                        max: hi,
                    });
                }
                continue;
            };
            if std::env::var_os("MAKEPAD_GPU_LM_REGIONS").is_some() {
                // TEMP instrumentation: name each region's model and give the
                // exact chart uv -> world map of its two largest up-facing
                // triangles, so an atlas dump's texels map back to world.
                let k = meshes.len();
                let mut tris: Vec<(f32, u32)> = Vec::new();
                for t in 0..src.caster.tri_count() as u32 {
                    let (a, b, c) = src.caster.triangle(t);
                    let ab = b - a;
                    let ac = c - a;
                    let n = Vec3f {
                        x: ab.y * ac.z - ab.z * ac.y,
                        y: ab.z * ac.x - ab.x * ac.z,
                        z: ab.x * ac.y - ab.y * ac.x,
                    };
                    let area2 = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                    if area2 > 1e-9 && n.y / area2 > 0.9 {
                        tris.push((area2, t));
                    }
                }
                tris.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
                for (_, t) in tris.iter().take(24) {
                    let vs = src.caster.triangle_verts(*t);
                    let (a, b, c) = src.caster.triangle(*t);
                    let w = |p: Vec3f| {
                        let q = inst.transform.transform_vec4(Vec4f { x: p.x, y: p.y, z: p.z, w: 1.0 });
                        (q.x, q.y, q.z)
                    };
                    let uvs: Vec<[f32; 2]> =
                        vs.iter().map(|i| src.ao_uv[*i as usize]).collect();
                    let (wa, wb, wc) = (w(a), w(b), w(c));
                    log!(
                        "lmtri {} {} uv({:.4},{:.4})({:.4},{:.4})({:.4},{:.4}) w({:.2},{:.2},{:.2})({:.2},{:.2},{:.2})({:.2},{:.2},{:.2})",
                        k, inst.model,
                        uvs[0][0], uvs[0][1], uvs[1][0], uvs[1][1], uvs[2][0], uvs[2][1],
                        wa.0, wa.1, wa.2, wb.0, wb.1, wb.2, wc.0, wc.1, wc.2,
                    );
                }
            }
            meshes.push(crate::lightmap::LmMeshInstance {
                source: src.clone(),
                transform: inst.transform,
            });
            mesh_map.push(pi);
            // The FLAT-WINDING-normal variant: the gather's backface test
            // must see what the CPU rays saw (lm_source implies it exists).
            let m = &self.static_models[at].1;
            mesh_geometry.push(
                m.bake_geometry
                    .as_ref()
                    .map(|g| g.geometry_id())
                    .unwrap_or_else(|| m.geometry.geometry_id()),
            );
        }
        // ONE ground light field for the whole scene: a synthetic heightfield
        // of terrain ∪ static box tops (roads, slabs, platforms), addressed
        // by world xz. Terrain tiles and every cube top sample this same
        // region, which is what lets a box road receive a house's shadow
        // without cubes carrying any lightmap data at all.
        let mut terrain_world = None;
        let mut planars = Vec::new();
        {
            // Bounds from the STATICS, not the terrain: a big terrain would
            // stretch the single ground region over empty grass and starve
            // the shadows of texels (the village measured ~40cm/texel that
            // way). Terrain outside the field renders fully lit — the
            // shaders test the rect rather than clamp-smearing its border.
            let (mut lo_x, mut lo_z) = (f32::MAX, f32::MAX);
            let (mut hi_x, mut hi_z) = (f32::MIN, f32::MIN);
            for (bmin, bmax) in &self.receiver_boxes {
                lo_x = lo_x.min(bmin.x);
                lo_z = lo_z.min(bmin.z);
                hi_x = hi_x.max(bmax.x);
                hi_z = hi_z.max(bmax.z);
            }
            for m in &meshes {
                let (bmin, bmax) = m.world_bounds();
                lo_x = lo_x.min(bmin.x);
                lo_z = lo_z.min(bmin.z);
                hi_x = hi_x.max(bmax.x);
                hi_z = hi_z.max(bmax.z);
            }
            if lo_x < hi_x && lo_z < hi_z {
                // Pad so shadows can run past the outermost caster, square so
                // one origin/cell pair serves both axes.
                // Pad so shadows can run past the outermost caster; cap so
                // density never collapses on a sprawling world (chunked
                // ground regions are the real fix at that scale).
                let pad = 6.0;
                let (lo_x, lo_z) = (lo_x - pad, lo_z - pad);
                let span = ((hi_x - lo_x).max(hi_z - lo_z) + pad).min(240.0);
                let n = ((span * crate::lightmap::LM_PLANAR_TEXELS_PER_UNIT) as usize + 2)
                    .clamp(2, 1025);
                let cell = span / (n - 1) as f32;
                let mut heights = vec![0.0f32; n * n];
                for gz in 0..n {
                    for gx in 0..n {
                        let x = lo_x + gx as f32 * cell;
                        let z = lo_z + gz as f32 * cell;
                        let mut h = world
                            .terrain
                            .as_ref()
                            .and_then(|t| t.height_at(x, z))
                            .unwrap_or(0.0);
                        for (bmin, bmax) in &self.receiver_boxes {
                            if x >= bmin.x - cell
                                && x <= bmax.x + cell
                                && z >= bmin.z - cell
                                && z <= bmax.z + cell
                                // GROUNDED boxes only: a road slab or crate
                                // resting on the terrain IS the ground there
                                // and should receive shadows at its top. A
                                // FLOATING platform is not — hoisting the
                                // field to its top swallowed the shadow it
                                // casts on the grass below, leaving hollow
                                // rim shadows around a lit footprint.
                                && bmin.y - h <= 0.75
                            {
                                h = h.max(bmax.y);
                            }
                        }
                        heights[gz * n + gx] = h;
                    }
                }
                terrain_world = Some(Vec4f { x: lo_x, y: lo_z, z: span, w: span });
                planars.push(crate::lightmap::LmPlanar {
                    x0: lo_x,
                    z0: lo_z,
                    x1: lo_x + span,
                    z1: lo_z + span,
                    y: 0.0,
                    field: Some(crate::lightmap::LmHeightField {
                        origin_x: lo_x,
                        origin_z: lo_z,
                        cell,
                        n,
                        heights: std::sync::Arc::new(heights),
                    }),
                });
            }
        }
        if meshes.is_empty() && planars.is_empty() {
            self.lightmap = None;
            self.lm_remaps.clear();
            self.lm_ground = None;
            self.lm_top = None;
            return;
        }
        let lights = self.static_lights_for(sun);
        let scene = crate::lightmap::LmScene {
            meshes,
            planars,
            boxes: self.occluder_boxes.clone(),
            lights,
            sun_dir: sun.dir,
            sun_color: sun.color,
            sun_sky: sun.sky,
            // Bounce is the slow luxury tier; off unless asked for until the
            // disk cache lands.
            bounce: std::env::var("LM_BOUNCE").is_ok(),
        };
        // The snapshot becomes render passes on the next frame
        // (gpu_lightmap.rs); delivery is a texture handle swap, no upload.
        self.gpu_baker.schedule(crate::gpu_lightmap::GpuBakeJob {
            scene,
            mesh_geometry,
            mesh_map,
            casters_only,
            terrain_world,
            trigger,
        });
    }

    /// Night-sky star panorama: an equirectangular PNG (the NASA SVS Deep
    /// Star Map ships with the sandbox — see resources/sky/ATTRIBUTION).
    /// Decoded here once; uploaded on the first sky draw. No call = no
    /// stars, the night dome stays plain.
    pub fn set_star_map_png(&mut self, bytes: &[u8]) {
        match ImageBuffer::from_png(bytes) {
            Ok(img) => {
                self.star_map = Some(img);
                self.star_texture = None;
            }
            Err(e) => log!("star map: png decode failed: {e:?}"),
        }
    }

    /// Find and load the star panorama by the standard search order:
    /// the `MAKEPAD_STAR_MAP` env override, then — walking up from the
    /// working directory — the repo-local cache `local/sky/` that
    /// `tools/download_stars.sh` fills (NASA/GSFC SVS "Deep Star Maps
    /// 2020", public domain, credit NASA/GSFC SVS; the ATTRIBUTION.txt
    /// sits beside it), then the sandbox's bundled copy. Without a hit the
    /// analytic point stars stay and one hint is logged. Returns whether a
    /// panorama is loaded.
    pub fn load_star_map(&mut self) -> bool {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(path) = std::env::var("MAKEPAD_STAR_MAP") {
            if !path.is_empty() {
                candidates.push(path.into());
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            let mut dir = Some(cwd.as_path());
            while let Some(d) = dir {
                candidates.push(d.join("local/sky/starmap_2020_4k.png"));
                candidates
                    .push(d.join("apps/sandbox/resources/sky/starmap_2020_4k.png"));
                dir = d.parent();
            }
        }
        for path in &candidates {
            if !path.is_file() {
                continue;
            }
            match std::fs::read(path) {
                Ok(bytes) => {
                    self.set_star_map_png(&bytes);
                    if self.star_map.is_some() {
                        log!("star map: {}", path.display());
                        return true;
                    }
                }
                Err(e) => log!("star map: {}: {e}", path.display()),
            }
        }
        log!(
            "star map: none found — analytic stars only (run tools/download_stars.sh, or set MAKEPAD_STAR_MAP)"
        );
        false
    }

    /// The bound star texture + gain: the decoded panorama, or a 1x1 black
    /// stand-in (gain 0) so the sky shader samples unconditionally.
    fn star_binding(&mut self, cx: &mut Cx) -> (Texture, f32) {
        if let Some(tex) = &self.star_texture {
            return (tex.clone(), if self.star_map.is_some() { 1.0 } else { 0.0 });
        }
        let (tex, gain) = match self.star_map.take() {
            Some(img) => (img.into_new_texture(cx), 1.0),
            None => {
                let mut black = ImageBuffer::default();
                black.width = 1;
                black.height = 1;
                black.data = vec![0xFF00_0000];
                (black.into_new_texture(cx), 0.0)
            }
        };
        // The ImageBuffer moved into the texture; remember which case this
        // was so the gain answer stays stable on later frames.
        self.star_texture = Some(tex.clone());
        if gain > 0.0 {
            self.star_map = Some(ImageBuffer::default());
        }
        (tex, gain)
    }

    /// Live GPU-lightmap scheduling policy switch (OnChange <-> Realtime).
    /// Takes effect immediately: Realtime -> OnChange re-dirties every
    /// region so mover shadows stamped into the tiles are baked away.
    pub fn set_gpu_lightmap_mode(&mut self, mode: crate::gpu_lightmap::GpuLightmapMode) {
        self.gpu_baker.set_mode(mode);
    }

    pub fn gpu_lightmap_mode(&self) -> crate::gpu_lightmap::GpuLightmapMode {
        self.gpu_baker.mode()
    }

    /// `(regions done, regions in the kick)` while the static lighting is
    /// still filling in over successive frames, `None` once it has settled.
    /// A big world is playable from its first frame in flat light; this is
    /// what lets an app say so instead of leaving the player wondering why
    /// the shadows are missing.
    pub fn lightmap_bake_progress(&self) -> Option<(usize, usize)> {
        self.gpu_baker.bake_progress()
    }

    /// Configure the device-local Realtime cascaded-shadow budget. This is
    /// presentation-only and cannot affect the shared simulation. Explicit
    /// `MAKEPAD_CSM_RES` / `MAKEPAD_CSM_FAR` launch overrides take final
    /// precedence; the returned value is the effective configuration.
    pub fn set_csm_config(
        &mut self,
        tile_resolution: usize,
        far_range: f32,
    ) -> crate::gpu_lightmap::CsmConfig {
        self.gpu_baker
            .set_csm_config(tile_resolution, far_range)
    }

    pub fn csm_config(&self) -> crate::gpu_lightmap::CsmConfig {
        self.gpu_baker.csm_config()
    }

    /// Set the orbit-camera look-at depth the Realtime cascades tighten
    /// around. Pass `None` for first-person walk (village-scale ladder).
    pub fn set_csm_focus_distance(&mut self, focus: Option<f32>) {
        self.csm_focus = focus.filter(|d| d.is_finite() && *d > 0.0);
    }

    /// Supply the complete caster/receiver bound for Realtime cascade
    /// fitting. This is presentation state and is especially useful for an
    /// imported editor model that is intentionally submitted as a dynamic
    /// caster so its first frame already has shadows.
    pub fn set_csm_scene_bounds(&mut self, bounds: Option<(Vec3f, Vec3f)>) {
        self.csm_scene_bounds = bounds.filter(|(min, max)| {
            min.x.is_finite()
                && min.y.is_finite()
                && min.z.is_finite()
                && max.x.is_finite()
                && max.y.is_finite()
                && max.z.is_finite()
                && min.x <= max.x
                && min.y <= max.y
                && min.z <= max.z
        });
    }

    /// Select whether loaded metallic/roughness materials use the renderer's
    /// PBR lane. This is presentation-only and defaults to `true`.
    pub fn set_pbr_materials_enabled(&mut self, enabled: bool) {
        self.pbr_materials_enabled = enabled;
    }

    pub fn pbr_materials_enabled(&self) -> bool {
        self.pbr_materials_enabled
    }

    /// Bind a screen-space AO target (`ssao::SsaoPass::output`; x =
    /// occlusion, 1 = unoccluded) for this frame's model lanes, with its
    /// strength.
    /// The factor multiplies the AMBIENT fill only — the shaders never
    /// apply it to the direct sun or the shadow-mapped light. `None`
    /// switches it off; a host must call this every frame it wants it.
    pub fn set_ssao(&mut self, ssao: Option<(Texture, f32)>) {
        self.ssao = ssao;
    }

    /// The bake's stand-in geometry for primitive ENTITY casters: one unit
    /// box in the packed-mesh layout (position lanes only — the depth
    /// shaders read nothing else), built once.
    fn ensure_lm_box_geometry(&mut self, cx: &mut Cx) {
        if self.lm_box_geometry.is_some() {
            return;
        }
        let stride = crate::skin::SKIN_VERTEX_FLOATS;
        let mut vertices = Vec::with_capacity(8 * stride);
        for i in 0..8 {
            vertices.extend_from_slice(&[
                if i & 1 == 0 { -0.5 } else { 0.5 },
                if i & 2 == 0 { -0.5 } else { 0.5 },
                if i & 4 == 0 { -0.5 } else { 0.5 },
                0.0,
                0.0,
                0.0,
                0.0,
            ]);
        }
        // 12 triangles over the corner lattice; winding irrelevant, the
        // depth passes are double-sided (gpu_lightmap's box-soup layout).
        const QUADS: [[u32; 4]; 6] = [
            [0, 2, 6, 4],
            [1, 3, 7, 5],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [0, 1, 3, 2],
            [4, 5, 7, 6],
        ];
        let mut indices: Vec<u32> = Vec::with_capacity(36);
        for q in QUADS {
            indices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        let g = Geometry::new(cx);
        g.update(cx, indices, vertices);
        self.lm_box_geometry = Some(g);
    }

    /// Every dynamic caster for the Realtime bake's depth passes: dynamic
    /// placed models (driven cars), skinned CHARACTERS (rest mesh + this
    /// frame's palette — [`Self::pack_skin_palettes`] must already have
    /// run), and Rigid/Mover primitive entities (crates) as their box
    /// approximation — box IS the dominant entity shape, and the bake's
    /// soft shadow forgives a sphere's corners far better than a missing
    /// shadow reads.
    fn collect_lm_movers(
        &self,
        world: &GameWorld,
        skinned_items: Option<&[SkinnedDraw]>,
    ) -> Vec<crate::gpu_lightmap::GpuLmMover> {
        let mut out = Vec::new();
        for (slot, inst) in self.placed_models.iter().enumerate() {
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let m = &self.static_models[at].1;
            // Anim parts cast as MOVERS even when their level is static: the
            // static atlas was baked without them (they are not in its
            // stream), so a door's shadow can only come from the cascades —
            // and it has to follow the door, which is what a mover is.
            for part in &m.anim_parts {
                let (_, time, _) =
                    self.model_anim_state.clock(&ModelTarget::Instance(slot), &inst.model, &part.def);
                let pose = Mat4f::mul(&inst.transform, &part.def.transform_at(time));
                for (g, _, _, _) in &part.draws {
                    out.push(crate::gpu_lightmap::GpuLmMover {
                        geometry: g.geometry_id(),
                        transform: pose,
                        min: part.def.min,
                        max: part.def.max,
                        skin: None,
                    });
                }
            }
            for part in &m.driven_parts {
                let local = inst
                    .part_poses
                    .iter()
                    .find(|pose| pose.connection == part.def.connection)
                    .map(|pose| pose.transform)
                    .unwrap_or_else(|| part.def.rest_transform());
                let pose = Mat4f::mul(&inst.transform, &local);
                for (g, _, _, _) in &part.draws {
                    out.push(crate::gpu_lightmap::GpuLmMover {
                        geometry: g.geometry_id(),
                        transform: pose,
                        min: part.def.min,
                        max: part.def.max,
                        skin: None,
                    });
                }
            }
            if !inst.dynamic {
                continue;
            }
            // A multi-material GLB owns one resident geometry per layer.
            // The main geometry alone would make only layer zero cast; walk
            // every visible layer so "dynamic model" means the whole model.
            for geometry in std::iter::once(&m.geometry)
                .chain(m.extra_draws.iter().map(|(geometry, ..)| geometry))
            {
                out.push(crate::gpu_lightmap::GpuLmMover {
                    geometry: geometry.geometry_id(),
                    transform: inst.transform,
                    min: m.min,
                    max: m.max,
                    skin: None,
                });
            }
        }
        if let (Some(items), Some(palette_tex)) = (skinned_items, &self.skin_palette_tex) {
            for (i, item) in items.iter().enumerate() {
                let Some(base) = self.skin_joint_bases.get(i).copied().filter(|b| *b >= 0.0)
                else {
                    continue;
                };
                let Some(at) = self
                    .skin_rig_geometries
                    .iter()
                    .position(|(k, _, _)| *k == item.rig)
                else {
                    continue;
                };
                // Posed bounds when the host measured them; a generous
                // character-sized default otherwise (bounds only cull lamp
                // faces — the sun pass draws every caster regardless).
                let (min, max) = item
                    .bounds
                    .unwrap_or((vec3f(-1.0, 0.0, -1.0), vec3f(1.0, 2.2, 1.0)));
                out.push(crate::gpu_lightmap::GpuLmMover {
                    geometry: self.skin_rig_geometries[at].1.geometry_id(),
                    transform: item.transform,
                    min,
                    max,
                    skin: Some(crate::gpu_lightmap::GpuLmSkin {
                        joint_tex: palette_tex.clone(),
                        joint_base: base,
                    }),
                });
            }
        }
        if let Some(box_geom) = &self.lm_box_geometry {
            for e in world.entities.iter() {
                if !matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                    || e.sensor
                    || e.hidden
                    || e.attached_to != 0
                {
                    continue;
                }
                if std::env::var_os("MAKEPAD_CSM_CASTER_LOG").is_some() {
                    log!(
                        "csm caster box: id {} kind {:?} pos ({:.1},{:.1},{:.1}) half ({:.2},{:.2},{:.2}) scale ({:.2},{:.2},{:.2}) alpha {:.2}",
                        e.id, e.kind, e.pos.x, e.pos.y, e.pos.z,
                        e.half.x, e.half.y, e.half.z,
                        e.scale.x, e.scale.y, e.scale.z, e.color.w
                    );
                }
                let s = vec3f(
                    e.half.x * e.scale.x * 2.0,
                    e.half.y * e.scale.y * 2.0,
                    e.half.z * e.scale.z * 2.0,
                );
                let mut t = Self::entity_rotation(e);
                for c in 0..3 {
                    t.v[c] *= s.x;
                    t.v[4 + c] *= s.y;
                    t.v[8 + c] *= s.z;
                }
                t.v[12] = e.pos.x;
                t.v[13] = e.pos.y;
                t.v[14] = e.pos.z;
                out.push(crate::gpu_lightmap::GpuLmMover {
                    geometry: box_geom.geometry_id(),
                    transform: t,
                    min: vec3f(-0.5, -0.5, -0.5),
                    max: vec3f(0.5, 0.5, 0.5),
                    skin: None,
                });
            }
        }
        out
    }

    /// Per-frame GPU bake step: realizes scheduled jobs and encodes this
    /// frame's passes — the whole atlas once per dirty kick (both modes,
    /// statics only), plus the cascade depth pass every Realtime frame.
    /// Pass ordering guarantees everything renders before the scene pass
    /// that samples it — same-frame delivery.
    fn run_gpu_lightmap(
        &mut self,
        cx: &mut CxDraw,
        world: &GameWorld,
        movers: &[crate::gpu_lightmap::GpuLmMover],
        csm_view: Option<&crate::shadow_csm::CsmView>,
        eye: Vec3f,
    ) {
        // A realized atlas is not a precondition for the cascade tier. In
        // Realtime the cascades ARE the dynamic-shadow contract, and worlds
        // that own no static lightmap at all — a flat starter terrain with
        // no props, so `kick_lightmap_bake` finds no AO mesh and no receiver
        // box and never schedules a job — must still get them. Gating this
        // on `has_state` is what made F8 read as "realtime deletes every
        // dynamic shadow" in exactly those worlds.
        if !self.gpu_baker.has_state()
            && !crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode()).csm
        {
            return;
        }
        // DEBUG (macOS): MAKEPAD_GPU_LM_DUMP=<prefix> writes the settled
        // GPU atlas as `<prefix>.a.pgm` (A = sun SDF) + `<prefix>.rgb.ppm`
        // (lamps) — the byte-level counterpart of the CPU bake's old
        // SANDBOX_LM_DUMP, for numeric parity comparison.
        #[cfg(target_os = "macos")]
        {
            static DUMPED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            static DUMP_FRAME: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if let Ok(prefix) = std::env::var("MAKEPAD_GPU_LM_DUMP") {
                // MAKEPAD_GPU_LM_DUMP_FRAME=<n> delays the capture n bake
                // frames so a streamed-in world (or Realtime, which is idle
                // from its first frame) dumps the SETTLED scene, not the
                // first half-loaded bake.
                let wait = std::env::var("MAKEPAD_GPU_LM_DUMP_FRAME")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                if self.gpu_baker.is_idle()
                    && DUMP_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= wait
                    && !DUMPED.swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    if let Some(atlas) = self.lightmap.clone() {
                        if let Some((w, h, bytes)) = cx.cx.debug_read_render_texture(&atlas) {
                            let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
                            pgm.extend(bytes.chunks_exact(4).map(|px| px[3]));
                            let _ = std::fs::write(format!("{prefix}.a.pgm"), pgm);
                            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                            for px in bytes.chunks_exact(4) {
                                // Metal readback of BGRA8 is BGRA byte order.
                                ppm.extend([px[2], px[1], px[0]]);
                            }
                            let _ = std::fs::write(format!("{prefix}.rgb.ppm"), ppm);
                            log!("gpu lightmap: dumped {w}x{h} atlas to {prefix}.a.pgm/.rgb.ppm");
                        }
                    }
                    // Intermediates: coverage (R=lit-frac, G=covered) — the
                    // gather truth before any distance transform.
                    for (tex, name) in self.gpu_baker.debug_stage_textures() {
                        if let Some((w, h, bytes)) = cx.cx.debug_read_render_texture(&tex) {
                            if name.contains("depth") {
                                // R32F scratch: raw little-endian floats,
                                // header-free — `w`/`h` ride in the name.
                                let _ = std::fs::write(
                                    format!("{prefix}.{name}.{w}x{h}.f32"),
                                    &bytes,
                                );
                                continue;
                            }
                            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                            for px in bytes.chunks_exact(4) {
                                ppm.extend([px[2], px[1], px[0]]);
                            }
                            let _ = std::fs::write(format!("{prefix}.{name}.ppm"), ppm);
                        }
                    }
                }
            }
        }
        let sun = crate::sun::resolve_sun(&world.sun);
        // The re-bake idempotence probe (macOS readback): with
        // MAKEPAD_GPU_LM_REBAKE set, every settled bake reports its atlas
        // signature, and each bake after the first reports its DIFF against
        // the first — the instrument that measured the accumulator leak
        // (gpu_lightmap.rs, section 1). `=<n>` also re-bakes the same world n
        // times by itself: MODE=redirty re-bakes the realized layout into its
        // own targets, anything else re-kicks the whole job. MIN_REGIONS /
        // MIN_LAMPS hold the probe until the streamed world has arrived.
        #[cfg(target_os = "macos")]
        if let Some(n) = std::env::var("MAKEPAD_GPU_LM_REBAKE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SETTLED_FRAMES: AtomicUsize = AtomicUsize::new(0);
            static BAKES: AtomicUsize = AtomicUsize::new(0);
            let env_usize = |k: &str| {
                std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0)
            };
            let (regions, lamps) = self.gpu_baker.debug_scene_size();
            if self.gpu_baker.is_idle()
                && regions >= env_usize("MAKEPAD_GPU_LM_REBAKE_MIN_REGIONS")
                && lamps >= env_usize("MAKEPAD_GPU_LM_REBAKE_MIN_LAMPS")
            {
                // Edge-triggered: one report per bake, not one per idle
                // frame — the readback and the diff are not free.
                if SETTLED_FRAMES.fetch_add(1, Ordering::Relaxed) == 2 {
                    let k = BAKES.fetch_add(1, Ordering::Relaxed);
                    if let Some(atlas) = self.lightmap.clone() {
                        if let Some((w, h, bytes)) = cx.cx.debug_read_render_texture(&atlas) {
                            lm_probe_report(
                                k,
                                w,
                                h,
                                regions,
                                lamps,
                                &bytes,
                                &self.gpu_baker.debug_region_rects(),
                            );
                        }
                    }
                    if k < n {
                        if std::env::var("MAKEPAD_GPU_LM_REBAKE_MODE").as_deref() == Ok("redirty") {
                            self.gpu_baker.debug_redirty();
                        } else {
                            self.kick_lightmap_bake(
                                world,
                                &sun,
                                crate::gpu_lightmap::BakeTrigger::WorldEdit,
                            );
                        }
                    }
                }
            } else {
                SETTLED_FRAMES.store(0, Ordering::Relaxed);
            }
        }
        let csm_scene_bounds = self.csm_scene_bounds;
        if let Some(d) = self.gpu_baker.run_frame(
            cx,
            sun.dir,
            &self.csm_static_casters,
            movers,
            csm_view,
            eye,
            csm_scene_bounds,
        ) {
            self.lightmap = Some(d.atlas);
            self.lm_remaps = vec![Vec4f::default(); self.placed_models.len()];
            for (k, pi) in d.mesh_map.iter().enumerate() {
                if let Some(slot) = self.lm_remaps.get_mut(*pi) {
                    *slot = d.mesh_rects[k].uv_remap(d.size);
                }
            }
            self.lm_ground = match (d.planar_rects.first(), d.terrain_world) {
                (Some(r), Some(w)) => {
                    // Density in the log so a dump's texels map back to
                    // world coordinates without spelunking.
                    log!(
                        "gpu lightmap: ground region {}x{} px at ({},{}) over world ({:.1},{:.1}) span {:.1} — {:.2} texels/unit",
                        r.w, r.h, r.x, r.y, w.x, w.y, w.z,
                        r.w as f32 / w.z.max(0.0001)
                    );
                    Some((r.uv_remap(d.size), w))
                }
                _ => None,
            };
            self.lm_top = Some(d.top);
        }
    }

    /// Is the baked lightmap being SAMPLED? (The bake itself always runs.)
    pub fn lightmap_enabled(&self) -> bool {
        self.lightmap_enabled
    }

    /// Turn baked-lightmap sampling on/off; returns the new state. Off, the
    /// world renders on the analytic path alone: full sun everywhere the
    /// cascades do not shadow, and no baked lamp pools. Nothing is
    /// invalidated, so the toggle is instant in both directions.
    pub fn set_lightmap_enabled(&mut self, on: bool) -> bool {
        self.lightmap_enabled = on;
        on
    }

    /// Which SPACE the model lanes shade in.
    ///
    /// Off (the default) is the game's, and it is right for a game: its
    /// texels are sRGB bytes used as-is, its [`crate::sun::SunLight`] rig is
    /// display-referred (0.72 direct plus 0.28 ambient, one for a fully lit
    /// white surface), and their product goes straight into the 8-bit
    /// target. Nothing in that path is linear and nothing needs mapping.
    ///
    /// On, the lanes decode their texels to linear reflectance, shade there,
    /// and finish through the same ACES fit and display gamma the analytic
    /// sky, the fog colour and the path tracer already use. That is what a
    /// host shading with real light needs — and, just as much, what any host
    /// with DARK materials needs: multiplying a cosine by an sRGB-encoded
    /// albedo and writing it raw crushes the midtones, so a fully sunlit
    /// charcoal wall lands at a tenth of the value it should and reads as a
    /// silhouette against a sky that WAS tone mapped.
    ///
    /// Only the placed/attached model lanes carry it. A world that also
    /// drives the cube, terrain or water lanes is a game world, and games
    /// leave this off.
    pub fn set_display_transform(&mut self, on: bool) {
        self.display_transform = if on { 1.0 } else { 0.0 };
    }

    /// Is the model lanes' linear + ACES lane on?
    pub fn display_transform(&self) -> bool {
        self.display_transform > 0.5
    }

    /// Flip it. The sandbox binds this to F9.
    pub fn toggle_lightmap(&mut self) -> bool {
        self.lightmap_enabled = !self.lightmap_enabled;
        self.lightmap_enabled
    }

    /// The lightmap atlas to bind: the real one, else a 1x1 "fully sunlit,
    /// no lamps" stand-in so shaders sample unconditionally. The stand-in
    /// is also what the kill switch binds — an atlas that says "lit, no
    /// lamps" everywhere IS the analytic path.
    fn lightmap_texture(&mut self, cx: &mut Cx) -> Texture {
        if let (true, Some(t)) = (self.lightmap_enabled, &self.lightmap) {
            return t.clone();
        }
        if self.lm_fallback.is_none() {
            self.lm_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    // A=255 (lit), RGB=0 (no lamp light).
                    data: Some(vec![0xFF00_0000]),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.lm_fallback.clone().unwrap()
    }

    fn upload_detail(
        &mut self,
        cx: &mut Cx,
        png: Option<&[u8]>,
        scale: [f32; 2],
    ) -> (Texture, [f32; 2]) {
        if scale[0].abs() <= 1e-4 || png.is_none() {
            return (self.detail_neutral(cx), [0.0, 0.0]);
        }
        match ImageBuffer::from_png(png.unwrap()) {
            Ok(buf) => (buf.into_new_mip_repeat_texture(cx), scale),
            Err(_) => (self.detail_neutral(cx), [0.0, 0.0]),
        }
    }

    /// Make one draw layer's material resident.
    ///
    /// A material with no shininess is flattened to the neutral one on the
    /// way in, so nothing downstream has to remember that glTF's default
    /// `metallicFactor` is 1 — see [`LayerMaterial`]. A metallicRoughness
    /// image that fails to decode (a GLB embedding JPEG rather than PNG)
    /// degrades to the factors instead of failing the model: a prop that
    /// loses its roughness MAP still looks like the prop, and a prop that
    /// fails to load does not.
    ///
    /// The decoded pixels are consumed by the upload — `ImageBuffer` is moved
    /// into the texture — so the only host copy that outlives this call is
    /// the encoded PNG inside the `StaticModel`, which the caller drops.
    fn upload_material(&mut self, cx: &mut Cx, pbr: &crate::model::PbrMaterial) -> LayerMaterial {
        if !pbr.is_shiny() {
            return LayerMaterial {
                metallic: 0.0,
                roughness: 1.0,
                orm: self.orm_neutral(cx),
                orm_on: false,
            };
        }
        let orm = pbr
            .orm_png
            .as_deref()
            .and_then(|bytes| ImageBuffer::from_png(bytes).ok())
            .map(|buf| buf.into_new_mip_repeat_texture(cx));
        match orm {
            Some(tex) => LayerMaterial {
                metallic: pbr.metallic,
                roughness: pbr.roughness,
                orm: tex,
                orm_on: true,
            },
            None => LayerMaterial {
                metallic: pbr.metallic,
                roughness: pbr.roughness,
                orm: self.orm_neutral(cx),
                orm_on: false,
            },
        }
    }

    /// White 1x1: sampled as an ORM it multiplies both factors by 1, so a
    /// material that binds it reads exactly as its factors even if `orm_on`
    /// were ever set by mistake.
    fn orm_neutral(&mut self, cx: &mut Cx) -> Texture {
        if self.orm_fallback.is_none() {
            self.orm_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    data: Some(vec![0xFFFF_FFFF]),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.orm_fallback.clone().unwrap()
    }

    fn detail_neutral(&mut self, cx: &mut Cx) -> Texture {
        if self.detail_fallback.is_none() {
            self.detail_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    data: Some(vec![0xFF80_8080]),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.detail_fallback.clone().unwrap()
    }

    /// The shadow-top plane to bind plus its (base, range) decode: the real
    /// one, else a 1x1 "no blocker measured" stand-in (byte 255) so shaders
    /// sample unconditionally.
    fn lm_top_binding(&mut self, cx: &mut Cx) -> (Texture, f32, f32) {
        if let (true, Some((t, base, range))) = (self.lightmap_enabled, &self.lm_top) {
            return (t.clone(), *base, *range);
        }
        if self.lm_top_fallback.is_none() {
            self.lm_top_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: 1,
                    height: 1,
                    data: Some(vec![255]),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            ));
        }
        (self.lm_top_fallback.clone().unwrap(), 0.0, 8.0)
    }

    /// Write this frame's cascade binding into one material family:
    /// `csm_map` at `slot` plus the shared `csm_*` uniforms. No binding
    /// (OnChange / no scene / sun down) writes the tier OFF and parks the
    /// harmless `fallback` texture in the slot — `csm_vis` early-outs
    /// before ever sampling it, but the pipeline still wants a bound
    /// texture.
    fn write_csm_uniforms(
        cx: &mut Cx,
        dv: &mut DrawVars,
        binding: &Option<(crate::shadow_csm::CsmFrame, Texture, f32)>,
        fallback: &Texture,
        slot: usize,
    ) {
        let Some((frame, tex, inv_res)) = binding else {
            dv.set_texture(slot, fallback);
            dv.set_uniform(cx, live_id!(csm_p), &[0.0, 0.001, 0.0, 0.0]);
            return;
        };
        dv.set_texture(slot, tex);
        let c = &frame.cascades;
        dv.set_uniform(
            cx,
            live_id!(csm_p),
            &[1.0, *inv_res, c[0].texel_world, c[1].texel_world],
        );
        dv.set_uniform(
            cx,
            live_id!(csm_bias),
            &[c[0].bias01, c[1].bias01, c[2].bias01, c[2].texel_world],
        );
        let rows = |v: Vec4f| [v.x, v.y, v.z, v.w];
        dv.set_uniform(cx, live_id!(csm_rx0), &rows(c[0].rx));
        dv.set_uniform(cx, live_id!(csm_ry0), &rows(c[0].ry));
        dv.set_uniform(cx, live_id!(csm_rz0), &rows(c[0].rz));
        dv.set_uniform(cx, live_id!(csm_rx1), &rows(c[1].rx));
        dv.set_uniform(cx, live_id!(csm_ry1), &rows(c[1].ry));
        dv.set_uniform(cx, live_id!(csm_rz1), &rows(c[1].rz));
        dv.set_uniform(cx, live_id!(csm_rx2), &rows(c[2].rx));
        dv.set_uniform(cx, live_id!(csm_ry2), &rows(c[2].ry));
        dv.set_uniform(cx, live_id!(csm_rz2), &rows(c[2].rz));
    }

    pub fn model_is_loaded(&self, id: &str) -> bool {
        self.static_models.iter().any(|(k, _)| k == id)
    }

    /// The prop's low-res multi-box collider in model space. A house comes
    /// back as walls and roof rather than one box, so its doorway is a gap.
    /// Light-bake occluder boxes for a loaded prop — few, face-aligned.
    pub fn model_occluder_parts(&self, id: &str) -> Option<&[(Vec3f, Vec3f)]> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.occluder_parts.as_slice())
    }

    pub fn model_collider_parts(&self, id: &str) -> Option<&[(Vec3f, Vec3f)]> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.collider_parts.as_slice())
    }

    /// Model-space bounds of a loaded prop, for building its collider.
    pub fn model_bounds(&self, id: &str) -> Option<(Vec3f, Vec3f)> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| (m.min, m.max))
    }

    /// Generic externally-driven connection points declared by this model.
    /// Empty is a valid ordinary model; no source-pack naming is consulted.
    pub fn model_driven_parts(&self, id: &str) -> Vec<DrivenPartInfo> {
        self.static_models
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, model)| {
                model
                    .driven_parts
                    .iter()
                    .map(|part| DrivenPartInfo {
                        connection: part.def.connection.clone(),
                        pivot: part.def.pivot,
                        anchor: part.def.anchor,
                        radius: part.def.radius,
                        width: part.def.width,
                        rest_transform: part.def.rest_transform(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Advance the sky clock — what the scrolling layers of a Quake sky
    /// ride. The host ticks it beside its other presentation clocks; a
    /// paused game keeps a still sky, and a capture that sets the time
    /// explicitly ([`Self::set_sky_time`]) is reproducible.
    pub fn tick_sky(&mut self, dt: f32) {
        if dt.is_finite() {
            // Wrapped well beyond any layer's period so a long session never
            // loses precision in the scroll offset.
            self.sky_time = (self.sky_time + dt) % 4096.0;
        }
    }

    pub fn set_sky_time(&mut self, time: f32) {
        self.sky_time = time;
    }

    pub fn sky_time(&self) -> f32 {
        self.sky_time
    }

    /// The map's sky definition (projection, layer count, repeat, speeds),
    /// for a host that wants to report or drive it.
    pub fn model_sky(&self, id: &str) -> Option<&crate::model::SkyPart> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .and_then(|(_, m)| m.sky.as_ref())
            .map(|s| &s.part)
    }

    /// The sky faces' triangles in MODEL space.
    ///
    /// Kept separate from [`Self::model_mesh`] because the two answers differ
    /// by consumer: the RENDERER must not shade sky faces like walls, but a
    /// WALKER usually must collide with them — a Doom sky brush is solid, and
    /// dropping it from collision lets the player walk out of the map.
    pub fn model_sky_mesh(&self, id: &str) -> Option<(&[Vec3f], &[u32])> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .and_then(|(_, m)| m.sky.as_ref())
            .map(|s| (s.positions.as_slice(), s.indices.as_slice()))
    }

    /// Say whether a model casts into the baked sun shadows and Realtime CSM.
    ///
    /// Only matters for a static that owns no AO layout of its own, which is
    /// the lane that otherwise falls back to a size heuristic
    /// ([`CASTER_ONLY_MAX_SPAN`]). A host loading an imported LEVEL — one
    /// enormous GLB that IS the world — should pass `false`: a level that
    /// casts into the bake shadows its own interior and every room goes
    /// dark. Props default to casting, so a fence still throws a shadow.
    ///
    /// CSM registration updates immediately; the pending atlas bake is also
    /// re-kicked for OnChange.
    pub fn set_model_casts_shadow(&mut self, id: &str, casts: bool) {
        if self.model_casts_shadow.insert(id.to_string(), casts) != Some(casts) {
            self.placed_scene_signature = None;
            self.models_rev = self.models_rev.wrapping_add(1);
            self.rebuild_csm_static_casters();
        }
    }

    /// The loaded model's own triangles in MODEL space — positions and
    /// indices, the pair a collision structure is built from.
    ///
    /// Kept from the load so a walker never re-parses the GLB (`level.rs`
    /// builds its BVH straight off this). ANIM PARTS ARE NOT IN IT: a door
    /// moves, so it belongs to [`Self::anim_part_boxes`], not to a static
    /// acceleration structure that would go stale the moment it opened.
    pub fn model_mesh(&self, id: &str) -> Option<(&[Vec3f], &[u32])> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| (m.mesh_positions.as_slice(), m.mesh_indices.as_slice()))
    }

    /// One named part's definition (states, clip, local bounds). The
    /// definitions are interleaved with their GPU handles, so a model's parts
    /// are enumerated by name ([`Self::model_anim_part_names`]) and fetched
    /// one at a time rather than handed out as a slice.
    pub fn model_anim_part(&self, id: &str, part: &str) -> Option<&crate::model::AnimPart> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .and_then(|(_, m)| m.anim_parts.iter().find(|p| p.def.name == part))
            .map(|p| &p.def)
    }

    /// Every part name a loaded model exposes, in file order.
    pub fn model_anim_part_names(&self, id: &str) -> Vec<String> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.anim_parts.iter().map(|p| p.def.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Drive one part toward a named state — the game's whole handle on a
    /// door.
    ///
    /// `target` is a model id (every placed copy; an imported level is one
    /// copy, so this is the usual form) or a placed slot index. `blend_secs`
    /// is how long THIS move takes: 0 snaps, and a command that lands
    /// mid-move retargets from where the part currently is, so a door caught
    /// half open reverses smoothly instead of jumping.
    ///
    /// Returns false — and changes nothing — when the model is not resident,
    /// has no such part, or the part has no such state; the caller has asked
    /// for something that does not exist, and silently doing nothing would
    /// hide an importer/contract mismatch.
    pub fn set_model_state(
        &mut self,
        target: impl Into<ModelTarget>,
        part: &str,
        state: &str,
        blend_secs: f32,
    ) -> bool {
        let target = target.into();
        let Some(model_id) = self.target_model_id(&target) else {
            return false;
        };
        // Fields, not accessors: the definition borrows `static_models` while
        // the clock map is written, and those are disjoint.
        let Some(def) = self
            .static_models
            .iter()
            .find(|(k, _)| *k == model_id)
            .and_then(|(_, m)| m.anim_parts.iter().find(|p| p.def.name == part))
            .map(|p| &p.def)
        else {
            return false;
        };
        self.model_anim_state.set(target, def, state, blend_secs)
    }

    /// Advance every triggered part by `dt` seconds of wall clock. Motion is
    /// LINEAR in time along the clip — the host ticks this once a frame,
    /// exactly where it ticks the rest of its presentation.
    pub fn tick_model_states(&mut self, dt: f32) {
        self.model_anim_state.tick(dt);
    }

    /// What every part of `target` is doing right now. Parts nobody has
    /// triggered report their model's default state.
    pub fn model_states(&self, target: impl Into<ModelTarget>) -> Vec<ModelPartState> {
        let target = target.into();
        let Some(model_id) = self.target_model_id(&target) else {
            return Vec::new();
        };
        let Some((_, loaded)) = self.static_models.iter().find(|(k, _)| *k == model_id) else {
            return Vec::new();
        };
        loaded
            .anim_parts
            .iter()
            .map(|p| {
                let (state, time, goal) = self.model_anim_state.clock(&target, &model_id, &p.def);
                ModelPartState {
                    part: p.def.name.clone(),
                    state,
                    state_name: p.def.states.get(state).cloned().unwrap_or_default(),
                    time,
                    target_time: goal,
                    settled: (time - goal).abs() <= 1.0e-6,
                }
            })
            .collect()
    }

    /// One part's state, or `None` when the model or part is unknown.
    pub fn model_part_state(
        &self,
        target: impl Into<ModelTarget>,
        part: &str,
    ) -> Option<ModelPartState> {
        let target = target.into();
        self.model_states(target).into_iter().find(|s| s.part == part)
    }

    /// The model id a target resolves to: itself, or the model in that slot.
    fn target_model_id(&self, target: &ModelTarget) -> Option<String> {
        match target {
            ModelTarget::Model(id) => Some(id.clone()),
            ModelTarget::Instance(i) => self.placed_models.get(*i).map(|m| m.model.clone()),
        }
    }


    /// Every placed instance's anim parts as WORLD-space collider boxes for
    /// this moment — a closed door is a wall, an open one is a hole, and a
    /// door caught halfway is exactly where it looks.
    ///
    /// This is the mover/walker query: it is not a broad-phase structure, so
    /// a caller with many doors should reject on `min`/`max` first.
    pub fn anim_part_boxes(&self) -> Vec<AnimPartBox> {
        let mut out = Vec::new();
        for (slot, inst) in self.placed_models.iter().enumerate() {
            let Some((_, loaded)) = self.static_models.iter().find(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            for part in &loaded.anim_parts {
                let (state, time, _) =
                    self.model_anim_state.clock(&ModelTarget::Instance(slot), &inst.model, &part.def);
                let m = Mat4f::mul(&inst.transform, &part.def.transform_at(time));
                let (boxes, min, max) = world_boxes(&m, &part.collider);
                out.push(AnimPartBox {
                    instance: slot,
                    model: inst.model.clone(),
                    part: part.def.name.clone(),
                    kind: part.def.kind.clone(),
                    state,
                    state_name: part.def.states.get(state).cloned().unwrap_or_default(),
                    boxes,
                    min,
                    max,
                });
            }
        }
        out
    }

    /// One part's world-space collider boxes. `Model` targets answer for the
    /// FIRST placed copy of that id (an imported level has exactly one).
    pub fn anim_part_collider(
        &self,
        target: impl Into<ModelTarget>,
        part: &str,
    ) -> Option<Vec<(Vec3f, Vec3f)>> {
        let target = target.into();
        let slot = match &target {
            ModelTarget::Instance(i) => *i,
            ModelTarget::Model(id) => self.placed_models.iter().position(|m| m.model == *id)?,
        };
        let inst = self.placed_models.get(slot)?;
        let (_, loaded) = self.static_models.iter().find(|(k, _)| *k == inst.model)?;
        let found = loaded.anim_parts.iter().find(|p| p.def.name == part)?;
        let (_, time, _) = self.model_anim_state.clock(&target, &inst.model, &found.def);
        let m = Mat4f::mul(&inst.transform, &found.def.transform_at(time));
        Some(world_boxes(&m, &found.collider).0)
    }

    /// Triangle count of a loaded prop, so a caller can budget a scene before
    /// drawing it. Counted from the index buffer rather than stored, because
    /// this is a reporting path, not a hot one.
    pub fn model_triangles(&self, id: &str) -> Option<usize> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.triangles)
    }

    /// Draw one world-model lane. Instances are grouped by model, so N
    /// copies of one prop cost ONE draw item with N instances rather than N
    /// draws — which is what makes a scene full of stock trees affordable.
    #[allow(clippy::too_many_arguments)]
    fn draw_models_inner(
        &mut self,
        cx: &mut Cx3d,
        mut draw: ModelDraw<'_>,
        eye: Vec3f,
        instances: &[ModelInstance],
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if instances.is_empty() {
            return;
        }
        let pbr_lane = draw.is_pbr();
        {
            let draw = draw.base();
            sun.write_into(
                &mut draw.light_dir,
                &mut draw.sun_color,
                &mut draw.sun_sky,
                &mut draw.sun_ground,
            );
            draw.fog_color = fog.0;
            draw.fog_density = fog.1;
            draw.depth_clip = 1.0;
            draw.lm_debug = self.lm_debug;
        }
        // Shading space, once for the whole lane: written before anything
        // binds, so every draw item of the frame reads the same value.
        {
            let display = self.display_transform;
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(display), &[display, 0.0, 0.0, 0.0]);
        }
        // The specular lobe is the one term in this renderer that needs the
        // camera. TRUE world position, matching v_csm.xyz / v_csm_n.
        if pbr_lane {
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(eye), &[eye.x, eye.y, eye.z, 0.0]);
        }
        // Screen-space AO, once for the whole lane (both lanes pass through
        // here). Ambient-only by shader law; strength 0 = the slot is never
        // sampled, which is every game host's path.
        {
            let ssao = self.ssao.clone();
            let vars = &mut draw.base().draw_vars;
            match &ssao {
                Some((tex, strength)) => {
                    vars.set_texture(7, tex);
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[*strength, 0.0, 0.0, 0.0]);
                }
                None => {
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[0.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        // One atlas for the whole static scene, so binding it here costs
        // nothing per instance and never breaks batching.
        let lm_tex = self.lightmap_texture(cx.cx);
        draw.base().draw_vars.set_texture(2, &lm_tex);
        {
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draw.base().draw_vars, &csm, &lm_tex, 4);
        }
        // The ground region, for DYNAMIC instances' baked sun shadow (a
        // driven car crossing a house's shadow darkens). A channel only —
        // their lamp light is analytic. The shadow-top plane rides along so
        // a dynamic lifted above the blocker rejects the ground's shadow.
        {
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            draw.base().draw_vars.set_texture(3, &top_tex);
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_top_decode),
                &[top_base, top_range, 0.0, 0.0],
            );
            let (g_rect, g_world) = self.lm_ground.unwrap_or_default();
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_ground_rect),
                &[g_rect.x, g_rect.y, g_rect.z, g_rect.w],
            );
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_ground_world),
                &[g_world.x, g_world.y, g_world.z, g_world.w],
            );
        }
        // Dynamic lights. STATICS share one transient-only block (their
        // lamp light is baked; only the dl_split prefix reaches them, so a
        // zeroed lamp region costs nothing and keeps their per-model
        // batching whole). DYNAMICS are per-INSTANCE: each looks up ITS OWN
        // precomputed grid cell (O(1), light_grid.rs) with a positional
        // dead-band, so a car parked under a lamp holds a byte-identical
        // block frame after frame — the no-flicker contract. Different
        // blocks split draw items on purpose (the append test compares
        // uniforms); a handful of extra items beats a light popping.
        let (mut anch_x, mut anch_z, mut anch_n) = (0.0f64, 0.0f64, 0u32);
        for inst in instances {
            anch_x += inst.transform.v[12] as f64;
            anch_z += inst.transform.v[14] as f64;
            anch_n += 1;
        }
        let anchor_all = vec3f(
            (anch_x / anch_n.max(1) as f64) as f32,
            1.0,
            (anch_z / anch_n.max(1) as f64) as f32,
        );
        let empty_block = LightBlock::default();
        let mut static_block = [0.0f32; LIGHT_BLOCK_FLOATS];
        let static_split = merge_transients_into_block(
            &empty_block,
            &self.frame_lights,
            self.frame_baked_count..self.frame_lights.len(),
            anchor_all,
            &mut self.light_rank,
            &mut static_block,
        );
        // Which block the draw_vars currently carry; statics rewrite only
        // after a dynamic instance changed it.
        let mut dynamic_block_active = true;
        // Sort by model so equal geometry+texture land adjacent: consecutive
        // add_instance calls with unchanged geometry and texture accumulate
        // into a single draw item.
        let mut order: Vec<usize> = (0..instances.len()).collect();
        order.sort_by(|a, b| instances[*a].model.cmp(&instances[*b].model));
        let mut last: Option<String> = None;
        for i in order {
            let inst = &instances[i];
            let dynamic = lane.is_dynamic(inst);
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model) else {
                continue;
            };
            let loaded = &self.static_models[at];
            // Lane filter. Both passes walk the same instance list — indices
            // address `lm_remaps` / `model_ground` / the light-cell key, so
            // they must not be renumbered — and each takes only the models
            // its shader owns.
            let uses_pbr_lane = self.pbr_materials_enabled && loaded.1.wants_pbr;
            if uses_pbr_lane != pbr_lane {
                continue;
            }
            // Hoisted: `loaded` borrows self, and the per-instance light
            // block below needs `&mut self` (cell hysteresis).
            let tri_count = loaded.1.triangles;
            // Offscreen copies are skipped BEFORE packing, and a model whose
            // copies all fall outside never opens a draw item at all. The
            // shadow a prop casts is not affected: prop shadows live in the
            // merged static shadow mesh, which is drawn whole regardless.
            if let Some(frustum) = frustum {
                if !frustum.intersects_obb(loaded.1.min, loaded.1.max, &inst.transform) {
                    match lane {
                        WorldModelLane::Placed => stats.model_culled += 1,
                        WorldModelLane::Attachment => stats.world_attachment_culled += 1,
                    }
                    continue;
                }
            }
            let (layer_draws, prelit) = {
                let m = &loaded.1;
                let mut layers = Vec::with_capacity(1 + m.extra_draws.len());
                // A model that is ALL anim parts (a lone door asset) has an
                // empty static stream and nothing to draw for layer 0.
                if m.triangles > 0 {
                    layers.push((
                        m.geometry.geometry_id(),
                        m.texture.clone(),
                        m.detail.clone(),
                        m.detail_scale,
                        m.material.clone(),
                    ));
                }
                for (g, t, d, s, mat) in &m.extra_draws {
                    layers.push((g.geometry_id(), t.clone(), d.clone(), *s, mat.clone()));
                }
                (layers, m.prelit)
            };
            // Rigid parts ride the PARENT's material: a door is cut from the
            // level tile it sits in, so its metal/roughness is the model's.
            let part_material = loaded.1.material.clone();
            // The pack's baked occlusion, on slot 1. Packs share atlases, so
            // this changes only when the pack does — the sort above keeps
            // models of a pack adjacent, so it does not break batching.
            let ao_tex = self
                .model_pack
                .iter()
                .find(|(m, _)| *m == inst.model)
                .and_then(|(_, pack)| self.ao_textures.iter().find(|(k, _)| k == pack))
                .map(|(_, t)| t);
            if let Some(t) = ao_tex {
                draw.base().draw_vars.set_texture(1, t);
                draw.base().ao_enabled = 1.0;
                stats.ao_bound += 1;
            } else {
                draw.base().ao_enabled = 0.0;
                stats.ao_missing += 1;
            }
            draw.base().transform = inst.transform;
            // This copy's window into the light atlas; zero disables — a
            // dynamic prop or an unbaked model lights analytically as before.
            draw.base().lm_rect = if dynamic {
                Vec4f::default()
            } else {
                self.lm_remaps.get(i).copied().unwrap_or_default()
            };
            // Dynamic-light gate: dynamics sum every uniform slot (lamps
            // included), statics only the transient prefix — their lamp
            // light is already in the atlas RGB.
            draw.base().dl_apply = if dynamic { 1.0 } else { 0.0 };
            draw.base().depth_bias = inst.depth_order * 1.0e-3;
            if dynamic {
                // This instance's OWN cell block + transients, and its
                // ground plane for the sun-ray-projected shadow sample.
                let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                let cell = self.stable_light_cell(lane.light_key(i), x, z);
                let block = match cell {
                    Some(c) => self.light_grid.block_of(c),
                    None => &empty_block,
                };
                let split = merge_transients_into_block(
                    block,
                    &self.frame_lights,
                    self.frame_baked_count..self.frame_lights.len(),
                    vec3f(x, inst.transform.v[13], z),
                    &mut self.light_rank,
                    &mut self.light_block_scratch,
                );
                write_light_block(cx.cx, &mut draw.base().draw_vars, &self.light_block_scratch, split);
                draw.base().ground_y = match lane {
                    WorldModelLane::Placed => self.model_ground.get(i),
                    WorldModelLane::Attachment => self.world_attachment_ground.get(i),
                }
                .copied()
                .unwrap_or(0.0);
                dynamic_block_active = true;
            } else if dynamic_block_active {
                write_light_block(cx.cx, &mut draw.base().draw_vars, &static_block, static_split);
                draw.base().ground_y = 0.0;
                dynamic_block_active = false;
            }
            if last.as_deref() != Some(inst.model.as_str()) {
                match lane {
                    WorldModelLane::Placed => stats.model_draws += 1,
                    WorldModelLane::Attachment => stats.world_attachment_draws += 1,
                }
                last = Some(inst.model.clone());
            }
            match lane {
                WorldModelLane::Placed => {
                    stats.model_instances += 1;
                    stats.model_triangles += tri_count;
                }
                WorldModelLane::Attachment => {
                    stats.world_attachment_instances += 1;
                    stats.world_attachment_triangles += tri_count;
                }
            }
            for (geometry_id, texture, detail, dscale, material) in &layer_draws {
                draw.base().draw_vars.geometry_id = Some(*geometry_id);
                draw.base().draw_vars.set_texture(0, texture);
                draw.base().draw_vars.set_texture(5, detail);
                draw.base().detail_st = vec2f(dscale[0], dscale[1]);
                draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                draw.set_material(material);
                if draw.base().draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draw.base().draw_vars);
                    draw.base().draw_vars.area = cx.update_area_refs(draw.base().draw_vars.area, new_area);
                }
            }
            // Rigid parts (doors, lifts). Each is one extra draw on the
            // PARENT's material — same shader, same textures, usually the
            // level tile it was cut from — placed at the instance transform
            // times where the part's state machine has it right now.
            //
            // A part carries no baked chart (lm_rect zeroed): it moves, so
            // the static atlas never had a window for it. Its shadow comes
            // from the realtime cascades, which see it as a mover.
            let parts: Vec<(Mat4f, usize, Vec<(GeometryId, Texture, Texture, [f32; 2])>)> = {
                let m = &self.static_models[at].1;
                let mut parts = Vec::with_capacity(m.anim_parts.len() + m.driven_parts.len());
                if !m.anim_parts.is_empty() {
                    let key = match lane {
                        WorldModelLane::Placed => ModelTarget::Instance(i),
                        // Attachments are not placed slots — a slot index
                        // would address someone else's instance — so their
                        // parts follow the per-MODEL command only.
                        WorldModelLane::Attachment => ModelTarget::Model(inst.model.clone()),
                    };
                    parts.extend(m.anim_parts.iter().map(|p| {
                            let (_, time, _) =
                                self.model_anim_state.clock(&key, &inst.model, &p.def);
                            (
                                p.def.transform_at(time),
                                p.def.indices.len() / 3,
                                p.draws
                                    .iter()
                                    .map(|(g, t, d, s)| {
                                        (g.geometry_id(), t.clone(), d.clone(), *s)
                                    })
                                    .collect(),
                            )
                        }));
                }
                parts.extend(m.driven_parts.iter().map(|part| {
                    let pose = inst
                        .part_poses
                        .iter()
                        .find(|pose| pose.connection == part.def.connection)
                        .map(|pose| pose.transform)
                        .unwrap_or_else(|| part.def.rest_transform());
                    (
                        pose,
                        part.def.indices.len() / 3,
                        part
                            .draws
                            .iter()
                            .map(|(g, t, d, s)| (g.geometry_id(), t.clone(), d.clone(), *s))
                            .collect(),
                    )
                }));
                parts
            };
            for (pose, part_tris, part_draws) in &parts {
                draw.base().transform = Mat4f::mul(&inst.transform, pose);
                draw.base().lm_rect = Vec4f::default();
                for (geometry_id, texture, detail, dscale) in part_draws {
                    draw.base().draw_vars.geometry_id = Some(*geometry_id);
                    draw.base().draw_vars.set_texture(0, texture);
                    draw.base().draw_vars.set_texture(5, detail);
                    draw.base().detail_st = vec2f(dscale[0], dscale[1]);
                    draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                    draw.set_material(&part_material);
                    if draw.base().draw_vars.can_instance() {
                        let new_area = cx.add_instance(&draw.base().draw_vars);
                        draw.base().draw_vars.area = cx.update_area_refs(draw.base().draw_vars.area, new_area);
                    }
                }
                match lane {
                    WorldModelLane::Placed => stats.model_triangles += part_tris,
                    WorldModelLane::Attachment => {
                        stats.world_attachment_triangles += part_tris
                    }
                }
            }
            // A part opened its own draw item; the next instance of the same
            // model must re-bind its own transform rather than accumulate
            // into the part's.
            if !parts.is_empty() {
                last = None;
            }
        }
    }

    /// The specular half of the model pass: the same instance list, walked
    /// again for the models whose material carries shininess.
    ///
    /// The shader is the renderer's own, built on first use rather than lent
    /// through `SceneDraws` (the `sky_draw` pattern). That is deliberate: a
    /// host does not opt into PBR, a MODEL does, and every existing host —
    /// VJ, sandbox, the thumbnailer — gets the lane the moment it loads a
    /// shiny GLB without touching its widget or its script.
    ///
    /// Costs nothing when the scene has no shiny models: `wants_pbr` is a
    /// bool on an already-resident struct, so the early-out below is one
    /// linear scan of the instance list and no GPU work at all.
    #[allow(clippy::too_many_arguments)]
    fn draw_pbr_models(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        instances: &[ModelInstance],
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if !self.pbr_materials_enabled {
            return;
        }
        let any = instances.iter().any(|inst| {
            self.static_models
                .iter()
                .any(|(k, m)| *k == inst.model && m.wants_pbr)
        });
        if !any {
            return;
        }
        if self.pbr_draw.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing with no shader.
            self.pbr_draw = cx
                .cx
                .try_with_vm(|vm| Box::new(DrawScenePbr::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.pbr_draw.take() else {
            return;
        };
        self.draw_models_inner(
            cx,
            ModelDraw::Pbr(&mut draw),
            eye,
            instances,
            lane,
            fog,
            sun,
            frustum,
            stats,
        );
        self.pbr_draw = Some(draw);
    }

    /// Draw every placed map's sky surfaces.
    ///
    /// One draw item per map: the faces are already one geometry, and the
    /// whole point of the lane is that a sky costs the same whether it is
    /// two brushes or two hundred. Depth is written normally — the faces sit
    /// where the level put them — but nothing else about the world reaches
    /// them: no lightmap window, no cascades, no fog, no sun.
    fn draw_sky_faces(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if !self
            .placed_models
            .iter()
            .any(|inst| self.model_sky(&inst.model).is_some())
        {
            return;
        }
        if self.sky_draw.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing a sky with no shader.
            self.sky_draw = cx
                .cx
                .try_with_vm(|vm| Box::new(DrawSceneSkyMap::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.sky_draw.take() else {
            return;
        };
        let time = self.sky_time;
        let instances = std::mem::take(&mut self.placed_models);
        for inst in &instances {
            let Some((_, loaded)) = self.static_models.iter().find(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let Some(sky) = &loaded.sky else { continue };
            // A map whose sky is entirely behind the camera pays nothing.
            // Per-FACE culling is deliberately not attempted: the faces are
            // one geometry precisely so a sky costs one draw.
            if let Some(frustum) = frustum {
                if !frustum.intersects_obb(sky.part.min, sky.part.max, &inst.transform) {
                    stats.model_culled += 1;
                    continue;
                }
            }
            draw.transform = inst.transform;
            draw.depth_clip = 1.0;
            draw.eye = vec4(eye.x, eye.y, eye.z, 0.0);
            draw.sky_p = vec4(
                sky.part.projection.code(),
                sky.part.repeat,
                sky.part.scroll(0, time),
                sky.part.scroll(1, time),
            );
            draw.sky_q = vec4(sky.part.v_span, 1.0, 0.0, 0.0);
            draw.draw_vars.geometry_id = Some(sky.geometry.geometry_id());
            draw.draw_vars.set_texture(0, &sky.tex0);
            draw.draw_vars.set_texture(1, &sky.tex1);
            if draw.draw_vars.can_instance() {
                let new_area = cx.add_instance(&draw.draw_vars);
                draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
            }
            stats.model_draws += 1;
            stats.model_triangles += sky.part.triangle_count();
        }
        self.placed_models = instances;
        self.sky_draw = Some(draw);
    }

    /// Draw the private FPS presentation list. This path deliberately does
    /// not share world-model setup: no AO/lightmap/top-map/CSM bindings, no
    /// dynamic-light selection, no frustum/caster/receiver work. A typical
    /// list is one 70-triangle pistol and therefore one draw item.
    fn draw_view_models_inner(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut DrawSceneViewModel,
        sun: &SunLight,
        stats: &mut RenderStats,
    ) {
        if self.view_models.is_empty() {
            return;
        }
        sun.write_into(
            &mut draw.light_dir,
            &mut draw.sun_color,
            &mut draw.sun_sky,
            &mut draw.sun_ground,
        );
        let stage_inv = self.stage.matrix().invert();
        for inst in &self.view_models {
            let Some((_, loaded)) = self.static_models.iter().find(|(id, _)| *id == inst.model)
            else {
                continue;
            };
            let layer_draws = {
                let mut layers = Vec::with_capacity(1 + loaded.extra_draws.len());
                layers.push((loaded.geometry.geometry_id(), loaded.texture.clone()));
                for (g, t, _, _, _) in &loaded.extra_draws {
                    layers.push((g.geometry_id(), t.clone()));
                }
                layers
            };
            // The scene draw-list carries Stage for every world draw. Cancel
            // it here: a held model is attached to the physical camera, not
            // scaled into an MR diorama or moved onto its floor anchor.
            draw.transform = Mat4f::mul(&stage_inv, &inst.transform);
            for (geometry_id, texture) in &layer_draws {
                draw.draw_vars.geometry_id = Some(*geometry_id);
                draw.draw_vars.set_texture(0, texture);
                if draw.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draw.draw_vars);
                    draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
                }
            }
            stats.view_model_instances += 1;
            stats.view_model_triangles += loaded.triangles;
        }
    }

    /// Is this rig's rest mesh resident? See [`Self::upload_skin_rig`].
    pub fn skin_rig_loaded(&self, rig: u64) -> bool {
        self.skin_rig_geometries.iter().any(|(k, _, _)| *k == rig)
    }

    /// Upload a rig's GPU rest bundle once (`SkinnedModel::rest_gpu`): the
    /// chart-split rest mesh plus its rest-pose AO atlas as an R8 texture.
    /// Every character wearing the rig shares both; the per-frame upload for
    /// a character is its joint palette. Idempotent per rig key.
    pub fn upload_skin_rig(&mut self, cx: &mut Cx, rig: u64, rest: crate::skin::SkinRestGpu) {
        if self.skin_rig_loaded(rig) {
            return;
        }
        let geometry = Geometry::new(cx);
        geometry.update(cx, rest.indices, rest.vertices);
        let ao_map = Texture::new_with_format(
            cx,
            TextureFormat::VecRu8 {
                width: rest.ao_size,
                height: rest.ao_size,
                data: Some(rest.ao_pixels),
                unpack_row_length: None,
                updated: TextureUpdated::Full,
            },
        );
        self.skin_rig_geometries.push((rig, geometry, ao_map));
    }

    /// Upload one delivered SDF atlas as an R8 texture + its addressing
    /// meta. The runtime cost of a caster's whole shadow tier is this
    /// texture plus one instanced quad per caster per frame.
    fn upload_sdf_atlas(
        cx: &mut Cx,
        atlas: crate::shadow_sdf::ShadowSdfAtlas,
    ) -> (Texture, SdfMeta) {
        let (w, h) = (atlas.width(), atlas.height());
        let meta = SdfMeta {
            rect: vec4(atlas.rect.0, atlas.rect.1, atlas.rect.2, atlas.rect.3),
            rows: atlas.rows,
            band_world: atlas.band_world,
            len_per_unit: atlas.len_per_unit,
        };
        let tex = Texture::new_with_format(
            cx,
            TextureFormat::VecRu8 {
                width: w,
                height: h,
                data: Some(atlas.pixels),
                unpack_row_length: None,
                updated: TextureUpdated::Full,
            },
        );
        (tex, meta)
    }

    /// The offline `.shadowsdf` sidecar for `glb` (tools/ao_bake), if it can
    /// be trusted. Three gates, all falling back to the off-thread bake
    /// rather than erroring:
    ///  - FRESH: sidecar mtime newer than the glb's (the `.aomesh` rule) —
    ///    a re-exported model must not keep its old silhouette.
    ///  - KEYED: `expect_hash` (a rig's rest hash, the `.skinao` scheme)
    ///    must match when the caller has one.
    ///  - THIS SUN: the bake projects at the sun's elevation
    ///    (`len_per_unit`), so a sidecar baked for a different sun would
    ///    draw every shadow at the wrong length. The tolerance only absorbs
    ///    normalisation ulps, not a different sky.
    /// A baked silhouette serves ANY sun whose shadow length is within a
    /// sane stretch of the baked one — the instance build stretches the
    /// sample window along the sun axis by the ratio (play-session-1 entry
    /// 18: exact-length matching rejected every sidecar the moment a level
    /// authored its own `time_of_day`, and the whole dynamic tier fell to
    /// blobs). Past the band the stretch distorts (a noon bake pulled to a
    /// sunset length smears) — those keep the blob tier.
    fn sun_len_compatible(baked: f32, now: f32) -> bool {
        baked > 0.0 && now > 0.0 && (0.2..=5.0).contains(&(now / baked))
    }

    fn load_shadow_sdf_sidecar(
        sidecar: &std::path::Path,
        glb: &std::path::Path,
        expect_hash: Option<u64>,
        sun: &SunLight,
    ) -> Option<crate::shadow_sdf::ShadowSdfAtlas> {
        // Staleness by mtime is only meaningful for checkout files with no
        // recorded identity. A caller that KNOWS the expected content hash
        // (store-streamed rigs — the cache writes both files at arbitrary
        // times) must not lose its shadows to write ordering.
        if expect_hash.is_none() {
            let stamp =
                |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            if stamp(sidecar)? <= stamp(glb)? {
                return None;
            }
        }
        let (atlas, hash) =
            crate::shadow_sdf::ShadowSdfAtlas::from_shadowsdf(&std::fs::read(sidecar).ok()?)?;
        if expect_hash.is_some_and(|h| h != hash) {
            return None;
        }
        if !Self::sun_len_compatible(atlas.len_per_unit, sun.shadow_len_per_unit()) {
            return None;
        }
        Some(atlas)
    }

    /// Resolve each rig's SDF atlas from its offline `.shadowsdf` sidecar
    /// (tools/ao_bake), once per rig per sun era. There is NO runtime
    /// silhouette baking — the sidecar IS the tier: a fresh, hash-matched,
    /// same-sun file uploads; anything else caches a `None` and the rig's
    /// characters keep the blob tier (re-tried only when the sun changes —
    /// [`Self::sdf_baked_sun_len`]).
    fn seed_skinned_sdf(&mut self, cx: &mut Cx, items: &[SkinnedDraw], sun: &SunLight) {
        for item in items {
            if self.sdf_atlas_tex.iter().any(|(r, _)| *r == item.rig) {
                continue;
            }
            let payload = item.sdf_sidecar.as_ref().and_then(|(glb, hash)| {
                let sidecar = std::path::PathBuf::from(format!("{glb}.shadowsdf"));
                Self::load_shadow_sdf_sidecar(
                    &sidecar,
                    std::path::Path::new(glb),
                    Some(*hash),
                    sun,
                )
            });
            let payload = match payload {
                Some(atlas) => {
                    log!(
                        "shadow sdf: rig {} atlas from sidecar ({}x{} R8, {} rows, {} bytes)",
                        item.rig,
                        atlas.width(),
                        atlas.height(),
                        atlas.rows,
                        atlas.pixels.len()
                    );
                    Some(Self::upload_sdf_atlas(cx, atlas))
                }
                None => {
                    log!(
                        "shadow sdf: rig {} has no loadable sidecar for this sun — blob tier \
                         (bake one with tools/ao_bake)",
                        item.rig
                    );
                    None
                }
            };
            self.sdf_atlas_tex.push((item.rig, payload));
        }
    }

    /// Resolve a dynamic MODEL's yaw-only SDF atlas from its offline
    /// `.shadowsdf` sidecar, once per model per sun era — the rig rule
    /// above, keyed by asset id ([`Self::models_root`]) instead of rest
    /// hash. No sidecar = its instances keep the blob tier.
    fn seed_model_sdf(&mut self, cx: &mut Cx, key: &str, sun: &SunLight) {
        if self.model_sdf_tex.contains_key(key) {
            return;
        }
        // Bytes that arrived with the model (asset store) outrank the
        // checkout sidecar; they carry no mtime, only the sun gate applies.
        let streamed = self.model_sdf_bytes.get(key).and_then(|bytes| {
            let (atlas, _hash) = crate::shadow_sdf::ShadowSdfAtlas::from_shadowsdf(bytes)?;
            Self::sun_len_compatible(atlas.len_per_unit, sun.shadow_len_per_unit())
                .then_some(atlas)
        });
        let glb = Self::models_root().join(format!("{key}.glb"));
        let sidecar = Self::models_root().join(format!("{key}.shadowsdf"));
        let payload = match streamed.or_else(|| Self::load_shadow_sdf_sidecar(&sidecar, &glb, None, sun)) {
            Some(atlas) => {
                log!(
                    "shadow sdf: model {} atlas from sidecar ({}x{} R8, {} rows, {} bytes)",
                    key,
                    atlas.width(),
                    atlas.height(),
                    atlas.rows,
                    atlas.pixels.len()
                );
                Some(Self::upload_sdf_atlas(cx, atlas))
            }
            None => {
                log!(
                    "shadow sdf: model {key} has no loadable sidecar for this sun — blob tier \
                     (bake one with tools/ao_bake)"
                );
                None
            }
        };
        self.model_sdf_tex.insert(key.to_string(), payload);
    }

    /// Pack EVERY character's joint palette into the shared RGBA32F texture,
    /// once per frame, BEFORE the GPU lightmap encodes its passes: the
    /// bake's skinned depth passes and the visible skinned draw bind this
    /// same texture, so a character shadows in exactly the pose it draws
    /// in. Fills [`Self::skin_joint_bases`] (-1 = no palette).
    ///
    /// Deliberately un-culled: an off-screen character still casts into a
    /// visible region (Realtime), and a palette is ~2 KB — the whole
    /// village's worth is one small upload.
    fn pack_skin_palettes(
        &mut self,
        cx: &mut Cx,
        items: &[SkinnedDraw],
        stats: &mut RenderStats,
    ) {
        self.skin_joint_bases.clear();
        // Power-of-two width and height keep texel-centre uvs exact in f32;
        // capacity only grows, so a steady crowd re-uses the allocation.
        let needed: usize = items
            .iter()
            .map(|i| i.palette.len() * crate::skin::PALETTE_TEXELS_PER_JOINT)
            .sum();
        if needed == 0 {
            self.skin_joint_bases.resize(items.len(), -1.0);
            return;
        }
        let rows = needed.div_ceil(JOINT_TEX_WIDTH).next_power_of_two();
        let capacity = JOINT_TEX_WIDTH * rows;
        if self.skin_palette_texels < capacity {
            self.skin_palette_tex = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: JOINT_TEX_WIDTH,
                    height: rows,
                    data: Some(vec![0.0; capacity * 4]),
                    updated: TextureUpdated::Full,
                },
            ));
            self.skin_palette_texels = capacity;
        }
        let palette_tex = self.skin_palette_tex.clone().unwrap();
        // The texture OBJECT binds per consumer; its DATA uploads at pass
        // encode, after this has finished writing every palette.
        let mut texels = palette_tex.take_vec_f32(cx);
        texels.clear();
        for item in items {
            if item.palette.is_empty() {
                self.skin_joint_bases.push(-1.0);
                continue;
            }
            self.skin_joint_bases.push((texels.len() / 4) as f32);
            crate::skin::palette_texels(&item.palette, &mut texels);
        }
        stats.skin_upload_bytes += (texels.len() * 4) as u64;
        // The texture's storage is its full capacity.
        texels.resize(self.skin_palette_texels * 4, 0.0);
        palette_tex.put_back_vec_f32(cx, texels, None);
    }

    /// Draw the skinned batch inside the already-open scene pass.
    ///
    /// GPU skinning: each rig's rest mesh is resident; every character's
    /// joint palette is packed into ONE RGBA32F texture per frame
    /// ([`Self::pack_skin_palettes`]) and blended in the vertex shader.
    /// Characters of one rig sort adjacent so they accumulate into a single
    /// draw item — the per-frame cost of a crowd is its palettes plus an
    /// instance record each, not its vertices.
    fn draw_skinned_inner(
        &mut self,
        cx: &mut Cx3d,
        batch: SkinnedBatch,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        sun.write_into(
            &mut batch.skinned.light_dir,
            &mut batch.skinned.sun_color,
            &mut batch.skinned.sun_sky,
            &mut batch.skinned.sun_ground,
        );
        // Dynamic lights are PER-CHARACTER: each looks up its own
        // precomputed grid cell (O(1), hysteresis-stable — light_grid.rs)
        // inside the draw loop below, so a villager standing under a lamp
        // holds a byte-identical block frame after frame and cannot
        // flicker. Characters sharing a cell (and rig and atlas) still
        // merge into one draw item — the append test compares uniforms.
        // The baked ground light field: characters walking through a house's
        // shadow darken (A channel gates their direct sun; RGB is never read
        // here — lamps arrive through the dl_* array).
        {
            let lm_tex = self.lightmap_texture(cx.cx);
            batch.skinned.draw_vars.set_texture(3, &lm_tex);
            // The shadow-top plane: heads clear a fence rail's shadow.
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            batch.skinned.draw_vars.set_texture(4, &top_tex);
            {
                let csm = self.gpu_baker.csm_binding();
                Self::write_csm_uniforms(
                    cx.cx,
                    &mut batch.skinned.draw_vars,
                    &csm,
                    &lm_tex,
                    5,
                );
            }
            batch.skinned.draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_top_decode),
                &[top_base, top_range, 0.0, 0.0],
            );
            let (g_rect, g_world) = self.lm_ground.unwrap_or_default();
            batch.skinned.draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_rect),
                &[g_rect.x, g_rect.y, g_rect.z, g_rect.w],
            );
            batch.skinned.draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_world),
                &[g_world.x, g_world.y, g_world.z, g_world.w],
            );
        }
        // Palettes were packed by pack_skin_palettes BEFORE the GPU lightmap
        // ran (its skinned depth passes bind the same texture). Cloned
        // handle: the loop below needs `&mut self` for the cell lookup.
        let Some(palette_tex) = self.skin_palette_tex.clone() else {
            return;
        };

        // Copies of one rig sort adjacent: consecutive add_instance calls
        // with unchanged geometry and textures merge into one draw item.
        let mut order: Vec<usize> = (0..batch.items.len()).collect();
        order.sort_by_key(|i| (batch.items[*i].rig, batch.items[*i].texture));
        for i in order {
            let item = &batch.items[i];
            let Some(base) = self.skin_joint_bases.get(i).copied().filter(|b| *b >= 0.0)
            else {
                continue;
            };
            let Some(at) = self
                .skin_rig_geometries
                .iter()
                .position(|(k, _, _)| *k == item.rig)
            else {
                continue;
            };
            // Cull BEFORE packing the palette — an offscreen character then
            // costs its pose math and nothing else. Bounds come from the
            // joint spheres (posed_bounds), conservative for any pose.
            if let (Some(frustum), Some((min, max))) = (frustum, item.bounds) {
                if !frustum.intersects_obb(min, max, &item.transform) {
                    stats.skinned_culled += 1;
                    continue;
                }
            }
            // This character's OWN light block (cell lookup + strength
            // merge — light_grid.rs), written BEFORE the draw item opens so
            // the capture is deterministic. Characters sharing a cell (and
            // rig/atlas) still merge into one item.
            {
                let (x, z) = (item.transform.v[12], item.transform.v[14]);
                let cell = self.stable_light_cell(0x8000_0000_0000_0000 | item.key, x, z);
                let block = match cell {
                    Some(c) => self.light_grid.block_of(c),
                    // (-1,-1) is out of range: the empty block.
                    None => self.light_grid.block_of((-1, -1)),
                };
                let split = merge_transients_into_block(
                    block,
                    &self.frame_lights,
                    self.frame_baked_count..self.frame_lights.len(),
                    vec3f(x, item.transform.v[13], z),
                    &mut self.light_rank,
                    &mut self.light_block_scratch,
                );
                write_light_block(
                    cx.cx,
                    &mut batch.skinned.draw_vars,
                    &self.light_block_scratch,
                    split,
                );
            }
            // Ground plane under this character, for the sun-ray-projected
            // baked-shadow sample (OnChange; the Realtime cascades need no
            // ground plane and no own-shadow bookkeeping — a character
            // simply is one more caster and receiver in the maps).
            batch.skinned.ground_y = self.char_ground.get(i).copied().unwrap_or(0.0);
            batch.skinned.joint_base = base;
            batch.skinned.draw_vars.geometry_id =
                Some(self.skin_rig_geometries[at].1.geometry_id());
            batch.skinned.transform = item.transform;
            batch.skinned.tint = item.tint;
            batch.skinned.depth_clip = 1.0;
            batch.skinned.fog_color = fog.0;
            batch.skinned.fog_density = fog.1;
            // Clamp rather than index blindly: a bad texture index would
            // otherwise panic mid-frame, and a character wearing the wrong
            // atlas is a visible bug worth surviving to see.
            if let Some(tex) = batch
                .textures
                .get(item.texture)
                .or_else(|| batch.textures.first())
            {
                batch.skinned.draw_vars.set_texture(0, tex);
            }
            batch.skinned.draw_vars.set_texture(1, &palette_tex);
            // The rig's rest-pose AO atlas — per rig, so it changes exactly
            // when the geometry does and never breaks the rig batching.
            batch.skinned.draw_vars.set_texture(2, &self.skin_rig_geometries[at].2);
            if batch.skinned.draw_vars.can_instance() {
                let new_area = cx.add_instance(&batch.skinned.draw_vars);
                batch.skinned.draw_vars.area =
                    cx.update_area_refs(batch.skinned.draw_vars.area, new_area);
            }
        }
    }

    /// Hysteresis-stable grid cell for a dynamic object: keeps the object's
    /// previous cell while it is still inside it plus a 1-unit margin, so
    /// dithering on a cell line never flaps its light block. `id` must be
    /// stable across frames (character key / placed-model index).
    fn stable_light_cell(&mut self, id: u64, x: f32, z: f32) -> Option<(i32, i32)> {
        if let Some(prev) = self.light_cell_memory.get(&id) {
            if self.light_grid.cell_still_fits(*prev, x, z, 1.0) {
                return Some(*prev);
            }
        }
        let cell = self.light_grid.cell_of(x, z)?;
        self.light_cell_memory.insert(id, cell);
        Some(cell)
    }

    /// Where a character's soft shadow ROOTS, which way it points and how
    /// strong it is. ONE function owns the whole anchor policy, so the
    /// GPU-instanced quad and the CPU fallback cannot drift apart:
    ///
    /// * The ROOT — the silhouette's foot end — is the caster's own ground
    ///   contact. A shadow is attached to its caster there, and no light
    ///   direction change can detach it: leans redirect the BODY of the
    ///   shadow, never its contact.
    /// * Airborne (`h` = feet above the local ground) the root slides with
    ///   the height-driven part of the projection — anti-sun
    ///   `-sun.xz / max(sun.y, 0.2) · h`, lamp `ρ·h/(bulb−mid)` — a
    ///   jumping character's shadow moves off their feet, which is the
    ///   whole visual point of a jump shadow.
    /// * A nearby lamp claims the LEAN continuously: dominance is the
    ///   lamp's share of light at the feet (biased ×2 — a near, low light
    ///   owns the local shadow well before it strictly outshines the sky),
    ///   never a hard sun-wins threshold. The lean is the true projection
    ///   from the bulb — `ρ·(mid+h)/(bulb−mid)` — and supplies the quad's
    ///   long-axis DIRECTION; at night the sun term dies and the lamp
    ///   saturates.
    /// * The landing ground is re-sampled AT THE ROOT, so jumping beside a
    ///   slab lands the shadow on the slab.
    /// * Alpha fades and the sprite swells with height (a soft shadow weakens
    ///   and spreads with occluder distance — the M64 treatment); a
    ///   dominant lamp darkens it back and compresses it toward the root.
    ///
    /// `None` = out of shadow range, draw nothing.
    fn character_shadow_anchor(
        &self,
        feet: Vec3f,
        receiver: &Receiver,
        sun: &SunLight,
    ) -> Option<ShadowAnchor> {
        character_shadow_anchor(feet, receiver, sun, &self.frame_lights)
    }

    /// Encode the whole 3D scene for one view. `draw_list` is the host's
    /// scene draw list (begun/ended here, exactly as before the move).
    pub fn draw_scene(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut SceneDraws,
        world: &GameWorld,
        scene_state: SceneState3D,
    ) -> RenderStats {
        self.draw_scene_full(cx, draw_list, draws, world, scene_state, None, None)
    }

    /// [`draw_scene`] plus an optional skinned-character batch.
    pub fn draw_scene_full(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut SceneDraws,
        world: &GameWorld,
        scene_state: SceneState3D,
        skinned: Option<SkinnedBatch>,
        mut models_draw: Option<&mut DrawSceneSkinned>,
    ) -> RenderStats {
        let mut stats = RenderStats::default();
        let camera_pos = scene_state.camera_pos;
        let stage_matrix = self.stage.matrix();
        // CPU frustum for per-frame instance culling AND the Realtime
        // bake's visible-region scheduling, in the same world units the
        // instance transforms use (the stage rides inside the clip matrix).
        // Flat stage only: in XR the runtime overwrites the pass camera
        // with its own eye matrices every frame (see stage.rs), so
        // scene_state is not what renders there and culling from it could
        // drop visible geometry. Only per-frame packing points consult
        // this — cached static slabs are never culled, because they outlive
        // the camera that built them.
        let frustum_val = (self.stage.mode == StageMode::Flat).then(|| {
            Frustum::from_clip_matrix(&Mat4f::mul(
                &scene_state.projection,
                &Mat4f::mul(&scene_state.view, &stage_matrix),
            ))
        });
        let frustum = frustum_val.as_ref();
        // Which tier serves the DYNAMIC casters this frame — the ONE
        // decision point (gpu_lightmap::dynamic_shadow_tiers) both the
        // draw gates below and the mover collection consume, so the
        // settings/F8 mode switch flips the complete contract atomically.
        let tiers = crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode());
        // Character palettes pack BEFORE the cascades encode: the skinned
        // depth passes bind the same texture the visible draw does.
        match &skinned {
            Some(batch) => self.pack_skin_palettes(cx.cx, &batch.items, &mut stats),
            None => self.skin_joint_bases.clear(),
        }
        let lm_movers = if tiers.csm {
            self.ensure_lm_box_geometry(cx.cx);
            self.collect_lm_movers(world, skinned.as_ref().map(|b| b.items.as_slice()))
        } else {
            Vec::new()
        };
        // The camera slice the Realtime cascades fit to: the far-plane
        // corners of THIS view, unprojected (far = clip z +w in every
        // backend's convention). Flat stage only — in XR the runtime owns
        // the eye matrices, and the baker falls back to eye-centered rings.
        let csm_view = (self.stage.mode == StageMode::Flat)
            .then(|| {
                let clip = Mat4f::mul(
                    &scene_state.projection,
                    &Mat4f::mul(&scene_state.view, &stage_matrix),
                );
                let inv = clip.invert();
                let corner = |x: f32, y: f32| {
                    let p = inv.transform_vec4(vec4(x, y, 1.0, 1.0));
                    if p.w.abs() < 1.0e-9 {
                        None
                    } else {
                        Some(vec3f(p.x / p.w, p.y / p.w, p.z / p.w))
                    }
                };
                Some(crate::shadow_csm::CsmView {
                    cam: camera_pos,
                    far_corners: [
                        corner(-1.0, -1.0)?,
                        corner(1.0, -1.0)?,
                        corner(-1.0, 1.0)?,
                        corner(1.0, 1.0)?,
                    ],
                    focus_distance: self.csm_focus.unwrap_or(0.0),
                })
            })
            .flatten();
        // The GPU light bake + cascades: encode this frame's passes (they
        // render BEFORE this scene pass, so a delivered atlas / fresh
        // cascade set is never sampled stale — no readback, no upload).
        self.run_gpu_lightmap(cx.cx, world, &lm_movers, csm_view.as_ref(), camera_pos);
        draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(stage_matrix);
        // The stage rides on the draw list's view transform: every game
        // shader computes `draw_list.view_transform * transform`, so one
        // uniform scales and anchors sky, terrain, cubes and characters
        // together. The instance transforms below stay in world units, which
        // is why the cached static slabs survive a stage change untouched.
        // (The view matrix cannot carry this — in XR the runtime overwrites
        // camera_view/_r with its own eye matrices every frame.)
        cx.cx.draw_lists[draw_list.id()]
            .draw_list_uniforms
            .view_transform = stage_matrix;
        // MR puts the game on your real floor: the room supplies the
        // horizon, so the game's own environment would only paint over the
        // passthrough feed.
        let shows_environment = self.stage.shows_environment();

        // One sun for every shader this frame (sun.rs). Written before any
        // batch begins, because instance fields are snapshotted per draw and
        // uniforms are captured when the draw item opens.
        let sun = crate::sun::resolve_sun(&world.sun);

        // SDF-atlas sun era: the sidecars bake against one sun elevation.
        // An EXPLICIT sun change (OnChange's only kind — the day cycle
        // forces Realtime, where the SDF tier is off) drops the caches and
        // re-tries the sidecars against the new sun; a caster whose sidecar
        // disagrees falls to the blob tier. No runtime re-bake exists.
        {
            let len = sun.shadow_len_per_unit();
            if (len - self.sdf_baked_sun_len).abs() > 1.0e-3 {
                self.sdf_baked_sun_len = len;
                self.sdf_atlas_tex.clear();
                self.model_sdf_tex.clear();
            }
        }

        // The analytic (Preetham) sky replaces the hand-painted gradient
        // wherever the script kept the DEFAULT dome colours — a script that
        // authored its own top/ground colours keeps its gradient. The
        // horizon colour stays customisable either way: it only ever tinted
        // the FOG, and in analytic mode the fog instead comes from the
        // model's own tone-mapped horizon so sky and haze agree.
        let sky_frame = world
            .sky
            .as_ref()
            .filter(|s| shows_environment && sky_wants_analytic(s))
            .map(|sky| {
                let turbidity = if sky.turbidity.is_finite() {
                    sky.turbidity
                } else {
                    sky_turbidity()
                };
                let compensation = if sky.exposure_ev.is_finite() {
                    2.0f32.powf(sky.exposure_ev.clamp(-12.0, 12.0))
                } else {
                    1.0
                };
                crate::sky::preetham_frame(
                    sun.dir,
                    turbidity,
                    sky_exposure() * compensation,
                )
            });

        // Fog only exists once the script asked for a sky.
        let (fog_color, fog_density) = match &world.sky {
            Some(sky) if shows_environment => {
                let c = sky_frame
                    .as_ref()
                    .map(|frame| frame.fog_rgb)
                    .unwrap_or(vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z));
                (c, sky.fog)
            }
            _ => (vec3(0.75, 0.87, 0.96), 0.0),
        };
        apply_sun(cx.cx, draws, &sun, fog_color);
        // The ground light field for the cube family (statics AND dynamics —
        // a mover crossing a baked shadow should darken). Uniforms, not
        // instance lanes: the packed slab layout stays untouched.
        {
            let lm_tex = self.lightmap_texture(cx.cx);
            // The shadow-top plane: a raised slab or ramp top compares its
            // fragment height against the height the shadow was measured
            // at, instead of wearing the grass-level shadow under it.
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            let (lm_rect, lm_world) = self.lm_ground.unwrap_or_default();
            let csm = self.gpu_baker.csm_binding();
            for dv in [
                &mut draws.cube.cube.draw_vars,
                &mut draws.alpha.cube.cube.draw_vars,
            ] {
                dv.set_texture(0, &lm_tex);
                dv.set_texture(1, &top_tex);
                Self::write_csm_uniforms(cx.cx, dv, &csm, &lm_tex, 2);
                dv.set_uniform(
                    cx.cx,
                    live_id!(lm_rect),
                    &[lm_rect.x, lm_rect.y, lm_rect.z, lm_rect.w],
                );
                dv.set_uniform(
                    cx.cx,
                    live_id!(lm_world),
                    &[lm_world.x, lm_world.y, lm_world.z, lm_world.w],
                );
                dv.set_uniform(
                    cx.cx,
                    live_id!(lm_top_decode),
                    &[top_base, top_range, 0.0, 0.0],
                );
            }
        }
        // This frame's dynamic light list (lamps + firework flashes + host
        // lights), then the TRANSIENT slice for the world-spanning shaders.
        // Cube slabs and terrain sum lights per PIXEL and receive only the
        // transients — their street-lamp light is already baked into the
        // atlas RGB, and adding it analytically would double-light every
        // static surface. Written before any of their draw items open.
        self.build_frame_lights(&sun);
        {
            let transients = self.frame_baked_count..self.frame_lights.len();
            select_lights_for_world(
                &self.frame_lights,
                transients,
                camera_pos,
                frustum,
                &mut self.light_rank,
                &mut self.light_sel,
            );
            for dv in [
                &mut draws.cube.cube.draw_vars,
                &mut draws.alpha.cube.cube.draw_vars,
                &mut draws.terrain.draw_vars,
            ] {
                write_light_uniforms(
                    cx.cx,
                    dv,
                    &self.frame_lights,
                    &self.light_sel,
                    self.light_sel.len(),
                );
            }
        }

        // 1. Sky dome around the camera (depth-tested at radius, drawn
        // first). Default-sky worlds draw the ANALYTIC dome (Preetham +
        // setting sun + stars); authored gradients keep DrawSceneSky.
        if let Some(sky) = world.sky.as_ref().filter(|_| shows_environment) {
            let mut transform = Mat4f::identity();
            transform.v[12] = camera_pos.x;
            transform.v[13] = camera_pos.y;
            transform.v[14] = camera_pos.z;
            match (&sky_frame, &mut draws.sky_analytic) {
                (Some(f), Some(sa)) => {
                    sa.cube.transform = transform;
                    sa.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    sa.cube.cube_size = vec3(800.0, 800.0, 800.0);
                    sa.cube.color = vec4(1.0, 1.0, 1.0, 1.0);
                    sa.cube.depth_clip = 1.0;
                    sa.pz_y = f.pz_y;
                    sa.pz_x = f.pz_x;
                    sa.pz_yc = f.pz_yc;
                    sa.pz_e = f.pz_e;
                    sa.pz_f0 = f.pz_f0;
                    sa.zenith = f.zenith;
                    sa.sun_e = f.sun;
                    sa.sun_true = f.sun_true;
                    // The star dome: panorama texture (or the 1x1 black
                    // stand-in) plus the celestial rotation, derived from
                    // the SAME hour that placed the sun — no host has to
                    // drive it, so every path that sets a time of day gets
                    // a sky that wheels correctly. A world with no hour
                    // (an authored sun direction) keeps a fixed dome.
                    let (star_tex, gain) = self.star_binding(cx.cx);
                    sa.cube.draw_vars.set_texture(0, &star_tex);
                    let rows = match world.sun.time_of_day {
                        Some(hours) => crate::sun::celestial_rows(hours, world.sun.latitude),
                        None => [
                            vec4(1.0, 0.0, 0.0, 0.0),
                            vec4(0.0, 1.0, 0.0, 0.0),
                            vec4(0.0, 0.0, 1.0, 0.0),
                        ],
                    };
                    sa.star_r0 = vec4(rows[0].x, rows[0].y, rows[0].z, gain);
                    sa.star_r1 = rows[1];
                    sa.star_r2 = rows[2];
                    sa.cube.draw(cx);
                }
                _ => {
                    draws.sky.cube.transform = transform;
                    draws.sky.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.sky.cube.cube_size = vec3(800.0, 800.0, 800.0);
                    draws.sky.cube.color = vec4(1.0, 1.0, 1.0, 1.0);
                    draws.sky.cube.depth_clip = 1.0;
                    draws.sky.sky_top = vec3(sky.top.x, sky.top.y, sky.top.z);
                    draws.sky.sky_horizon =
                        vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z);
                    draws.sky.sky_ground = vec3(sky.ground.x, sky.ground.y, sky.ground.z);
                    draws.sky.sky_bottom =
                        vec3(sky.ground_bottom.x, sky.ground_bottom.y, sky.ground_bottom.z);
                    draws.sky.cube.draw(cx);
                }
            }
            stats.sky_drawn = true;
        }

        // 2. The smooth terrain mesh, in CHUNK_SIZE tiles so the hills
        // behind the camera cost nothing. One draw item per visible tile —
        // the price of being able to skip the rest.
        // Terrain can be megabytes at 257². It is immutable for the whole
        // draw, so borrow it from the world; cloning here turned a steady
        // landscape into frame-rate-scaled memory bandwidth.
        if let Some(terrain) = world.terrain.as_ref().filter(|_| shows_environment) {
            self.ensure_terrain_tiles(cx.cx, terrain, world.terrain_materials.as_ref());
            draws.terrain.transform = Mat4f::identity();
            draws.terrain.depth_clip = 1.0;
            draws.terrain.fog_color = fog_color;
            draws.terrain.fog_density = fog_density;
            draws.terrain.lm_debug = self.lm_debug;
            // One planar lightmap region serves every tile: tile uvs derive
            // from world xz, so all tiles share one rect pair.
            let (lm_rect, lm_world) = self.lm_ground.unwrap_or_default();
            draws.terrain.lm_rect = lm_rect;
            draws.terrain.lm_world = lm_world;
            let lm_tex = self.lightmap_texture(cx.cx);
            draws.terrain.draw_vars.set_texture(0, &lm_tex);
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draws.terrain.draw_vars, &csm, &lm_tex, 1);
            for tile in &self.terrain_tiles {
                if let Some(frustum) = frustum {
                    if !frustum.intersects_aabb(tile.min, tile.max) {
                        stats.terrain_tiles_culled += 1;
                        continue;
                    }
                }
                draws.terrain.draw_vars.geometry_id = Some(tile.geometry.geometry_id());
                if draws.terrain.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draws.terrain.draw_vars);
                    draws.terrain.draw_vars.area =
                        cx.update_area_refs(draws.terrain.draw_vars.area, new_area);
                }
                stats.terrain_tiles_drawn += 1;
                stats.terrain_drawn = true;
            }
        }

        // 2b. Voxel terrain chunk meshes (mix.md T2/T3): the editable field's
        // surface, drawn through the SAME terrain shader and planar lightmap
        // region as the tiles — the render path the heightfield already
        // earned, reused rather than reinvented. One item per visible chunk.
        self.ensure_voxel_tiles(cx.cx, world.voxel.as_deref());
        if !self.voxel_tiles.is_empty() && shows_environment {
            draws.terrain.transform = Mat4f::identity();
            draws.terrain.depth_clip = 1.0;
            draws.terrain.fog_color = fog_color;
            draws.terrain.fog_density = fog_density;
            draws.terrain.lm_debug = self.lm_debug;
            let (lm_rect, lm_world) = self.lm_ground.unwrap_or_default();
            draws.terrain.lm_rect = lm_rect;
            draws.terrain.lm_world = lm_world;
            let lm_tex = self.lightmap_texture(cx.cx);
            draws.terrain.draw_vars.set_texture(0, &lm_tex);
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draws.terrain.draw_vars, &csm, &lm_tex, 1);
            for tile in &self.voxel_tiles {
                if let Some(frustum) = frustum {
                    if !frustum.intersects_aabb(tile.min, tile.max) {
                        stats.terrain_tiles_culled += 1;
                        continue;
                    }
                }
                draws.terrain.draw_vars.geometry_id = Some(tile.geometry.geometry_id());
                if draws.terrain.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draws.terrain.draw_vars);
                    draws.terrain.draw_vars.area =
                        cx.update_area_refs(draws.terrain.draw_vars.area, new_area);
                }
                stats.terrain_tiles_drawn += 1;
                stats.terrain_drawn = true;
            }
        }

        // PERF: sections 3+4 batch per shape through many_instances. Statics
        // come from packed slabs rebuilt only when world.render_rev moves
        // (bump it — mark_render_dirty — or your static edit won't show);
        // dynamics (movers, their parts, beams, blob shadows) re-pack every
        // frame. One draw call per shape per pass; empty batches are skipped.
        // fog_color is a uniform now (set in apply_sun); only the density
        // stays per-instance, because shadows switch it off individually.
        draws.cube.fog_density = fog_density;
        draws.alpha.cube.fog_density = fog_density;

        // CPU light bake (bake.rs). Geometry-dependent occlusion is baked
        // once per world edit; the sun term is one ray per target and
        // refreshes whenever the sun swings, so a day cycle moves the baked
        // shadows instead of freezing them at dawn. Both land in the colours
        // packed below — the GPU never learns this happened.
        self.bake.update(world, &sun);
        stats.bake = self.bake.stats();

        let vars_ready = draws.cube.cube.draw_vars.can_instance()
            && draws.alpha.cube.cube.draw_vars.can_instance();
        let slab_key = (world.render_rev, self.bake.generation());
        if vars_ready && self.slab_key != Some(slab_key) {
            let t0 = std::time::Instant::now();
            self.rebuild_static_slabs(draws, world);
            stats.slab_us += perf_us(t0);
            stats.slab_rebuilds += 1;
            self.slab_key = Some(slab_key);
        }
        stats.static_instances = self.slab_instance_count;
        stats.dyn_instances = 0;
        stats.instance_floats = draws.cube.cube.draw_vars.as_slice().len() as u32;

        // Slab chunk visibility, once per frame — both passes below memcpy
        // only the chunks whose content bounds touch the frustum. The chunks
        // themselves are camera-independent, so this never invalidates them.
        self.chunk_visible.clear();
        for chunk in &self.static_chunks {
            let visible = match frustum {
                Some(frustum) => frustum.intersects_aabb(chunk.min, chunk.max),
                None => true,
            };
            self.chunk_visible.push(visible);
            if visible {
                stats.chunks_drawn += 1;
            } else {
                stats.chunks_culled += 1;
            }
        }

        // Adaptive quality, resolved once per frame so every consumer below
        // sees the same level even if the governor moves mid-frame.
        let quality = self.thermometer.quality();
        // The dynamic shadow-mesh layer (entity hull drapes + pre-sidecar
        // blobs), OnChange only: in Realtime every caster is in the tiles.
        // Dropping projected_shadows demotes hulls to blobs — grounded but
        // cheaper — under thermal pressure.
        let shadow_mesh_enabled =
            draws.shadow.is_some() && quality.projected_shadows && tiers.sdf_quads;
        let shadow_budget =
            ((self.shadow_budget as f32 * quality.shadow_caster_scale).round() as usize).max(1);
        // Particles are pure decoration and the first thing to go, so this
        // keeps a prefix rather than sampling: an emitter's earliest
        // particles are its oldest, and thinning from the tail makes a plume
        // shorten instead of flickering into holes.
        let particle_count =
            (self.particle_instances.len() as f32 * quality.particle_scale).round() as usize;
        self.shadow_mesh.clear();
        // World-settle work, debounced: an edit burst pays ONE receiver-box
        // refresh + lightmap kick at rest. GENUINE world edits only — this
        // key never moves from routine mover/replication traffic, which is
        // what keeps OnChange at zero bake passes in steady state (the
        // two-mode invariant; gpu_lightmap.rs pins it).
        {
            // The DAYLIGHT quantum is in the key: a lamp's strength is a
            // function of the sky (the headroom rail), so a sun that moves
            // enough to change it has changed the atlas, not just the shade
            // term. Quantized, so a day cycle pays a bake per 3% of pool —
            // not one per frame.
            let day_key = Self::lamp_daylight_key(&sun);
            let key = (
                world.render_rev,
                self.bake.generation(),
                self.models_rev,
                day_key,
            );
            if self
                .shadow_gate
                .should_rebuild(key, std::time::Instant::now(), SHADOW_SETTLE)
            {
                self.refresh_shadow_receivers(world);
                // Same settle cadence: the light bake becomes GPU render
                // passes on the next frame (gpu_lightmap.rs). In Realtime
                // the baker re-bakes visible regions per frame on its own —
                // only a WORLD change (or a sun change that moves the lamps)
                // re-schedules the whole job; OnChange re-kicks on every
                // settle, sun changes included.
                let world_key = (world.render_rev, self.models_rev, day_key);
                if self.lm_kick_key != Some(world_key)
                    || self.gpu_baker.mode() == crate::gpu_lightmap::GpuLightmapMode::OnChange
                {
                    // Name the cause in the bake's own log line: a blowout
                    // that pops in has to be attributable to the run that
                    // caused it, not guessed at from a screenshot.
                    let trigger = match self.lm_kick_key {
                        None => crate::gpu_lightmap::BakeTrigger::FirstBake,
                        Some((rev, models, _)) if (rev, models) != (world_key.0, world_key.1) => {
                            crate::gpu_lightmap::BakeTrigger::WorldEdit
                        }
                        Some(_) => crate::gpu_lightmap::BakeTrigger::SunChange,
                    };
                    self.lm_kick_key = Some(world_key);
                    self.kick_lightmap_bake(world, &sun, trigger);
                }
                self.shadow_gate.mark_built(key);
            }
        }

        // PERF: resolve dynamic parts and shape membership ONCE per frame —
        // the per-shape loops below must not re-scan entities per part.
        let mut dyn_parts: Vec<(usize, usize)> = Vec::new();
        for (part_index, part) in world.parts.iter().enumerate() {
            let Some(owner_index) = entity_index_sorted(&world.entities, part.owner) else {
                continue;
            };
            if world.entities[owner_index].kind != BodyKind::Static || part.anim_active {
                dyn_parts.push((part_index, owner_index));
            }
        }
        let mut dyn_entity_shapes = [false; 5];
        let mut dyn_sensor_shapes = [false; 5];
        for e in world
            .entities
            .iter()
            .filter(|e| e.kind != BodyKind::Static && !e.hidden)
        {
            if e.sensor {
                dyn_sensor_shapes[e.shape.index()] = true;
            } else {
                dyn_entity_shapes[e.shape.index()] = true;
            }
        }
        let mut dyn_part_shapes = [false; 5];
        for (part_index, _) in &dyn_parts {
            dyn_part_shapes[world.parts[*part_index].shape.index()] = true;
        }

        // 3. Opaque pass, one batch per shape. Statics stream in from the
        // VISIBLE chunks only — still the same single draw item per shape,
        // the culling just shrinks what gets memcpy'd into it.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = self
                .static_chunks
                .iter()
                .zip(&self.chunk_visible)
                .any(|(c, v)| *v && !c.slab[shape_index].is_empty());
            let has_dynamic_entity = dyn_entity_shapes[shape_index];
            let has_dynamic_part = dyn_part_shapes[shape_index];
            let has_beams = shape == Shape::Box && !world.beams.is_empty();
            if !has_static && !has_dynamic_entity && !has_dynamic_part && !has_beams {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            draws.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            draws.cube.cube.many_instances =
                cx.begin_many_instances(&draws.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut draws.cube.cube.many_instances {
                    for (chunk, visible) in self.static_chunks.iter().zip(&self.chunk_visible) {
                        if *visible {
                            mi.instances.extend_from_slice(&chunk.slab[shape_index]);
                        }
                    }
                }
            }
            // Dynamic entities: movers/kinematics/projectiles of this shape.
            for e in world
                .entities
                .iter()
                .filter(|e| !e.sensor && !e.hidden && e.kind != BodyKind::Static && e.shape == shape)
            {
                // Every shape geometry spans [-0.5, 0.5] scaled by cube_size,
                // so |half*scale| bounds the visual under ANY rotation. The
                // entity's cast shadow is not tied to this instance — blobs
                // and silhouettes draw from their own loops below.
                if let Some(frustum) = frustum {
                    let r = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    )
                    .length();
                    if !frustum.intersects_sphere(e.pos, r) {
                        stats.dyn_culled += 1;
                        continue;
                    }
                }
                let mut transform = Self::entity_rotation(e);
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                draws.cube.cube.transform = transform;
                draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                // Movers sample the baked probe lattice, so a crate rolling
                // under a bridge darkens without a shadow map or a pass.
                draws.cube.cube.color =
                    shade_color(e.color, self.bake.dynamic_shade(e.pos), e.glow);
                draws.cube.cube.depth_clip = 1.0;
                draws.cube.glow = e.glow;
                draws.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Parts that are NOT in the slab: dynamic owner, or mid-animation.
            for (part_index, owner_index) in dyn_parts.iter().copied() {
                let part = &world.parts[part_index];
                if part.shape != shape {
                    continue;
                }
                let owner = &world.entities[owner_index];
                let mut owner_frame = Self::entity_rotation(owner);
                owner_frame.v[12] = owner.pos.x;
                owner_frame.v[13] = owner.pos.y;
                owner_frame.v[14] = owner.pos.z;
                let mut local = Mat4f::rotation(part.rot);
                local.v[12] = part.offset.x * owner.scale.x;
                local.v[13] = part.offset.y * owner.scale.y;
                local.v[14] = part.offset.z * owner.scale.z;
                let transform = Mat4f::mul(&owner_frame, &local);
                // Both frames are pure rotations, so the composed transform's
                // translation is the part's world centre and |half*scale|
                // bounds it, exactly as for the entity above.
                if let Some(frustum) = frustum {
                    let center = vec3f(transform.v[12], transform.v[13], transform.v[14]);
                    let r = vec3f(
                        part.half.x * owner.scale.x,
                        part.half.y * owner.scale.y,
                        part.half.z * owner.scale.z,
                    )
                    .length();
                    if !frustum.intersects_sphere(center, r) {
                        stats.dyn_culled += 1;
                        continue;
                    }
                }
                draws.cube.cube.transform = transform;
                draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.cube.cube.cube_size = vec3(
                    part.half.x * 2.0 * owner.scale.x,
                    part.half.y * 2.0 * owner.scale.y,
                    part.half.z * 2.0 * owner.scale.z,
                );
                draws.cube.cube.color =
                    shade_color(part.color, self.bake.dynamic_shade(owner.pos), part.glow);
                draws.cube.cube.depth_clip = 1.0;
                draws.cube.glow = part.glow;
                draws.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Immediate-mode beams (box batch): a box stretched between two
            // points (grapple cables, lasers). Cable axis on local z.
            if has_beams {
                for beam in &world.beams {
                    let d = beam.to - beam.from;
                    let len = d.length();
                    if len < 1.0e-4 {
                        continue;
                    }
                    // Sphere around the beam's midpoint; the box is size x
                    // size x len, so half the length plus the full cross
                    // section covers it under any orientation.
                    if let Some(frustum) = frustum {
                        let mid = beam.from + d * 0.5;
                        if !frustum.intersects_sphere(mid, len * 0.5 + beam.size) {
                            stats.dyn_culled += 1;
                            continue;
                        }
                    }
                    let f = d * (1.0 / len);
                    let upv = if f.y.abs() > 0.99 {
                        vec3f(1.0, 0.0, 0.0)
                    } else {
                        vec3f(0.0, 1.0, 0.0)
                    };
                    let r = Vec3f::cross(upv, f).normalize();
                    let u = Vec3f::cross(f, r);
                    let mid = beam.from + d * 0.5;
                    let mut m = Mat4f::identity();
                    m.v[0] = r.x;
                    m.v[1] = r.y;
                    m.v[2] = r.z;
                    m.v[4] = u.x;
                    m.v[5] = u.y;
                    m.v[6] = u.z;
                    m.v[8] = f.x;
                    m.v[9] = f.y;
                    m.v[10] = f.z;
                    m.v[12] = mid.x;
                    m.v[13] = mid.y;
                    m.v[14] = mid.z;
                    draws.cube.cube.transform = m;
                    draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.cube.cube.cube_size = vec3(beam.size, beam.size, len);
                    draws.cube.cube.color = beam.color;
                    draws.cube.cube.depth_clip = 1.0;
                    draws.cube.glow = beam.glow;
                    draws.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                }
            }
            if let Some(mi) = draws.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        // 3.5 Skinned characters — after all opaque, before alpha blending.
        if let Some(batch) = skinned {
            // Sidecar resolution precedes the shadow loop below (it
            // consults the uploaded atlases). OnChange only: in Realtime
            // the SDF tier draws nothing, so there is nothing to resolve.
            if tiers.sdf_quads {
                self.seed_skinned_sdf(cx.cx, &batch.items, &sun);
            }
            // Character shadows: one SDF-silhouette quad each. Per
            // character per frame the CPU computes ONE anchor (ground
            // sample + lamp/height policy, character_shadow_anchor) and
            // pushes one five-vec4 instance record — the pixel stage
            // morphs the baked silhouette between the yaw/phase neighbour
            // cells by lerping DISTANCES (shadow_sdf.rs), so this loop's
            // cost is flat and tiny no matter how the crowd grows. The
            // anchor stays the single boss of placement: its landing point
            // positions the quad, its lean supplies the owning light's
            // azimuth (sun blended toward a dominant lamp), its size_mul
            // compresses the sprite under a near-overhead bulb, its alpha
            // darkens it. The blob survives only as the fallback for a rig
            // with no loadable sidecar (or a host with no SDF shader).
            // Realtime draws NONE of this — characters are in the tiles.
            if shadow_mesh_enabled {
                let t0 = std::time::Instant::now();
                let ground = world
                    .terrain
                    .as_ref()
                    .and_then(|t| {
                        let p = batch.items.first()?;
                        t.height_at(p.transform.v[12], p.transform.v[14])
                    })
                    .unwrap_or(0.0);
                for item in batch.items.iter() {
                    let t = &item.transform;
                    // Footprint from the transform's own scale: characters
                    // are authored around a ~0.45-unit half footprint.
                    let sx = (t.v[0] * t.v[0] + t.v[1] * t.v[1] + t.v[2] * t.v[2]).sqrt();
                    let sz = (t.v[8] * t.v[8] + t.v[9] * t.v[9] + t.v[10] * t.v[10]).sqrt();
                    let receiver = Receiver {
                        base_y: ground,
                        terrain: world.terrain.as_ref(),
                        statics: &self.receiver_boxes,
                    };
                    let feet = vec3f(t.v[12], t.v[13], t.v[14]);
                    let Some(a) = self.character_shadow_anchor(feet, &receiver, &sun)
                    else {
                        continue;
                    };
                    if draws.shadow_sdf.is_some() {
                        if let Some((_, Some((_, meta)))) = self
                            .sdf_atlas_tex
                            .iter()
                            .find(|(r, _)| *r == item.rig)
                        {
                            let yaw = t.v[8].atan2(t.v[10]);
                            // Horizontal direction TOWARD the owning light:
                            // opposite the anchor's lean (the silhouette
                            // points down the lean); a lean too small to
                            // read (dead under a bulb, or a noon sun) falls
                            // back to the sun's azimuth.
                            let l = (a.lean.x * a.lean.x + a.lean.y * a.lean.y).sqrt();
                            let (gx, gz) = if l > 1.0e-3 {
                                (-a.lean.x / l, -a.lean.y / l)
                            } else {
                                let g = sun.dir_ground();
                                (g.x, g.y)
                            };
                            // The atlas is baked in a canonical light frame
                            // (azimuth +x); the cell index is the yaw
                            // RELATIVE to the light: world = M(alpha) *
                            // canonical(yaw - alpha), alpha from the axis.
                            let rel = yaw - (-gz).atan2(gx);
                            let band2 = 2.0 * meta.band_world.max(1.0e-4);
                            let scale = sx.max(sz) * a.size_mul;
                            // Sun-tolerant stretch: the window IS the sample
                            // map, so scaling its ALONG components by the
                            // current-vs-baked shadow-length ratio stretches
                            // the baked silhouette to today's sun.
                            let sun_len = sun.shadow_len_per_unit();
                            let stretch =
                                (sun_len / meta.len_per_unit.max(0.05)).clamp(0.2, 5.0);
                            // Ride the highest surface under the
                            // silhouette's run (see sdf_quad_ground) —
                            // rect.x is the window's down-sun edge.
                            let y_quad = sdf_quad_ground(
                                a.root,
                                &receiver,
                                gx,
                                gz,
                                (-meta.rect.x * stretch).max(0.0) * scale,
                            );
                            // The quad's window origin — the bake's ground
                            // anchor, the silhouette's FOOT end — sits at
                            // the pinned root: the shadow stays attached to
                            // the boots however hard a lamp leans its body.
                            self.sdf_instances.push(SdfInstance {
                                atlas: SdfAtlasKey::Rig(item.rig),
                                a: vec4(a.root.x, y_quad, a.root.z, a.lift),
                                b: vec4(gx, gz, scale, a.alpha),
                                c: vec4(
                                    rel,
                                    item.gait_phase,
                                    item.gait_blend,
                                    meta.rows as f32,
                                ),
                                d: vec4(
                                    meta.rect.x * stretch,
                                    meta.rect.y,
                                    meta.rect.z * stretch,
                                    meta.rect.w,
                                ),
                                e: vec4(
                                    SDF_SOFT_BASE / band2,
                                    SDF_SOFT_HARDEN / (sun_len.max(0.2) * band2),
                                    0.0,
                                    0.0,
                                ),
                            });
                            stats.shadows += 1;
                            stats.sdf_shadow_instances += 1;
                            continue;
                        }
                        // Atlas still baking (or the rig baked to
                        // nothing): fall through to the blob.
                    }
                    // Blob fallback. Same anchor policy; the lamp/height
                    // darkening rides a local sun copy the mesh builder
                    // reads its alpha from.
                    let mut shadow_sun = sun;
                    shadow_sun.shadow_alpha =
                        (sun.shadow_alpha * (1.0 + 0.7 * a.lamp_w)).min(0.6);
                    // A blob is a contact shadow — it sits at the pinned
                    // root (never displaced by a lamp lean).
                    let centre = vec3f(a.root.x, feet.y, a.root.z);
                    if crate::shadow_mesh::build_blob_shadow(
                        centre,
                        0.45 * sx * a.size_mul,
                        0.45 * sz * a.size_mul,
                        &shadow_sun,
                        &receiver,
                        &mut self.shadow_mesh,
                    ) {
                        stats.shadows += 1;
                    }
                }
                stats.dyn_shadow_us += perf_us(t0);
            }
            // Ground height under every character (one receiver sample each):
            // the shader projects its baked-shadow lookup along the sun ray
            // down to this plane, so the boundary slants across the body and
            // a jumping character exits shadow as they rise.
            self.char_ground.clear();
            for item in &batch.items {
                let (x, z) = (item.transform.v[12], item.transform.v[14]);
                let base = world
                    .terrain
                    .as_ref()
                    .and_then(|t| t.height_at(x, z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: base,
                    terrain: world.terrain.as_ref(),
                    statics: &self.receiver_boxes,
                };
                self.char_ground.push(receiver.sample(x, z).0);
            }
            self.draw_skinned_inner(
                cx,
                batch,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
        }

        // Dynamic prop instances — the driveable cars. Their shadow is the
        // same SDF-silhouette quad the characters draw, from a yaw-only
        // atlas loaded from the model's offline `.shadowsdf` sidecar
        // (shadow_sdf.rs, tools/ao_bake): per frame each car costs one
        // anchor + one five-vec4 instance record, gait inputs inert. The
        // same anchor policy owns placement — lean and compression under a
        // dominant lamp, height offset on ramps. The atlas bakes the car
        // FLAT, so a heavily tilted or airborne car falls back to the
        // plain blob, as does any instance whose model has no sidecar.
        // Realtime draws none of this — the cars are in the tiles.
        if shadow_mesh_enabled {
            let t0 = std::time::Instant::now();
            let instances = std::mem::take(&mut self.placed_models);
            for inst in &instances {
                if !inst.dynamic {
                    continue;
                }
                let Some((mmin, mmax)) = self
                    .static_models
                    .iter()
                    .find(|(k, _)| *k == inst.model)
                    .map(|(_, m)| (m.min, m.max))
                else {
                    continue;
                };
                // First sight of this model loads its sidecar (one stat +
                // read); a miss caches a None so the frame never re-tries.
                self.seed_model_sdf(cx.cx, &inst.model, &sun);
                let t = &inst.transform;
                let mid = vec3f(
                    (mmin.x + mmax.x) * 0.5,
                    mmin.y,
                    (mmin.z + mmax.z) * 0.5,
                );
                let feet = vec3f(
                    t.v[0] * mid.x + t.v[4] * mid.y + t.v[8] * mid.z + t.v[12],
                    t.v[1] * mid.x + t.v[5] * mid.y + t.v[9] * mid.z + t.v[13],
                    t.v[2] * mid.x + t.v[6] * mid.y + t.v[10] * mid.z + t.v[14],
                );
                let sx = (t.v[0] * t.v[0] + t.v[1] * t.v[1] + t.v[2] * t.v[2]).sqrt();
                let sz = (t.v[8] * t.v[8] + t.v[9] * t.v[9] + t.v[10] * t.v[10]).sqrt();
                let ground = world
                    .terrain
                    .as_ref()
                    .and_then(|tr| tr.height_at(feet.x, feet.z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: ground,
                    terrain: world.terrain.as_ref(),
                    statics: &self.receiver_boxes,
                };
                // Tilt/air gates: body-up vs world-up from the transform's
                // y basis, clear air from the receiver under the footprint.
                let up_len =
                    (t.v[4] * t.v[4] + t.v[5] * t.v[5] + t.v[6] * t.v[6]).sqrt().max(1.0e-6);
                let up_y = t.v[5] / up_len;
                let air = feet.y - receiver.sample(feet.x, feet.z).0;
                let mut drawn = false;
                if draws.shadow_sdf.is_some() && car_sprite_allowed(up_y, air) {
                    if let Some(Some((_, meta))) = self.model_sdf_tex.get(&inst.model) {
                        if let Some(a) = character_shadow_anchor(
                            feet,
                            &receiver,
                            &sun,
                            &self.frame_lights,
                        ) {
                            let yaw = t.v[8].atan2(t.v[10]);
                            // Same owning-light frame math as the
                            // characters: axis toward the light, opposite
                            // the anchor's lean, sun azimuth when the lean
                            // is too small to read.
                            let l = (a.lean.x * a.lean.x + a.lean.y * a.lean.y).sqrt();
                            let (gx, gz) = if l > 1.0e-3 {
                                (-a.lean.x / l, -a.lean.y / l)
                            } else {
                                let g = sun.dir_ground();
                                (g.x, g.y)
                            };
                            let rel = yaw - (-gz).atan2(gx);
                            let band2 = 2.0 * meta.band_world.max(1.0e-4);
                            let scale = sx.max(sz) * a.size_mul;
                            // Sun-tolerant stretch, exactly as for rigs.
                            let sun_len = sun.shadow_len_per_unit();
                            let stretch =
                                (sun_len / meta.len_per_unit.max(0.05)).clamp(0.2, 5.0);
                            // Same raised-receiver guard as the characters:
                            // a car parked on grass beside a proud road
                            // slab must not bury its silhouette under it.
                            let y_quad = sdf_quad_ground(
                                a.root,
                                &receiver,
                                gx,
                                gz,
                                (-meta.rect.x * stretch).max(0.0) * scale,
                            );
                            // Window origin pinned at the root, exactly as
                            // for characters: the wheels' contact line
                            // never leaves the car.
                            self.sdf_instances.push(SdfInstance {
                                atlas: SdfAtlasKey::Model(inst.model.clone()),
                                a: vec4(a.root.x, y_quad, a.root.z, a.lift),
                                b: vec4(gx, gz, scale, a.alpha),
                                c: vec4(rel, 0.0, 0.0, meta.rows as f32),
                                d: vec4(
                                    meta.rect.x * stretch,
                                    meta.rect.y,
                                    meta.rect.z * stretch,
                                    meta.rect.w,
                                ),
                                e: vec4(
                                    SDF_SOFT_BASE / band2,
                                    SDF_SOFT_HARDEN / (sun_len.max(0.2) * band2),
                                    0.0,
                                    0.0,
                                ),
                            });
                            stats.shadows += 1;
                            stats.sdf_shadow_instances += 1;
                            drawn = true;
                        }
                    }
                }
                if !drawn {
                    crate::shadow_mesh::build_blob_shadow(
                        feet,
                        (mmax.x - mmin.x) * 0.55 * sx,
                        (mmax.z - mmin.z) * 0.55 * sz,
                        &sun,
                        &receiver,
                        &mut self.shadow_mesh,
                    );
                }
            }
            self.placed_models = instances;
            stats.dyn_shadow_us += perf_us(t0);
        }

        // Stock props: the same shader as the skinned path (both are textured
        // packed meshes), but with geometry uploaded once instead of per frame.
        if let Some(draw) = models_draw.as_deref_mut() {
            // Ground plane per DYNAMIC instance (cars), for the sun-ray
            // projected baked-shadow sample; statics never read it.
            self.model_ground.clear();
            for inst in &self.placed_models {
                if !inst.dynamic {
                    self.model_ground.push(0.0);
                    continue;
                }
                let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                let base = world
                    .terrain
                    .as_ref()
                    .and_then(|t| t.height_at(x, z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: base,
                    terrain: world.terrain.as_ref(),
                    statics: &self.receiver_boxes,
                };
                self.model_ground.push(receiver.sample(x, z).0);
            }
            let instances = std::mem::take(&mut self.placed_models);
            self.draw_models_inner(
                cx,
                ModelDraw::Diffuse(draw),
                camera_pos,
                &instances,
                WorldModelLane::Placed,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.draw_pbr_models(
                cx,
                camera_pos,
                &instances,
                WorldModelLane::Placed,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.placed_models = instances;

            // Actor-attached props share the world material/depth pass, but
            // this is their ONLY renderer traversal. In particular they
            // were absent from the lightmap/CSM mover and shadow-building
            // stages above. A receiver sample gives their shader the same
            // projected baked-shadow lookup as other moving geometry
            // without registering any receiver geometry of their own.
            self.world_attachment_ground.clear();
            for inst in &self.world_attachments {
                let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                let base = world
                    .terrain
                    .as_ref()
                    .and_then(|t| t.height_at(x, z))
                    .unwrap_or(0.0);
                let receiver = Receiver {
                    base_y: base,
                    terrain: world.terrain.as_ref(),
                    statics: &self.receiver_boxes,
                };
                self.world_attachment_ground
                    .push(receiver.sample(x, z).0);
            }
            let attachments = std::mem::take(&mut self.world_attachments);
            self.draw_models_inner(
                cx,
                ModelDraw::Diffuse(draw),
                camera_pos,
                &attachments,
                WorldModelLane::Attachment,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.draw_pbr_models(
                cx,
                camera_pos,
                &attachments,
                WorldModelLane::Attachment,
                (fog_color, fog_density),
                &sun,
                frustum,
                &mut stats,
            );
            self.world_attachments = attachments;
        }

        // 3s. The map's own sky surfaces, after the opaque world so they are
        // depth-rejected behind it rather than shading over it. Drawn even
        // when a host lends no models_draw: the sky lane owns its shader.
        self.draw_sky_faces(cx, camera_pos, frustum, &mut stats);

        // 3w. Water sheets (mix.md W1): one displaced grid per `game.water`
        // volume, drawn after every opaque pass (blending sees depth: a hull
        // below the surface tints, a hull above does not) and before the
        // alpha batches, so sensor ghosts and particles composite over the
        // water. The VERTEX shader displaces by the same wave sum the sim
        // steps — same coefficients, same expression (pin test below) —
        // visual only: physics never reads the GPU.
        self.ensure_water_tiles(cx.cx, world.water.as_deref());
        if !self.water_tiles.is_empty() && shows_environment {
            if let Some(water_draw) = draws.water.as_deref_mut() {
                water_draw.transform = Mat4f::identity();
                water_draw.depth_clip = 1.0;
                water_draw.fog_color = fog_color;
                water_draw.fog_density = fog_density;
                sun.write_into(
                    &mut water_draw.light_dir,
                    &mut water_draw.sun_color,
                    &mut water_draw.sun_sky,
                    &mut water_draw.sun_ground,
                );
                // The sim's own f32 tick-time — the ONE time base both sides
                // of the wave expression consume.
                let t = makepad_game_sim::water::tick_time(world.tick);
                const WAVE_A: [LiveId; 8] = [
                    live_id!(wave_a0), live_id!(wave_a1), live_id!(wave_a2), live_id!(wave_a3),
                    live_id!(wave_a4), live_id!(wave_a5), live_id!(wave_a6), live_id!(wave_a7),
                ];
                const WAVE_B: [LiveId; 8] = [
                    live_id!(wave_b0), live_id!(wave_b1), live_id!(wave_b2), live_id!(wave_b3),
                    live_id!(wave_b4), live_id!(wave_b5), live_id!(wave_b6), live_id!(wave_b7),
                ];
                for tile in &self.water_tiles {
                    if let Some(frustum) = frustum {
                        if !frustum.intersects_aabb(tile.min, tile.max) {
                            continue;
                        }
                    }
                    // Per-volume uniforms: a differing coefficient set starts
                    // its own draw item (the appendable check compares
                    // dyn_uniforms), so volumes never share stale waves.
                    for i in 0..MAX_WAVES {
                        water_draw
                            .draw_vars
                            .set_uniform(cx.cx, WAVE_A[i], &tile.waves_a[i]);
                        water_draw
                            .draw_vars
                            .set_uniform(cx.cx, WAVE_B[i], &tile.waves_b[i]);
                    }
                    water_draw
                        .draw_vars
                        .set_uniform(cx.cx, live_id!(water_params), &[0.0, t, 0.0, 0.0]);
                    water_draw.draw_vars.geometry_id = Some(tile.geometry.geometry_id());
                    if water_draw.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&water_draw.draw_vars);
                        water_draw.draw_vars.area =
                            cx.update_area_refs(water_draw.draw_vars.area, new_area);
                    }
                }
            }
        }

        // 4. Alpha pass, one batch per shape: static sensors from the slab,
        // then blob shadows (box batch) and dynamic sensors — drawn after all
        // opaque geometry so blending sees depth.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = self
                .static_chunks
                .iter()
                .zip(&self.chunk_visible)
                .any(|(c, v)| *v && !c.slab_alpha[shape_index].is_empty());
            let has_dynamic_sensor = dyn_sensor_shapes[shape_index];
            // Entity casters (crates, movers): runtime-spawned primitive
            // bodies with no offline atlas, so in OnChange their ONLY tier
            // is the hull drape / blob quad below. In Realtime they are in
            // the tiles (collect_lm_movers) and this tier is off.
            let has_shadows = shape == Shape::Box
                && tiers.sdf_quads
                && world.entities.iter().any(|e| {
                    matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                        && !e.sensor
                        && !e.hidden
                        && e.attached_to == 0
                });
            let has_particles = shape == Shape::Box && particle_count > 0;
            if !has_static && !has_dynamic_sensor && !has_shadows && !has_particles {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            draws.alpha.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            draws.alpha.cube.cube.many_instances =
                cx.begin_many_instances(&draws.alpha.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut draws.alpha.cube.cube.many_instances {
                    for (chunk, visible) in self.static_chunks.iter().zip(&self.chunk_visible) {
                        if *visible {
                            mi.instances.extend_from_slice(&chunk.slab_alpha[shape_index]);
                        }
                    }
                }
            }
            if has_shadows {
                // Tiered cast shadows: the nearest casters get a real
                // silhouette MESH (shadow_mesh.rs, accumulated below and
                // drawn as one geometry); everything else falls back to a
                // blob quad in this batch. So the budget buys fidelity, and
                // the whole shadow layer is at most two draw calls.
                for (e, ground, projected) in
                    Self::shadow_casters(world, camera_pos, shadow_budget)
                {
                    let half = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    );
                    if projected && shadow_mesh_enabled {
                        // Silhouette tier: hull of the caster's own points,
                        // draped over whatever it lands on.
                        let mut transform = Self::entity_rotation(e);
                        transform.v[12] = e.pos.x;
                        transform.v[13] = e.pos.y;
                        transform.v[14] = e.pos.z;
                        caster_points(
                            e.shape,
                            &transform,
                            vec3f(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                            &mut self.shadow_points,
                        );
                        let receiver = Receiver {
                            base_y: ground,
                            terrain: world.terrain.as_ref(),
                            statics: &self.receiver_boxes,
                        };
                        if crate::shadow_mesh::build_caster_shadow(
                            &self.shadow_points,
                            &sun,
                            &receiver,
                            &mut self.shadow_mesh,
                        ) {
                            stats.shadows += 1;
                            stats.projected_shadows += 1;
                        }
                        continue;
                    }
                    let quad = crate::shadow::blob_shadow(e.pos, half, ground, &sun);
                    let Some(quad) = quad else { continue };
                    // No fog on a shadow. It lies ON ground that is already
                    // fogged, so fogging it again mixes its RGB toward the
                    // bright horizon colour and a distant shadow comes out
                    // LIGHTER than the surface it darkens.
                    draws.alpha.cube.fog_density = 0.0;
                    draws.alpha.cube.cube.transform = quad.transform();
                    draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.alpha.cube.cube.cube_size = quad.size();
                    // The pipeline blends PREMULTIPLIED (src*1 + dst*(1-a)),
                    // so a shadow must be premultiplied black: RGB 0 leaves
                    // exactly ground*(1-a) — a true multiplicative shadow.
                    // Unpremultiplied dark RGB adds light instead of removing
                    // it, which is why the old blob shadows read as pale.
                    draws.alpha.cube.cube.color = vec4(0.0, 0.0, 0.0, quad.alpha);
                    draws.alpha.cube.cube.depth_clip = 1.0;
                    draws.alpha.cube.glow = 0.0;
                    draws.alpha.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                    stats.shadows += 1;
                }
                draws.alpha.cube.fog_density = fog_density;
            }
            if has_particles {
                // Small unlit-ish cubes: cheap, and they read fine at the
                // sizes particles live at. Emission carries the colour so a
                // spark stays bright regardless of where the sun is.
                for p in self.particle_instances.iter().take(particle_count) {
                    let mut transform = Mat4f::identity();
                    transform.v[12] = p.pos.x;
                    transform.v[13] = p.pos.y;
                    transform.v[14] = p.pos.z;
                    draws.alpha.cube.cube.transform = transform;
                    draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.alpha.cube.cube.cube_size = vec3(p.size, p.size, p.size);
                    // Premultiplied, same blend rule as the shadows above —
                    // otherwise a fading particle stays at full brightness
                    // and smoke reads as a blown-out white plume.
                    draws.alpha.cube.cube.color = vec4(
                        p.color.x * p.color.w,
                        p.color.y * p.color.w,
                        p.color.z * p.color.w,
                        p.color.w,
                    );
                    draws.alpha.cube.cube.depth_clip = 1.0;
                    draws.alpha.cube.glow = 0.8;
                    draws.alpha.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                    stats.particles += 1;
                }
                draws.alpha.cube.glow = 0.0;
            }
            for e in world
                .entities
                .iter()
                .filter(|e| e.sensor && !e.hidden && e.kind != BodyKind::Static && e.shape == shape)
            {
                if let Some(frustum) = frustum {
                    let r = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    )
                    .length();
                    if !frustum.intersects_sphere(e.pos, r) {
                        stats.dyn_culled += 1;
                        continue;
                    }
                }
                let mut transform = Self::entity_rotation(e);
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                draws.alpha.cube.cube.transform = transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.alpha.cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                let mut color = e.color;
                if color.w >= 0.99 {
                    // Sensors are see-through by default; explicit alpha wins.
                    color.w = 0.35;
                }
                draws.alpha.cube.cube.color = color;
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = e.glow;
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            if let Some(mi) = draws.alpha.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        // 4.5 The dynamic shadow-mesh layer (entity hull drapes + the blob
        // fallbacks accumulated above), rebuilt and uploaded per frame —
        // small by construction: statics live in the baked lightmap and
        // characters + cars ride the SDF quads below. Drawn after the alpha
        // batches so it lies over the ground it darkens; depth test on /
        // depth write off means overlapping shadows can never fight for the
        // buffer.
        if let Some(shadow) = draws.shadow.as_deref_mut() {
            let t0 = std::time::Instant::now();
            self.last_dynamic_shadow_tris = self.shadow_mesh.triangle_count();
            if !self.shadow_mesh.is_empty() {
                let geometry = self.shadow_geometry.get_or_insert_with(|| Geometry::new(cx.cx));
                geometry.update(
                    cx.cx,
                    std::mem::take(&mut self.shadow_mesh.indices),
                    std::mem::take(&mut self.shadow_mesh.vertices),
                );
                shadow.draw_vars.geometry_id = Some(geometry.geometry_id());
                if shadow.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&shadow.draw_vars);
                    shadow.draw_vars.area = cx.update_area_refs(shadow.draw_vars.area, new_area);
                }
            }
            stats.dyn_shadow_us += perf_us(t0);
        }

        // 4.6 SDF silhouette shadows — the dynamic casters (characters +
        // driven cars): one shared quad, one five-vec4 record per caster,
        // the silhouette morphed per PIXEL from the caster's SDF atlas.
        // Sorted by atlas so each rig's crowd (and each car model's fleet)
        // shares a draw item — the atlas bind is what splits items.
        if let Some(sd) = draws.shadow_sdf.as_deref_mut() {
            if !self.sdf_instances.is_empty() {
                let t0 = std::time::Instant::now();
                let geometry_id = self.ensure_flare_geometry(cx.cx);
                sd.draw_vars.geometry_id = Some(geometry_id);
                sd.depth_clip = 1.0;
                self.sdf_instances.sort_by(|a, b| a.atlas.cmp(&b.atlas));
                let mut bound: Option<&SdfAtlasKey> = None;
                for inst in &self.sdf_instances {
                    if bound != Some(&inst.atlas) {
                        let tex = match &inst.atlas {
                            SdfAtlasKey::Rig(rig) => self
                                .sdf_atlas_tex
                                .iter()
                                .find(|(r, _)| r == rig)
                                .and_then(|(_, p)| p.as_ref()),
                            SdfAtlasKey::Model(key) => {
                                self.model_sdf_tex.get(key).and_then(|p| p.as_ref())
                            }
                        };
                        let Some((tex, _)) = tex else { continue };
                        sd.draw_vars.set_texture(0, tex);
                        bound = Some(&inst.atlas);
                    }
                    sd.sdf_a = inst.a;
                    sd.sdf_b = inst.b;
                    sd.sdf_c = inst.c;
                    sd.sdf_d = inst.d;
                    sd.sdf_e = inst.e;
                    if sd.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&sd.draw_vars);
                        sd.draw_vars.area =
                            cx.update_area_refs(sd.draw_vars.area, new_area);
                    }
                }
                stats.dyn_shadow_us += perf_us(t0);
            }
        }
        self.sdf_instances.clear();

        // 5. MR shadow catcher: a dark translucent slab just under the
        // diorama's footprint. Without it the world reads as floating
        // stickers over the passthrough feed; with it, planted on the floor.
        // Drawn last so it blends over everything it sits beneath.
        if !shows_environment {
            if let Some(footprint) = Self::world_footprint(world) {
                let (center, radius) = footprint;
                let mut transform = Mat4f::identity();
                transform.v[12] = center.x;
                transform.v[13] = center.y - 0.02 / self.stage.scale.max(1.0e-4);
                transform.v[14] = center.z;
                draws.alpha.cube.cube.transform = transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                // Flat slab: thin in y, footprint-sized in x/z.
                let thickness = 0.04 / self.stage.scale.max(1.0e-4);
                draws.alpha.cube.cube.cube_size =
                    vec3(radius * 2.0, thickness, radius * 2.0);
                draws.alpha.cube.cube.color = vec4(0.0, 0.0, 0.0, 0.25);
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = 0.0;
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
                stats.shadow_catcher_drawn = true;
            }
        }

        // 6. Fireworks, last of all: additive and depth-write-off, so they
        // must come after every opaque surface has laid down the depth they
        // test against. ONE instance per shell — the GPU expands each into
        // SPARKS_PER_SHELL sparks from a closed form (firework.rs).
        if let Some(fw) = draws.firework.as_deref_mut() {
            if !self.firework_instances.is_empty() && shows_environment {
                let geometry_id = self.ensure_spark_geometry(cx.cx);
                fw.draw_vars.geometry_id = Some(geometry_id);
                fw.depth_clip = 1.0;
                for f in &self.firework_instances {
                    fw.origin_age = vec4(f.origin.x, f.origin.y, f.origin.z, f.age);
                    fw.launch_life = vec4(f.launch.x, f.launch.y, f.launch.z, f.life);
                    // Roomy quad; the sprite core occupies only its middle fifth
                    // (see spark_pixel), so this is streak headroom, not dot size.
                    fw.params = vec4(f.speed, f.seed, 1.1, 0.0);
                    fw.color = f.color;
                    fw.color_tail = f.color_tail;
                    if fw.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&fw.draw_vars);
                        fw.draw_vars.area = cx.update_area_refs(fw.draw_vars.area, new_area);
                    }
                }
                stats.firework_shells = self.firework_instances.len() as u64;
            }
        }

        // 6.5 Old-school lamp flares: one additive camera-facing billboard
        // pin-glow per visible street lamp, positions straight off the
        // harvested lamp list. Depth-tested (a wall between you and the lamp
        // eats the glow — that IS the old-school behaviour) but never
        // depth-written, drawn with the other late transparents.
        if let Some(fl) = draws.flare.as_deref_mut() {
            if shows_environment && !self.lamp_cache.is_empty() {
                let geometry_id = self.ensure_flare_geometry(cx.cx);
                fl.draw_vars.geometry_id = Some(geometry_id);
                fl.depth_clip = 1.0;
                for l in &self.lamp_cache {
                    if let Some(frustum) = frustum {
                        if !frustum.intersects_sphere(l.pos, 1.5) {
                            continue;
                        }
                    }
                    // Nudge toward the camera so the fixture's own head — the
                    // bulb sits INSIDE it — cannot eclipse its flare.
                    let to_cam = camera_pos - l.pos;
                    let d = to_cam.length().max(1.0e-4);
                    let pos = l.pos + to_cam * (0.35 / d).min(0.5);
                    // ~0.8-1.4 units across, scaled by the lamp's intensity
                    // (lamp colours cap at 2.0 for the atlas encode).
                    let intensity = light_intensity(l) * 0.5;
                    let size = (1.4 * intensity).clamp(0.8, 1.4);
                    fl.flare_pos = vec4(pos.x, pos.y, pos.z, size);
                    fl.flare_col = vec4(
                        l.color.x * 0.5,
                        l.color.y * 0.5,
                        l.color.z * 0.5,
                        1.0,
                    );
                    if fl.draw_vars.can_instance() {
                        let new_area = cx.add_instance(&fl.draw_vars);
                        fl.draw_vars.area = cx.update_area_refs(fl.draw_vars.area, new_area);
                    }
                    stats.flares += 1;
                }
            }
        }

        // 6.6 In-world video screen: one textured quad on the shared flare
        // geometry. The host owns placement and the per-frame texture; this
        // just issues the instance. Opaque + depth-written, so ordering
        // against the transparents above doesn't matter.
        if let Some(sc) = draws.screen.as_deref_mut() {
            let geometry_id = self.ensure_flare_geometry(cx.cx);
            sc.draw_vars.geometry_id = Some(geometry_id);
            sc.depth_clip = 1.0;
            sc.cutout = 0.0;
            sc.uv_rect = vec4(0.0, 0.0, 1.0, 1.0);
            if sc.screen_size.x.abs() > 0.0
                && sc.screen_size.y > 0.0
                && sc.draw_vars.can_instance()
            {
                let new_area = cx.add_instance(&sc.draw_vars);
                sc.draw_vars.area = cx.update_area_refs(sc.draw_vars.area, new_area);
            }
            // Sprite billboards are cut-out artwork, always: the quad is a
            // bounding box around a figure and everything outside it is
            // transparent. Without the alpha test every actor is a black
            // rectangle. The video screen above keeps cutout 0.
            sc.cutout = 1.0;
            for inst in draws.screen_instances {
                if inst.size.x <= 0.0 || inst.size.y <= 0.0 {
                    continue;
                }
                sc.draw_vars.set_texture(0, &inst.texture);
                sc.screen_pos = inst.pos;
                sc.screen_size = inst.size;
                sc.uv_rect = inst.uv;
                if sc.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&sc.draw_vars);
                    sc.draw_vars.area = cx.update_area_refs(sc.draw_vars.area, new_area);
                }
            }
        }

        // 7. View-local held meshes, after the complete world. The dedicated
        // shader maps them into a portable near-depth band, while their queue
        // never visited any world bake/caster path above.
        if let Some(draw) = draws.view_model.as_deref_mut() {
            self.draw_view_models_inner(cx, draw, &sun, &mut stats);
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        draw_list.end(cx);
        stats
    }
}

#[cfg(test)]
mod realm_lifecycle_tests {
    use super::*;

    fn model(id: &str, x: f32, dynamic: bool, depth_order: f32) -> ModelInstance {
        let mut transform = Mat4f::identity();
        transform.v[12] = x;
        ModelInstance {
            model: id.to_string(),
            transform,
            dynamic,
            depth_order,
            part_poses: Vec::new(),
        }
    }

    #[test]
    fn equal_length_static_content_changes_invalidate_placed_scene_caches() {
        let mut renderer = Renderer::default();
        let original = model("kit/lamp", 1.0, false, 0.0);
        renderer.set_models(vec![original.clone()]);
        let original_rev = renderer.models_rev;

        // An identical frame is steady state and keeps delivered remaps.
        renderer.lm_remaps.push(vec4f(1.0, 1.0, 1.0, 1.0));
        renderer.set_models(vec![original]);
        assert_eq!(renderer.models_rev, original_rev);
        assert_eq!(renderer.lm_remaps.len(), 1);

        // Same count, different transform: this was the length-only hole.
        renderer.set_models(vec![model("kit/lamp", 2.0, false, 0.0)]);
        assert_eq!(renderer.models_rev, original_rev.wrapping_add(1));
        assert!(renderer.lm_remaps.is_empty());
        let moved_rev = renderer.models_rev;

        // Model identity and static depth order are content as well.
        renderer.set_models(vec![model("kit/lantern", 2.0, false, 0.0)]);
        assert_eq!(renderer.models_rev, moved_rev.wrapping_add(1));
        let renamed_rev = renderer.models_rev;
        renderer.set_models(vec![model("kit/lantern", 2.0, false, 3.0)]);
        assert_eq!(renderer.models_rev, renamed_rev.wrapping_add(1));
    }

    #[test]
    fn pbr_material_policy_defaults_on_and_can_be_disabled() {
        let mut renderer = Renderer::default();
        assert!(renderer.pbr_materials_enabled());
        renderer.set_pbr_materials_enabled(false);
        assert!(!renderer.pbr_materials_enabled());
    }

    #[test]
    fn dynamic_motion_does_not_rebake_the_static_scene() {
        let mut renderer = Renderer::default();
        renderer.set_models(vec![model("cars/ambulance", 1.0, true, 0.0)]);
        let rev = renderer.models_rev;
        renderer.lm_remaps.push(Vec4f::default());

        renderer.set_models(vec![model("cars/ambulance", 40.0, true, 0.0)]);

        assert_eq!(renderer.models_rev, rev);
        assert_eq!(renderer.lm_remaps.len(), 1);
    }

    #[test]
    fn world_attachments_are_dynamic_but_never_enter_placed_scene_identity() {
        let mut renderer = Renderer::default();
        renderer.set_models(vec![model("town/house", 2.0, false, 0.0)]);
        let rev = renderer.models_rev;
        let signature = renderer.placed_scene_signature;
        renderer.lm_remaps.push(Vec4f::default());

        // Deliberately pass `dynamic: false`: lane ownership, not a caller
        // flag, makes attachments analytically lit moving geometry.
        renderer.set_world_attachments(vec![model("kit/lamp", 4.0, false, 0.0)]);
        assert!(WorldModelLane::Attachment.is_dynamic(&renderer.world_attachments[0]));
        assert_eq!(renderer.models_rev, rev);
        assert_eq!(renderer.placed_scene_signature, signature);
        assert_eq!(renderer.lm_remaps.len(), 1);
        assert_eq!(renderer.placed_models.len(), 1);
        assert_eq!(renderer.placed_models[0].model, "town/house");
        assert_eq!(renderer.world_attachments.len(), 1);

        // Per-frame socket motion only replaces the attachment queue.
        renderer.set_world_attachments(vec![model("kit/lamp", 40.0, false, 0.0)]);
        assert_eq!(renderer.models_rev, rev);
        assert_eq!(renderer.placed_scene_signature, signature);
        assert_eq!(renderer.lm_remaps.len(), 1);
    }

    #[test]
    fn world_attachment_light_hysteresis_has_its_own_key_namespace() {
        for slot in [0, 1, 77, u32::MAX as usize] {
            let attachment = WorldModelLane::Attachment.light_key(slot);
            assert_ne!(attachment, WorldModelLane::Placed.light_key(slot));
            assert_ne!(attachment, 0x8000_0000_0000_0000 | slot as u64);
            assert_eq!(attachment >> 62, 3);
        }
    }

    #[test]
    fn replicated_tracer_velocity_is_its_full_3d_visual_axis() {
        let dir = vec3f(0.31, 0.47, -0.83).normalize();
        let tracer = Entity {
            kind: BodyKind::Mover,
            vel: dir * 90.0,
            tag: "tracer".to_string(),
            ..Default::default()
        };

        let rotation = Renderer::entity_rotation(&tracer);
        let visual_axis = vec3f(rotation.v[8], rotation.v[9], rotation.v[10]);
        assert!(
            visual_axis.dot(dir) > 0.999_999,
            "tracer visual axis {visual_axis:?} diverged from replicated velocity {dir:?}"
        );
    }

    #[test]
    fn view_models_never_enter_world_identity_or_world_model_ownership() {
        let mut renderer = Renderer::default();
        renderer.set_models(vec![model("town/house", 2.0, false, 0.0)]);
        let rev = renderer.models_rev;
        let signature = renderer.placed_scene_signature;
        renderer.lm_remaps.push(Vec4f::default());

        renderer.set_view_models(vec![model("fps/pistol", 0.0, true, 0.0)]);
        assert_eq!(renderer.models_rev, rev);
        assert_eq!(renderer.placed_scene_signature, signature);
        assert_eq!(renderer.lm_remaps.len(), 1);
        assert_eq!(renderer.placed_models.len(), 1);
        assert_eq!(renderer.placed_models[0].model, "town/house");
        assert_eq!(renderer.view_models.len(), 1);

        // Camera-relative motion updates only the private presentation queue.
        renderer.set_view_models(vec![model("fps/pistol", 100.0, true, 0.0)]);
        assert_eq!(renderer.models_rev, rev);
        assert_eq!(renderer.placed_scene_signature, signature);
        assert_eq!(renderer.lm_remaps.len(), 1);
    }

    #[test]
    fn entering_a_realm_clears_world_identity_but_preserves_device_policy() {
        let mut renderer = Renderer::default();
        renderer.set_shadow_budget(7);
        renderer.set_stage(Stage::mr_diorama(vec3f(1.0, 2.0, 3.0), 0.4, 0.08));
        renderer.set_gpu_lightmap_mode(crate::gpu_lightmap::GpuLightmapMode::Realtime);
        let csm_config = renderer.set_csm_config(1024, 48.0);
        let mut settings = renderer.bake_settings();
        settings.ao_rays = 3;
        settings.max_probes = 19;
        renderer.set_bake_settings(settings);

        renderer.slab_key = Some((1, 1));
        renderer.slab_instance_count = 23;
        renderer.terrain_revision = 1;
        renderer.water_rev = Some(1);
        renderer.set_models(vec![model("town/house", 9.0, false, 0.0)]);
        renderer.set_world_attachments(vec![model("fps/pistol", 9.0, false, 0.0)]);
        renderer.world_attachment_ground.push(5.5);
        renderer.lm_remaps.push(vec4f(0.5, 0.5, 0.25, 0.25));
        renderer.lm_ground = Some((Vec4f::default(), Vec4f::default()));
        renderer.receiver_boxes.push((Vec3f::default(), vec3f(1.0, 1.0, 1.0)));
        renderer.occluder_boxes.push((Vec3f::default(), vec3f(1.0, 1.0, 1.0)));
        renderer.shadow_mesh.vertices.push(1.0);
        renderer.lm_lights.push(crate::lightmap::LmLight::omni(
            Vec3f::default(),
            vec3f(1.0, 0.8, 0.5),
            8.0,
        ));
        renderer.frame_lights.push(crate::lightmap::LmLight::omni(
            Vec3f::default(),
            vec3f(1.0, 1.0, 1.0),
            3.0,
        ));
        renderer.frame_baked_count = 1;
        renderer.host_lights.push(crate::lightmap::LmLight::omni(
            Vec3f::default(),
            vec3f(1.0, 1.0, 1.0),
            2.0,
        ));
        renderer.lamp_cache.push(crate::lightmap::LmLight::omni(
            Vec3f::default(),
            vec3f(1.0, 1.0, 1.0),
            4.0,
        ));
        renderer.lamp_cache_rev = Some((renderer.models_rev, 256));
        renderer.light_rank.push((1.0, 0));
        renderer.light_sel.push(0);
        renderer.light_block_scratch[0] = 1.0;
        renderer.light_cell_memory.insert(7, (2, 3));
        renderer.char_ground.push(4.0);
        renderer.model_ground.push(5.0);
        renderer.lm_kick_key = Some((1, renderer.models_rev, 32));
        renderer.shadow_points.push(vec3f(1.0, 2.0, 3.0));
        renderer.shadow_gate.built = Some((1, renderer.models_rev, 1, 32));
        let models_rev = renderer.models_rev;
        let bake_generation = renderer.bake.generation();
        let quality = renderer.quality();

        renderer.enter_realm();

        assert_eq!(renderer.slab_key, None);
        assert_eq!(renderer.slab_instance_count, 0);
        assert!(renderer.terrain_tiles.is_empty());
        assert_eq!(renderer.terrain_revision, 0);
        assert!(renderer.voxel_tiles.is_empty());
        assert!(renderer.water_tiles.is_empty());
        assert_eq!(renderer.water_rev, None);
        assert!(renderer.placed_models.is_empty());
        assert!(renderer.world_attachments.is_empty());
        assert!(renderer.world_attachment_ground.is_empty());
        assert!(renderer.view_models.is_empty());
        assert_eq!(renderer.placed_scene_signature, None);
        assert!(renderer.receiver_boxes.is_empty());
        assert!(renderer.occluder_boxes.is_empty());
        assert!(renderer.shadow_mesh.is_empty());
        assert!(renderer.lm_remaps.is_empty());
        assert_eq!(renderer.lm_ground, None);
        assert!(!renderer.gpu_baker.has_state());
        assert!(renderer.lm_lights.is_empty());
        assert!(renderer.frame_lights.is_empty());
        assert_eq!(renderer.frame_baked_count, 0);
        assert!(renderer.host_lights.is_empty());
        assert!(renderer.lamp_cache.is_empty());
        assert_eq!(renderer.lamp_cache_rev, None);
        assert!(renderer.light_rank.is_empty());
        assert!(renderer.light_sel.is_empty());
        assert_eq!(renderer.light_block_scratch, [0.0; LIGHT_BLOCK_FLOATS]);
        assert!(renderer.light_cell_memory.is_empty());
        assert!(renderer.char_ground.is_empty());
        assert!(renderer.model_ground.is_empty());
        assert_eq!(renderer.lm_kick_key, None);
        assert!(renderer.shadow_points.is_empty());
        assert!(renderer.shadow_gate.built.is_none());
        assert!(renderer.shadow_gate.pending.is_none());
        assert_eq!(renderer.models_rev, models_rev.wrapping_add(1));
        assert_eq!(renderer.bake.generation(), bake_generation.wrapping_add(1));

        assert_eq!(renderer.shadow_budget(), 7);
        assert_eq!(renderer.gpu_lightmap_mode(), crate::gpu_lightmap::GpuLightmapMode::Realtime);
        assert_eq!(renderer.csm_config(), csm_config);
        assert_eq!(renderer.bake_settings().ao_rays, 3);
        assert_eq!(renderer.bake_settings().max_probes, 19);
        assert_eq!(renderer.quality(), quality);
        assert_eq!(renderer.stage().mode, StageMode::MrDiorama);
        assert_eq!(renderer.stage().origin, vec3f(1.0, 2.0, 3.0));
        assert_eq!(renderer.stage().yaw, 0.4);
        assert_eq!(renderer.stage().scale, 0.08);
    }
}

#[cfg(test)]
mod sun_tests {
    use super::*;

    /// T7 on the game side: every game shader must read ONE sun. Before the
    /// unification each shader carried its own ambient/direct constants and
    /// five script blocks set the light direction by hand, so changing one
    /// silently left the others behind. `write_into` is the single write
    /// path — apply_sun calls it once per shader and draw_skinned_inner
    /// calls it for the skinned struct, so uniformity is compiler-enforced
    /// and this asserts the payload rather than eyeballing a capture.
    #[test]
    fn write_into_sets_every_sun_field() {
        let sun = SunLight::from_time_of_day(8.0, 52.0);
        let (mut dir, mut color, mut sky, mut ground) = (
            Vec3f::default(),
            Vec3f::default(),
            Vec3f::default(),
            Vec3f::default(),
        );
        sun.write_into(&mut dir, &mut color, &mut sky, &mut ground);
        assert_eq!(dir, sun.dir);
        assert_eq!(color, sun.color);
        assert_eq!(sky, sun.sky);
        assert_eq!(ground, sun.ground);
    }

    /// Two shaders fed by the same sun end up with identical values — the
    /// property that used to fail silently.
    #[test]
    fn two_targets_receive_identical_values() {
        let sun = SunLight::from_time_of_day(17.0, 52.0);
        let mut a = [Vec3f::default(); 4];
        let mut b = [Vec3f::default(); 4];
        let [a0, a1, a2, a3] = &mut a;
        sun.write_into(a0, a1, a2, a3);
        let [b0, b1, b2, b3] = &mut b;
        sun.write_into(b0, b1, b2, b3);
        assert_eq!(a, b);
    }

    /// A default world must light exactly as the pre-unification shaders
    /// did, so adopting SceneSun did not restyle every existing game.
    #[test]
    fn the_default_sun_is_the_legacy_look() {
        let sun = crate::sun::resolve_sun(&makepad_game_sim::SunConfig::default());
        assert_eq!(sun, SunLight::default());
        // Flat hemisphere collapses mix(ground, sky, h) to the old constant.
        assert_eq!(sun.sky, sun.ground);
    }

}

#[cfg(test)]
mod cull_tests {
    use super::*;

    /// The camera the sandbox actually flies: perspective like scene.rs
    /// builds (near 1, far 500), eye at +10z looking at the origin down -z.
    fn frustum() -> Frustum {
        let view = Mat4f::look_at(vec3f(0.0, 0.0, 10.0), vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0));
        let projection = Mat4f::perspective(60.0, 1.0, 1.0, 500.0);
        Frustum::from_clip_matrix(&Mat4f::mul(&projection, &view))
    }

    fn unit_box_at(pos: Vec3f) -> (Vec3f, Vec3f, Mat4f) {
        let mut t = Mat4f::identity();
        t.v[12] = pos.x;
        t.v[13] = pos.y;
        t.v[14] = pos.z;
        (vec3f(-1.0, -1.0, -1.0), vec3f(1.0, 1.0, 1.0), t)
    }

    /// The plane extraction is validated against the definition of clip
    /// space rather than against itself: a point is in the frustum iff its
    /// clip coordinates satisfy |x|,|y| <= w and -w <= z <= w, and the
    /// extracted planes must agree (via a zero-size box at that point).
    /// Margins make the plane side a touch looser — never tighter.
    #[test]
    fn planes_agree_with_clip_space() {
        let view = Mat4f::look_at(vec3f(3.0, 2.0, 10.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 1.0, 0.0));
        let projection = Mat4f::perspective(72.0, 1.6, 1.0, 500.0);
        let clip = Mat4f::mul(&projection, &view);
        let fr = Frustum::from_clip_matrix(&clip);
        for i in 0..125 {
            let p = vec3f(
                ((i % 5) as f32 - 2.0) * 40.0,
                (((i / 5) % 5) as f32 - 2.0) * 40.0,
                ((i / 25) as f32 - 2.0) * 40.0,
            );
            let c = clip.transform_vec4(vec4(p.x, p.y, p.z, 1.0));
            let inside = c.x.abs() <= c.w && c.y.abs() <= c.w && c.z.abs() <= c.w && c.w > 0.0;
            if inside {
                assert!(
                    fr.intersects_obb(p, p, &Mat4f::identity()),
                    "visible point culled at {:?}",
                    p
                );
            }
        }
    }

    #[test]
    fn a_box_behind_the_camera_culls() {
        let fr = frustum();
        // Eye is at z=10 looking toward -z; z=20 is squarely behind it.
        let (min, max, t) = unit_box_at(vec3f(0.0, 0.0, 20.0));
        assert!(!fr.intersects_obb(min, max, &t));
        assert!(!fr.intersects_sphere(vec3f(0.0, 0.0, 20.0), 1.0));
    }

    #[test]
    fn a_box_in_front_does_not_cull() {
        let fr = frustum();
        let (min, max, t) = unit_box_at(vec3f(0.0, 0.0, 0.0));
        assert!(fr.intersects_obb(min, max, &t));
        assert!(fr.intersects_sphere(vec3f(0.0, 0.0, 0.0), 1.0));
    }

    #[test]
    fn a_box_straddling_a_plane_does_not_cull() {
        let fr = frustum();
        // At 10 units depth with a 60-degree fov the left plane sits at
        // x ~= -5.77; a box spanning -100..0 in x straddles it.
        let t = Mat4f::identity();
        assert!(fr.intersects_obb(
            vec3f(-100.0, -1.0, -1.0),
            vec3f(0.0, 1.0, 1.0),
            &t
        ));
        // And one that swallows the whole frustum stays, too.
        assert!(fr.intersects_obb(
            vec3f(-1000.0, -1000.0, -1000.0),
            vec3f(1000.0, 1000.0, 1000.0),
            &t
        ));
    }

    #[test]
    fn a_box_fully_beside_the_frustum_culls() {
        let fr = frustum();
        let (min, max, t) = unit_box_at(vec3f(-100.0, 0.0, 0.0));
        assert!(!fr.intersects_obb(min, max, &t));
        // …and past the far plane (far = 500 from the eye at z=10).
        let (min, max, t) = unit_box_at(vec3f(0.0, 0.0, -600.0));
        assert!(!fr.intersects_obb(min, max, &t));
    }

    /// The rotation path: bounds are model-space, so the corners must go
    /// through the instance transform before the plane test. This box is
    /// beside the frustum if only its translation were applied (x 10..500 at
    /// ~10 units depth, right plane at ~6.3), but yaw 90 lays it along -z,
    /// straddling the far plane dead ahead — it must survive.
    #[test]
    fn a_rotated_box_is_tested_in_world_space() {
        let fr = frustum();
        let t = Mat4f::rotation(vec3f(0.0, std::f32::consts::FRAC_PI_2, 0.0));
        assert!(fr.intersects_obb(
            vec3f(10.0, -0.5, -0.5),
            vec3f(500.0, 0.5, 0.5),
            &t
        ));
        // Sanity of the premise: unrotated it culls.
        assert!(!fr.intersects_obb(
            vec3f(10.0, -0.5, -0.5),
            vec3f(500.0, 0.5, 0.5),
            &Mat4f::identity()
        ));
    }

    #[test]
    fn aabb_test_matches_the_corner_test() {
        let fr = frustum();
        // In front stays, behind culls, straddler stays — the same contract
        // the OBB corners give, via the positive-vertex shortcut.
        assert!(fr.intersects_aabb(vec3f(-1.0, -1.0, -1.0), vec3f(1.0, 1.0, 1.0)));
        assert!(!fr.intersects_aabb(vec3f(-1.0, -1.0, 19.0), vec3f(1.0, 1.0, 21.0)));
        assert!(fr.intersects_aabb(vec3f(-100.0, -1.0, -1.0), vec3f(0.0, 1.0, 1.0)));
        assert!(!fr.intersects_aabb(vec3f(-101.0, -1.0, -1.0), vec3f(-99.0, 1.0, 1.0)));
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    #[test]
    fn chunk_cell_floors_negative_coordinates() {
        assert_eq!(chunk_cell(0.0, 0.0), (0, 0));
        assert_eq!(chunk_cell(CHUNK_SIZE - 0.01, CHUNK_SIZE - 0.01), (0, 0));
        assert_eq!(chunk_cell(CHUNK_SIZE, 0.0), (1, 0));
        // Truncation would fold cell -1 onto cell 0 and merge two cells'
        // content bounds across the origin.
        assert_eq!(chunk_cell(-0.01, -0.01), (-1, -1));
        assert_eq!(chunk_cell(-CHUNK_SIZE, -CHUNK_SIZE), (-1, -1));
        assert_eq!(chunk_cell(-CHUNK_SIZE - 0.01, 0.0), (-2, 0));
    }

    /// The debounce contract: the FIRST build is immediate (nothing stale
    /// exists to show), an edit burst coalesces into ONE rebuild once the
    /// world has been still for the settle window, and a key that keeps
    /// moving keeps the rebuild parked.
    #[test]
    fn shadow_gate_coalesces_an_edit_burst() {
        use std::time::{Duration, Instant};
        let settle = Duration::from_millis(200);
        let t0 = Instant::now();
        let mut gate = ShadowRebuildGate::default();
        // First sight builds immediately.
        assert!(gate.should_rebuild((1, 0, 0, 0), t0, settle));
        gate.mark_built((1, 0, 0, 0));
        assert!(!gate.should_rebuild((1, 0, 0, 0), t0, settle));
        // Burst: five mutations in quick succession — no rebuild during it,
        // and the settle clock restarts on every change.
        for i in 2..7u64 {
            let now = t0 + Duration::from_millis(10 * i);
            assert!(!gate.should_rebuild((i, 0, 0, 0), now, settle));
        }
        // Still pending just before the window closes...
        let last_change = t0 + Duration::from_millis(60);
        assert!(!gate.should_rebuild((6, 0, 0, 0), last_change + Duration::from_millis(199), settle));
        // ...and exactly one rebuild once it has.
        let at_rest = last_change + Duration::from_millis(200);
        assert!(gate.should_rebuild((6, 0, 0, 0), at_rest, settle));
        gate.mark_built((6, 0, 0, 0));
        assert!(!gate.should_rebuild((6, 0, 0, 0), at_rest + Duration::from_millis(1000), settle));
    }

    /// Tiling must regroup the terrain mesh, not change it: the union of
    /// every tile's triangles is byte-identical (as a multiset of emitted
    /// vertices) to the whole-mesh emission.
    #[test]
    fn terrain_tiles_union_to_the_whole_mesh() {
        let n = 5;
        let terrain = Terrain {
            cells: n,
            cell_size: 30.0,
            origin: -60.0,
            heights: (0..n * n).map(|i| (i as f32 * 0.7).sin() * 3.0).collect(),
            colors: (0..n * n)
                .map(|i| vec4(i as f32 / 25.0, 0.5, 0.25, 1.0))
                .collect(),
            revision: 1,
        };
        let vertex_multiset = |vertices: &[f32]| {
            let mut set: Vec<Vec<u32>> = vertices
                .chunks_exact(16)
                .map(|v| v.iter().map(|f| f.to_bits()).collect())
                .collect();
            set.sort();
            set
        };
        let (whole_verts, whole_idx, ..) = terrain_tile_data(&terrain, None, 0, n - 1, 0, n - 1);
        // 30-unit cells against 48-unit tiles: one cell per tile, 4x4 tiles.
        let cells_per_tile = ((CHUNK_SIZE / terrain.cell_size) as usize).max(1);
        assert_eq!(cells_per_tile, 1);
        let mut tiled_verts = Vec::new();
        let mut tiled_idx_count = 0;
        for gz in (0..n - 1).step_by(cells_per_tile) {
            for gx in (0..n - 1).step_by(cells_per_tile) {
                let (v, i, min, max) = terrain_tile_data(
                    &terrain,
                    None,
                    gx,
                    (gx + cells_per_tile).min(n - 1),
                    gz,
                    (gz + cells_per_tile).min(n - 1),
                );
                // Tile bounds contain the tile's own vertices.
                for p in v.chunks_exact(16) {
                    assert!(p[0] >= min.x && p[0] <= max.x);
                    assert!(p[1] >= min.y && p[1] <= max.y);
                    assert!(p[2] >= min.z && p[2] <= max.z);
                }
                tiled_idx_count += i.len();
                tiled_verts.extend_from_slice(&v);
            }
        }
        assert_eq!(tiled_idx_count, whole_idx.len());
        assert_eq!(vertex_multiset(&tiled_verts), vertex_multiset(&whole_verts));
    }
}

#[cfg(test)]
mod light_tests {
    use super::*;
    use crate::lightmap::LmLight;

    fn omni(x: f32, y: f32, z: f32, intensity: f32, radius: f32) -> LmLight {
        LmLight::omni(vec3f(x, y, z), vec3f(intensity, intensity, intensity), radius)
    }

    /// The village's own numbers: its fixed midday sun and one harvested
    /// street lamp — bulb at ~2.82 on a 3.2-unit pole, photometry solved by
    /// `lightmap::lamp_photometry` exactly as `harvest_lamps` does it.
    fn village_sun() -> crate::sun::SunLight {
        crate::sun::SunLight {
            dir: vec3f(0.55, 0.56, 0.62).normalize(),
            ..Default::default()
        }
    }

    /// The same village at the hour a street lamp is FOR: sun on its way
    /// down, a few degrees up.
    fn dusk_sun() -> crate::sun::SunLight {
        crate::sun::SunLight {
            dir: vec3f(0.55, 0.1, 0.62).normalize(),
            ..Default::default()
        }
    }

    fn street_lamp() -> LmLight {
        let (radius, strength) = crate::lightmap::lamp_photometry(2.82);
        LmLight {
            pos: vec3f(0.0, 2.82, 0.0),
            color: vec3f(strength, strength * 0.775, strength * 0.475),
            radius,
            dir: vec3f(0.0, -1.0, 0.0),
            spot: 1.0,
        }
    }

    fn flat() -> Receiver<'static> {
        Receiver { base_y: 0.0, terrain: None, statics: &[] }
    }

    /// A caster on grass at the edge of a PROUD road slab, sun laying the
    /// silhouette across the slab: the quad plane must rise to the slab's
    /// top, or the slab depth-buries the whole silhouette and the shadow
    /// vanishes at exactly that heading (and flickers with camera angle at
    /// the slab's edge) — the walking-into-the-road report this pins.
    #[test]
    fn sdf_quad_rides_a_raised_receiver_under_the_silhouette() {
        // Road slab top 8 cm proud, starting 0.5 units down-sun of the
        // anchor — the silhouette's far half lands on it.
        let slabs = [(vec3f(-8.0, -1.0, -8.0), vec3f(-0.5, 0.08, 8.0))];
        let receiver = Receiver { base_y: 0.0, terrain: None, statics: &slabs };
        let anchor = vec3f(0.0, 0.0, 0.0);
        // Light from +x: the silhouette runs toward -x, onto the slab.
        let y = sdf_quad_ground(anchor, &receiver, 1.0, 0.0, 2.0);
        assert!(
            (y - 0.08).abs() < 1.0e-6,
            "quad must ride the slab top, got y = {y}"
        );
        // Silhouette running AWAY from the slab (toward +x): stays put.
        let y = sdf_quad_ground(anchor, &receiver, -1.0, 0.0, 2.0);
        assert_eq!(y, 0.0, "no slab under the run — plane must not move");
        // No statics at all: the anchor's own height wins.
        let y = sdf_quad_ground(vec3f(0.0, 0.3, 0.0), &flat(), 1.0, 0.0, 2.0);
        assert_eq!(y, 0.3);
    }

    /// Standing 1.5 units from a lit street lamp at DUSK, the lamp must
    /// visibly own the shadow: the LEAN points along the anti-lamp azimuth
    /// by a readable amount (sub-0.2-unit leans read as nothing — the report
    /// this pins), the shadow darkens — and the ROOT stays at the boots,
    /// because a lean redirects the silhouette's body, never its contact.
    /// This is the live scene's exact arithmetic, so if a tuning change makes
    /// the lean invisible again, this fails before a user says it.
    ///
    /// Dusk, not noon: ownership is decided by which source is actually
    /// lighting the character, and once a lamp emits what a lamp emits
    /// (`lightmap::LM_LAMP_GROUND_PEAK`, well under the sun's 0.72 direct
    /// term) it takes the shadow as the sun goes down — never at midday.
    /// `the_midday_sun_keeps_the_shadow_from_a_lamp` pins the other half.
    #[test]
    fn a_dusk_street_lamp_visibly_leans_a_nearby_shadow() {
        let lights = [street_lamp()];
        let feet = vec3f(1.5, 0.0, 0.0);
        let a = character_shadow_anchor(feet, &flat(), &dusk_sun(), &lights)
            .expect("grounded character must have a shadow");
        assert!(
            a.lamp_w > 0.5,
            "beside the pole at dusk the lamp should dominate, lamp_w = {}",
            a.lamp_w
        );
        assert!(
            a.lean.x > 0.4,
            "the lean must point away from the lamp and be readable \
             (got {:?}, want x > 0.4)",
            a.lean
        );
        assert!(a.lean.y.abs() < 0.05, "off-azimuth lean: {:?}", a.lean);
        // THE contract the lean must never break: grounded, the foot end
        // of the shadow is the caster's own contact point (the floating
        // sideways-silhouette report this pins).
        assert!(
            (a.root.x - feet.x).abs() < 1.0e-6 && (a.root.z - feet.z).abs() < 1.0e-6,
            "a grounded lamp lean must not move the root, got {:?}",
            a.root
        );
        // Darkened over the plain sun shadow at the same spot.
        let plain = character_shadow_anchor(feet, &flat(), &dusk_sun(), &[])
            .expect("baseline");
        assert!(a.alpha > plain.alpha, "a dominant lamp must darken the fan");
        assert_eq!(plain.lamp_w, 0.0);
        // And the baseline grounded root stays under the feet too.
        assert!((plain.root.x - feet.x).abs() < 1.0e-6);
        // The sun-baked silhouette compresses toward the feet as the lamp
        // takes over; the plain sun shadow keeps its full size.
        assert!(
            a.size_mul < 0.75,
            "a dominant lamp must compress the fan, size_mul = {}",
            a.size_mul
        );
        assert!((plain.size_mul - 1.0).abs() < 1.0e-6);
    }

    /// Directly under the bulb after sundown: no direction to lean, so lean
    /// AND root stay pinned at the feet — but the lamp owns the shadow
    /// outright, so it must be strongly compressed and darkened, never the
    /// sun's long dusk silhouette parked under a lamp (the report this pins).
    #[test]
    fn under_the_bulb_the_shadow_pins_small_at_the_feet() {
        let lights = [street_lamp()];
        let feet = vec3f(0.1, 0.0, 0.0);
        for sun in [
            {
                let mut s = village_sun();
                s.dir = vec3f(0.55, 0.05, 0.62).normalize();
                s
            },
            {
                let mut s = village_sun();
                s.dir = vec3f(0.55, 0.02, 0.62).normalize();
                s
            },
        ] {
            let a = character_shadow_anchor(feet, &flat(), &sun, &lights)
                .expect("anchor");
            assert!(a.lamp_w > 0.85, "under the bulb lamp_w = {}", a.lamp_w);
            assert!(
                a.lean.x.abs() < 0.05 && a.lean.y.abs() < 0.05,
                "no lean under the bulb, got {:?}",
                a.lean
            );
            assert!(
                (a.root.x - feet.x).abs() < 1.0e-6
                    && (a.root.z - feet.z).abs() < 1.0e-6,
                "grounded root must sit at the feet, got {:?}",
                a.root
            );
            assert!(
                a.size_mul < 0.55,
                "the shadow must compress under the bulb, size_mul = {}",
                a.size_mul
            );
        }
    }

    /// The other half of the ownership contract, and the one the overbright
    /// bake used to get wrong: at MIDDAY the sun keeps the shadow. A street
    /// lamp emits a fraction of daylight (`lightmap::LM_LAMP_GROUND_PEAK`
    /// against the sun's 0.72 direct term), so a character beside a pole at
    /// noon casts ONE shadow, pointing away from the sun.
    ///
    /// This failed before the photometry rewrite: the harvested lamp was
    /// pinned at the atlas's 2.0 encode ceiling, which put 0.87 on the ground
    /// — brighter than noon — and let a lamp swing shadows in broad daylight.
    #[test]
    fn the_midday_sun_keeps_the_shadow_from_a_lamp() {
        // The frame never shows an unrailed lamp: static_lights_for scales
        // every baked light by the daylight headroom before anything —
        // anchor weighting included — sees it. The night peak raise made
        // railing here load-bearing: the raw 0.72-class strength would
        // out-vote a noon sun no shipped frame ever pits it against.
        let sun = village_sun();
        let mut lamp = street_lamp();
        let s = crate::lightmap::lamp_daylight_scale(crate::lightmap::daylight_on_ground(
            sun.dir, sun.color, sun.sky,
        ));
        lamp.color = lamp.color * s;
        let lights = [lamp];
        for feet in [vec3f(1.5, 0.0, 0.0), vec3f(0.1, 0.0, 0.0)] {
            let a = character_shadow_anchor(feet, &flat(), &sun, &lights).expect("anchor");
            assert!(
                a.lamp_w < 0.35,
                "a lamp must not own a noon shadow, lamp_w = {} at {feet:?}",
                a.lamp_w
            );
            // Still the sun's own silhouette: pointing anti-sun, full size.
            let plain = character_shadow_anchor(feet, &flat(), &sun, &[]).expect("baseline");
            assert!(
                (a.size_mul - plain.size_mul).abs() < 0.35,
                "the noon silhouette must keep its size, {} vs {}",
                a.size_mul,
                plain.size_mul
            );
        }
    }

    /// Sun on the horizon (night edge): the lamp saturates and the LEAN is
    /// the bulb's TRUE mid-body projection, not a fraction of it — while
    /// the root never leaves the boots.
    #[test]
    fn at_night_the_lamp_owns_the_shadow_outright() {
        let mut sun = village_sun();
        sun.dir = vec3f(0.55, 0.02, 0.62).normalize();
        let lights = [street_lamp()];
        let feet = vec3f(1.5, 0.0, 0.0);
        let a = character_shadow_anchor(feet, &flat(), &sun, &lights).expect("anchor");
        assert!(a.lamp_w > 0.95, "night lamp_w = {}", a.lamp_w);
        // True projection: rho * MID / (bulb - MID) = 1.5*0.9/1.92 = 0.703.
        assert!(
            (a.lean.x - 0.703).abs() < 0.05,
            "night lean should be the full projection (~0.70), got {}",
            a.lean.x
        );
        assert!(
            (a.root.x - feet.x).abs() < 1.0e-6 && (a.root.z - feet.z).abs() < 1.0e-6,
            "the night lean must not detach the root from the boots, got {:?}",
            a.root
        );
    }

    /// The whole-policy pinning contract in one sweep: for EVERY grounded
    /// scenario — no lamp, day lamp beside, night lamp beside, dead under
    /// the bulb — the silhouette's foot end is the caster's own contact
    /// point. Only height may move it: a sun jump slides the root down the
    /// anti-sun azimuth by exactly `h/sun.y`, and a lamp-owned jump slides
    /// it by the projection's height growth — never by the standing lean.
    #[test]
    fn the_shadow_root_never_leaves_the_feet_on_the_ground() {
        let lights = [street_lamp()];
        let night = {
            let mut s = village_sun();
            s.dir = vec3f(0.55, 0.02, 0.62).normalize();
            s
        };
        let cases: [(&str, Vec3f, &[crate::lightmap::LmLight], &SunLight); 4] = [
            ("no lamp", vec3f(3.0, 0.0, 1.0), &[], &village_sun()),
            ("day lamp beside", vec3f(1.5, 0.0, 0.0), &lights, &village_sun()),
            ("night lamp beside", vec3f(1.5, 0.0, 0.0), &lights, &night),
            ("under the bulb", vec3f(0.1, 0.0, 0.0), &lights, &night),
        ];
        for (name, feet, lights, sun) in cases {
            let a = character_shadow_anchor(feet, &flat(), sun, lights)
                .unwrap_or_else(|| panic!("{name}: anchor"));
            let d = ((a.root.x - feet.x).powi(2) + (a.root.z - feet.z).powi(2)).sqrt();
            assert!(
                d < 1.0e-6,
                "{name}: grounded root drifted {d} units from the feet"
            );
        }
        // Airborne under the sun alone: the root carries the whole height
        // projection — the jump shadow still slides off the feet.
        let sun = village_sun();
        let feet = vec3f(3.0, 1.2, 1.0);
        let a = character_shadow_anchor(feet, &flat(), &sun, &[]).expect("air anchor");
        let expect = vec2f(
            -sun.dir.x / sun.dir.y.max(0.2) * 1.2,
            -sun.dir.z / sun.dir.y.max(0.2) * 1.2,
        );
        assert!(
            (a.root.x - (feet.x + expect.x)).abs() < 1.0e-5
                && (a.root.z - (feet.z + expect.y)).abs() < 1.0e-5,
            "sun jump must slide the root by the full height projection"
        );
        // Airborne beside the night lamp: the root moves only by the
        // projection's GROWTH with height, which is strictly less than the
        // standing lean it explicitly excludes.
        let feet = vec3f(1.5, 0.8, 0.0);
        let a = character_shadow_anchor(feet, &flat(), &night, &lights)
            .expect("lamp air anchor");
        let slide = ((a.root.x - feet.x).powi(2) + (a.root.z - feet.z).powi(2)).sqrt();
        assert!(
            slide > 0.05,
            "a lamp-owned jump must still slide the shadow, slide = {slide}"
        );
        assert!(
            slide < a.lean.x,
            "the airborne root slide ({slide}) must stay under the full \
             standing lean ({}) it excludes",
            a.lean.x
        );
    }

    /// The car SDF sprite's tilt/air gate: flat-and-grounded draws the
    /// sprite; a rolled car or one launched off a ramp falls back to the
    /// blob (the atlas bakes the car FLAT, so a flat sprite under a tilted
    /// car is a lie).
    #[test]
    fn tilted_or_airborne_cars_fall_back_to_the_blob() {
        assert!(car_sprite_allowed(1.0, 0.0), "parked flat");
        assert!(car_sprite_allowed(0.95, 0.4), "moderate ramp");
        assert!(!car_sprite_allowed(0.5, 0.0), "rolled onto its side");
        assert!(!car_sprite_allowed(1.0, 3.0), "big air");
        assert!(!car_sprite_allowed(0.3, 4.0), "both at once");
    }

    /// Out of the lamp's radius nothing leans — the pavement case that made
    /// "the lean never fires" so easy to reproduce: the south-verge walk is
    /// 8+ units from the north-verge bulbs, outside radius 8.
    #[test]
    fn out_of_radius_the_sun_keeps_the_shadow() {
        let lights = [street_lamp()];
        let feet = vec3f(8.5, 0.0, 0.0);
        let a = character_shadow_anchor(feet, &flat(), &village_sun(), &lights)
            .expect("anchor");
        assert_eq!(a.lamp_w, 0.0);
        assert!((a.root.x - feet.x).abs() < 1.0e-6);
        assert!(a.lean.x.abs() < 1.0e-6 && a.lean.y.abs() < 1.0e-6);
    }

    /// The core contract: up to 8 slots, strongest (intensity × attenuation
    /// at the nearest anchor) first, and a light whose radius reaches no
    /// anchor never makes the list however bright it is.
    #[test]
    fn selection_takes_the_strongest_eight_and_rejects_by_radius() {
        let mut lights = Vec::new();
        // Ten candidates in a row, each 2 units further away, same radius.
        for i in 0..10 {
            lights.push(omni(2.0 * i as f32, 3.0, 0.0, 1.0, 40.0));
        }
        // A blazing light whose radius cannot reach the anchor.
        lights.push(omni(100.0, 3.0, 0.0, 50.0, 5.0));
        let anchors = [vec3f(0.0, 0.0, 0.0)];
        let (mut rank, mut sel) = (Vec::new(), Vec::new());
        select_strongest_lights(
            &lights,
            0..lights.len(),
            &anchors,
            MAX_DYNAMIC_LIGHTS,
            &mut rank,
            &mut sel,
        );
        assert_eq!(sel.len(), 8, "hard cap at 8");
        assert!(!sel.contains(&10), "out-of-radius light must be rejected");
        // Nearest first: identical intensity and radius, so attenuation
        // orders them by distance.
        assert_eq!(&sel[..3], &[0, 1, 2]);
        // The two furthest in-radius candidates are the ones cut.
        assert!(!sel.contains(&8) && !sel.contains(&9));
    }

    /// Intensity can outrank proximity: a far bright light beats a near dim
    /// one when the attenuation gap does not cancel it.
    #[test]
    fn selection_weighs_intensity_against_attenuation() {
        let lights = vec![
            omni(4.0, 0.0, 0.0, 0.1, 50.0),  // near, dim
            omni(10.0, 0.0, 0.0, 10.0, 50.0), // further, blazing
        ];
        let anchors = [vec3f(0.0, 0.0, 0.0)];
        let (mut rank, mut sel) = (Vec::new(), Vec::new());
        select_strongest_lights(&lights, 0..2, &anchors, 8, &mut rank, &mut sel);
        assert_eq!(sel[0], 1, "brightness must be able to win");
    }

    /// Multi-anchor: a light near ANY anchor counts — the batch selection
    /// must not starve a lamp that owns one far-flung character.
    #[test]
    fn selection_uses_the_nearest_anchor() {
        let lights = vec![omni(100.0, 0.0, 0.0, 1.0, 8.0)];
        let far_only = [vec3f(0.0, 0.0, 0.0)];
        let with_near = [vec3f(0.0, 0.0, 0.0), vec3f(99.0, 0.0, 0.0)];
        let (mut rank, mut sel) = (Vec::new(), Vec::new());
        select_strongest_lights(&lights, 0..1, &far_only, 8, &mut rank, &mut sel);
        assert!(sel.is_empty());
        select_strongest_lights(&lights, 0..1, &with_near, 8, &mut rank, &mut sel);
        assert_eq!(sel, vec![0]);
    }

    /// Appending a second ranked range (the transients-then-lamps split the
    /// prop batch uses) respects the remaining slot budget and never
    /// duplicates an index.
    #[test]
    fn split_selection_fills_remaining_slots_without_duplicates() {
        let mut lights = Vec::new();
        for i in 0..6 {
            lights.push(omni(i as f32, 0.0, 0.0, 1.0, 30.0)); // "lamps"
        }
        for i in 0..6 {
            lights.push(omni(0.5 + i as f32, 1.0, 0.0, 2.0, 30.0)); // "transients"
        }
        let anchors = [vec3f(0.0, 0.0, 0.0)];
        let (mut rank, mut sel) = (Vec::new(), Vec::new());
        // Transients first (indices 6..12), then lamps fill what is left.
        select_strongest_lights(&lights, 6..12, &anchors, MAX_DYNAMIC_LIGHTS, &mut rank, &mut sel);
        let split = sel.len();
        assert_eq!(split, 6);
        select_strongest_lights(
            &lights,
            0..6,
            &anchors,
            MAX_DYNAMIC_LIGHTS - split,
            &mut rank,
            &mut sel,
        );
        assert_eq!(sel.len(), 8);
        let mut seen = sel.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 8, "no duplicate slots");
        assert!(sel[..6].iter().all(|i| *i >= 6), "transients keep the prefix");
        assert!(sel[6..].iter().all(|i| *i < 6), "lamps fill the tail");
    }

    /// The firework flash: nothing while climbing, a bright pop at the
    /// burst, decaying over the shell's life, coloured like the shell.
    #[test]
    fn firework_flash_derivation() {
        let mk = |age: f32| crate::firework::FireworkInstance {
            origin: vec3f(10.0, 35.0, -20.0),
            age,
            life: 4.0,
            speed: 30.0,
            seed: 1.0,
            color: vec4(1.0, 0.4, 0.2, 1.0),
            color_tail: vec4(1.0, 0.2, 0.1, 1.0),
            launch: vec3f(10.0, 0.5, -20.0),
            style: 0.0,
        };
        assert!(firework_flash_light(&mk(-0.5)).is_none(), "climbing shell has no flash");
        assert!(firework_flash_light(&mk(4.5)).is_none(), "expired shell has no flash");
        let burst = firework_flash_light(&mk(0.0)).expect("burst flash");
        let late = firework_flash_light(&mk(3.0)).expect("late glow");
        assert_eq!(burst.pos, vec3f(10.0, 35.0, -20.0), "flash sits at the burst point");
        assert!(burst.radius > 0.0 && burst.spot == 0.0, "omni with a real radius");
        assert!(
            light_intensity(&burst) > 4.0 * light_intensity(&late),
            "flash must decay over the shell's life: {} vs {}",
            light_intensity(&burst),
            light_intensity(&late)
        );
        // Shell hue carried through: red-dominant shell, red-dominant flash.
        assert!(burst.color.x > burst.color.y && burst.color.y > burst.color.z);
    }
}

#[cfg(test)]
mod water_sheet_tests {
    use super::*;
    use makepad_game_sim::WaterWave;

    fn test_volume() -> WaterVolume {
        let mut wave = WaterWave::new(0.6, -0.8, 0.7, 18.0, 5.0);
        wave.phase = 1.25;
        wave.group = 4.0;
        WaterVolume {
            min: vec3f(-10.0, -5.0, -10.0),
            max: vec3f(10.0, 0.0, 10.0),
            density: 1.0,
            current: vec3f(0.0, 0.0, 0.0),
            waves: vec![wave],
            color: vec4(0.2, 0.5, 0.8, 0.6),
            entity: 0,
        }
    }

    /// The W1 CPU/GPU agreement gate, pinned structurally (a headless unit
    /// test cannot run the GPU — documented approach):
    ///
    /// (a) the uniform packer hands the shader the sim's RAW `WaterWave`
    ///     fields, bit-for-bit, in the documented slot layout — no unit
    ///     conversion exists to drift;
    /// (b) the shader source evaluates the EXACT canonical expression the
    ///     sim documents (`sim::water::wave_terms`: phase, set envelope,
    ///     height, slope — including the same deliberate omission of the
    ///     envelope derivative) over those slots, asserted as source text.
    ///
    /// Editing either side of the agreement forces an edit here, which is
    /// the agreement being enforced.
    #[test]
    fn the_shader_evaluates_the_sims_wave_expression_over_the_sims_coefficients() {
        let volume = test_volume();
        let w = volume.waves[0];
        let (a, b) = pack_wave_uniforms(&volume);
        assert_eq!(a[0], [w.dir_x, w.dir_z, w.k, w.omega], "wave_a slot layout");
        assert_eq!(b[0], [w.amp, w.phase, w.group, 0.0], "wave_b slot layout");
        for i in 1..MAX_WAVES {
            assert_eq!(a[i], [0.0; 4], "unused wave_a slot {i} must stay zero");
            assert_eq!(b[i], [0.0; 4], "unused wave_b slot {i} must stay zero");
        }

        let src = include_str!("shaders.rs");
        let at = src.find("mod.draw.DrawSceneWater").expect("water shader missing");
        let end = src[at..].find("\n    mod.draw.").map_or(src.len(), |e| at + e);
        let decl = &src[at..end.max(at + 1)];
        for expr in [
            // phase = k·(dir·p) − ω·t + phase0
            "let phase = wa.z * (wa.x * p.x + wa.y * p.y) - wa.w * t + wb.y",
            // set envelope e², e = ½ + ½·cos(phase/group)
            "let e = 0.5 + 0.5 * cos(phase / wb.z)",
            "env = e * e",
            // slope shared term (envelope derivative omitted, like the CPU)
            "let slope = wb.x * env * cos(phase) * wa.z",
            // (height, dh/dx, dh/dz)
            "return vec3(wb.x * env * sin(phase), slope * wa.x, slope * wa.y)",
            // one shared time base
            "let t = self.water_params.y",
        ] {
            assert!(
                decl.contains(expr),
                "water shader lost the canonical expression `{expr}` — update \
                 sim::water and this pin TOGETHER or physics and visuals split"
            );
        }
        // Slot count stays in lockstep with the sim's cap and the renderer's
        // uniform id tables.
        assert_eq!(MAX_WAVES, 8, "MAX_WAVES moved: update wave_a*/wave_b* slots");
        assert!(decl.contains("wave_a7:") && !decl.contains("wave_a8"));
    }

    #[test]
    fn the_sheet_grid_spans_the_volume_at_the_still_level() {
        let volume = test_volume();
        let (vertices, indices, min, max) = water_sheet_data(&volume);
        assert!(!indices.is_empty());
        assert_eq!(vertices.len() % 16, 0, "PbrVertex stride");
        // Every vertex sits at the still level inside the bounds; the wave
        // headroom lives in the culling box, not the mesh.
        for v in vertices.chunks_exact(16) {
            assert_eq!(v[1], volume.level());
            assert!(v[0] >= volume.min.x - 1.0e-3 && v[0] <= volume.max.x + 1.0e-3);
            assert!(v[2] >= volume.min.z - 1.0e-3 && v[2] <= volume.max.z + 1.0e-3);
            // The volume's color rides per vertex (alpha = translucency).
            assert_eq!(v[11], volume.color.w);
        }
        assert!(min.y < volume.level() && max.y > volume.level());
        // Indices address real vertices.
        let count = (vertices.len() / 16) as u32;
        assert!(indices.iter().all(|i| *i < count));
    }
}

#[cfg(test)]
mod shadow_sdf_sidecar_tests {
    use super::*;

    /// A tiny valid atlas + its glb stand-in on disk, with the sidecar
    /// stamped strictly NEWER than the glb (the fresh case the gates are
    /// then tightened around one axis at a time).
    fn fixture(
        dir: &std::path::Path,
        hash: u64,
        sun: &SunLight,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let glb = dir.join("caster.glb");
        let sidecar = dir.join("caster.glb.shadowsdf");
        std::fs::write(&glb, b"not really a glb - only its mtime matters").unwrap();
        let atlas = crate::shadow_sdf::ShadowSdfAtlas {
            pixels: vec![
                200u8;
                crate::shadow_sdf::SDF_YAWS
                    * crate::shadow_sdf::SDF_CELL
                    * crate::shadow_sdf::SDF_CELL
            ],
            rows: 1,
            rect: (-1.0, -1.0, 2.0, 2.0),
            band_world: 0.25,
            len_per_unit: sun.shadow_len_per_unit(),
        };
        std::fs::write(&sidecar, atlas.to_shadowsdf(hash)).unwrap();
        // mtimes: glb one minute in the past, sidecar now — unambiguous
        // even on filesystems with coarse timestamps.
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&glb)
            .unwrap()
            .set_modified(past)
            .unwrap();
        (glb, sidecar)
    }

    /// Every gate in `Renderer::load_shadow_sdf_sidecar`: fresh +
    /// keyed + same-sun loads; a stale mtime, a foreign hash, or a
    /// different sun each falls back to `None` (= the off-thread bake).
    #[test]
    fn sidecar_gates_hold() {
        let dir = std::env::temp_dir().join(format!("shadowsdf_gates_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sun = SunLight { dir: vec3f(0.55, 0.62, 0.56).normalize(), ..SunLight::default() };
        let (glb, sidecar) = fixture(&dir, 77, &sun);

        // Fresh, keyed, same sun: loads.
        let atlas = Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &sun)
            .expect("fresh keyed sidecar should load");
        assert_eq!(atlas.rows, 1);
        // No expected hash (the model case): also loads.
        assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, None, &sun).is_some());
        // Wrong hash: rejected.
        assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(78), &sun).is_none());
        // A MILDLY different sun (an authored time_of_day): loads — the
        // instance build stretches the window by the length ratio
        // (play-session-1 entry 18; exact matching starved the whole tier).
        let mild =
            SunLight { dir: vec3f(0.45, 0.70, 0.46).normalize(), ..SunLight::default() };
        assert!(
            Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &mild).is_some()
        );
        // A WILDLY different sun (near-noon vs the low bake): outside the
        // 0.2-5x stretch band — rejected; a smeared stretch is worse than
        // the blob.
        let other = SunLight { dir: vec3f(0.1, 0.95, 0.1).normalize(), ..SunLight::default() };
        assert!(
            Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &other).is_none()
        );
        // Stale mtime rejects only an UNKEYED load (checkout files). A
        // keyed load trusts the hash: the store cache writes both files at
        // arbitrary times, and write ordering must not cost the shadows.
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(&sidecar)
            .unwrap()
            .set_modified(older)
            .unwrap();
        assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, None, &sun).is_none());
        assert!(Renderer::load_shadow_sdf_sidecar(&sidecar, &glb, Some(77), &sun).is_some());
        // Missing either file: rejected, never a panic.
        assert!(Renderer::load_shadow_sdf_sidecar(
            &dir.join("absent.shadowsdf"),
            &glb,
            None,
            &sun
        )
        .is_none());
        assert!(Renderer::load_shadow_sdf_sidecar(
            &sidecar,
            &dir.join("absent.glb"),
            None,
            &sun
        )
        .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The rigid-part state machine: a door's whole behaviour, tested without a
/// device. The GPU side is one extra draw with the matrix these produce.
#[cfg(test)]
mod anim_part_tests {
    use super::*;
    use crate::model::tests::{rooms_and_door_glb, Door};

    /// The importer-shaped fixture's door: closed at t=0 (on the floor),
    /// open at t=1 (lifted 3), authored open.
    fn door() -> crate::model::AnimPart {
        let mut m = StaticModel::parse_glb(&rooms_and_door_glb(Door::Animated)).unwrap();
        assert_eq!(m.anim_parts.len(), 1);
        m.anim_parts.pop().unwrap()
    }

    fn lift(part: &crate::model::AnimPart, time: f32) -> f32 {
        part.transform_at(time).v[13]
    }

    #[test]
    fn an_untriggered_part_sits_in_the_authored_default() {
        let part = door();
        let states = ModelStates::default();
        let (state, time, target) =
            states.clock(&ModelTarget::Model("map".into()), "map", &part);
        assert_eq!(state, 1, "extras.default = open");
        assert!((time - 1.0).abs() < 1.0e-6);
        assert!((target - 1.0).abs() < 1.0e-6);
        assert!((lift(&part, time) - 3.0).abs() < 1.0e-6);
    }

    #[test]
    fn closing_travels_the_clip_in_the_blend_time() {
        let part = door();
        let mut states = ModelStates::default();
        let key = ModelTarget::Model("map".into());
        assert!(states.set(key.clone(), &part, "closed", 1.0));

        // Half the blend, half the travel — linear in time, by contract.
        states.tick(0.5);
        let (_, time, _) = states.clock(&key, "map", &part);
        assert!((time - 0.5).abs() < 1.0e-5, "time {time}");
        assert!((lift(&part, time) - 1.5).abs() < 1.0e-4);

        // The rest of it, and then it stops dead rather than overshooting.
        states.tick(0.5);
        let (state, time, _) = states.clock(&key, "map", &part);
        assert_eq!(state, 0);
        assert!(time.abs() < 1.0e-6, "time {time}");
        states.tick(5.0);
        let (_, time, target) = states.clock(&key, "map", &part);
        assert!(time.abs() < 1.0e-6 && (time - target).abs() < 1.0e-6);
        assert!(lift(&part, time).abs() < 1.0e-6, "closed sits on the floor");
    }

    #[test]
    fn a_command_mid_move_reverses_from_where_the_part_is() {
        let part = door();
        let mut states = ModelStates::default();
        let key = ModelTarget::Model("map".into());
        states.set(key.clone(), &part, "closed", 1.0);
        states.tick(0.5);
        let (_, half, _) = states.clock(&key, "map", &part);

        // Re-aimed at open from exactly half way: no jump, and the reversal
        // takes its own blend rather than resuming the old one.
        states.set(key.clone(), &part, "open", 1.0);
        let (state, time, target) = states.clock(&key, "map", &part);
        assert_eq!(state, 1);
        assert!((time - half).abs() < 1.0e-6, "the door jumped: {time} vs {half}");
        assert!((target - 1.0).abs() < 1.0e-6);

        states.tick(0.5);
        let (_, time, _) = states.clock(&key, "map", &part);
        assert!((time - 0.75).abs() < 1.0e-5, "time {time}");
        states.tick(0.5);
        let (_, time, _) = states.clock(&key, "map", &part);
        assert!((time - 1.0).abs() < 1.0e-6, "time {time}");
        assert!((lift(&part, time) - 3.0).abs() < 1.0e-5);
    }

    #[test]
    fn a_zero_blend_snaps_and_an_unknown_state_changes_nothing() {
        let part = door();
        let mut states = ModelStates::default();
        let key = ModelTarget::Model("map".into());
        assert!(states.set(key.clone(), &part, "closed", 0.0));
        let (_, time, _) = states.clock(&key, "map", &part);
        assert!(time.abs() < 1.0e-6, "zero blend is instant");

        assert!(!states.set(key.clone(), &part, "ajar", 1.0), "no such state");
        let (state, time, _) = states.clock(&key, "map", &part);
        assert_eq!(state, 0);
        assert!(time.abs() < 1.0e-6);
    }

    /// The collision half: a closed door is a wall in the doorway, an open
    /// one is not. Same boxes, moved by the same matrix the draw uses.
    #[test]
    fn collider_boxes_move_with_the_part() {
        let part = door();
        let local = part.collider_boxes();
        assert!(!local.is_empty());
        let mut instance = Mat4f::identity();
        instance.v[13] = 10.0; // the level itself is placed 10 up

        let world_at = |time: f32| {
            let m = Mat4f::mul(&instance, &part.transform_at(time));
            world_boxes(&m, &local)
        };
        let (closed, cmin, cmax) = world_at(part.state_time(0));
        let (open, omin, omax) = world_at(part.state_time(1));
        assert_eq!(closed.len(), local.len());
        // Closed: the slab fills the opening at the level's own height.
        assert!((cmin.y - 10.0).abs() < 1.0e-4, "{cmin:?}");
        assert!((cmax.y - 13.0).abs() < 1.0e-4, "{cmax:?}");
        // Open: the same slab has risen out of the way by the clip's travel.
        assert!((omin.y - 13.0).abs() < 1.0e-4, "{omin:?}");
        assert!((omax.y - 16.0).abs() < 1.0e-4, "{omax:?}");
        assert!(open.iter().zip(&closed).all(|(o, c)| {
            (o.0.y - c.0.y - 3.0).abs() < 1.0e-3 && (o.0.x - c.0.x).abs() < 1.0e-4
        }));
        // Half way is half way here too — a door caught moving is where it
        // looks, not snapped to an end state.
        let (_, hmin, _) = world_at(0.5);
        assert!((hmin.y - 11.5).abs() < 1.0e-3, "{hmin:?}");
    }

    #[test]
    fn a_slot_command_wins_over_a_model_command() {
        let part = door();
        let mut states = ModelStates::default();
        states.set(ModelTarget::Model("map".into()), &part, "closed", 0.0);
        states.set(ModelTarget::Instance(2), &part, "open", 0.0);

        // Slot 2 asked for open; every other copy follows the model command.
        let (state, _, _) = states.clock(&ModelTarget::Instance(2), "map", &part);
        assert_eq!(state, 1);
        let (state, _, _) = states.clock(&ModelTarget::Instance(3), "map", &part);
        assert_eq!(state, 0, "an unaddressed slot follows the model command");
    }

    /// Slot numbers only mean anything against one placed list.
    #[test]
    fn a_changed_placed_scene_drops_slot_commands_but_keeps_model_ones() {
        let part = door();
        let mut renderer = Renderer::default();
        renderer.model_anim_state.set(ModelTarget::Model("map".into()), &part, "closed", 0.0);
        renderer.model_anim_state.set(ModelTarget::Instance(0), &part, "closed", 0.0);

        let mut transform = Mat4f::identity();
        transform.v[12] = 7.0;
        renderer.set_models(vec![ModelInstance {
            model: "map".to_string(),
            transform,
            dynamic: false,
            depth_order: 0.0,
            part_poses: Vec::new(),
        }]);
        assert!(renderer
            .model_anim_state
            .map
            .keys()
            .all(|(t, _)| matches!(t, ModelTarget::Model(_))));
        assert_eq!(renderer.model_anim_state.map.len(), 1);
    }

    #[test]
    fn model_targets_come_from_ids_and_slots() {
        assert_eq!(
            ModelTarget::from("maps/e1m1"),
            ModelTarget::Model("maps/e1m1".to_string())
        );
        assert_eq!(ModelTarget::from(4usize), ModelTarget::Instance(4));
    }

    /// An ordinary prop has no parts, and every query says so without
    /// pretending the model is missing.
    #[test]
    fn a_model_without_parts_answers_empty() {
        let renderer = Renderer::default();
        assert!(renderer.model_anim_part_names("kit/lamp").is_empty());
        assert!(renderer.model_anim_part("kit/lamp", "door_1").is_none());
        assert!(renderer.anim_part_boxes().is_empty());
        assert!(renderer.model_states("kit/lamp").is_empty());
    }
}

/// Which statics may cast into the baked sun shadows. A prop casts onto the
/// world; a whole imported level IS the world, and casting it shadows its own
/// rooms.
#[cfg(test)]
mod caster_only_tests {
    use super::*;

    fn bounds(span: f32) -> (Vec3f, Vec3f) {
        (vec3f(0.0, 0.0, 0.0), vec3f(span, span * 0.5, span))
    }

    #[test]
    fn a_prop_casts_and_a_level_does_not() {
        // A fence, a shed, a kit building: all well under the span.
        for span in [1.0, 4.0, 20.0, 39.9] {
            let (lo, hi) = bounds(span);
            assert!(
                casts_as_caster_only(None, false, lo, hi),
                "a {span} m prop must still cast"
            );
        }
        // A Doom/Duke map arrives as one static hundreds of metres across.
        for span in [40.1, 200.0, 4000.0] {
            let (lo, hi) = bounds(span);
            assert!(
                !casts_as_caster_only(None, false, lo, hi),
                "a {span} m level must not shadow its own interior"
            );
        }
    }

    #[test]
    fn a_prelit_model_never_casts() {
        let (lo, hi) = bounds(3.0);
        assert!(!casts_as_caster_only(None, true, lo, hi));
    }

    /// The host's answer beats the heuristic in both directions.
    #[test]
    fn an_explicit_answer_wins() {
        let (big_lo, big_hi) = bounds(500.0);
        let (small_lo, small_hi) = bounds(2.0);
        assert!(casts_as_caster_only(Some(true), false, big_lo, big_hi));
        assert!(casts_as_caster_only(Some(true), true, big_lo, big_hi));
        assert!(!casts_as_caster_only(Some(false), false, small_lo, small_hi));
    }

    /// Setting it re-kicks the bake, and setting the same answer twice does
    /// not — the bake is expensive and the signature is what gates it.
    #[test]
    fn changing_the_answer_re_kicks_the_bake() {
        let mut renderer = Renderer::default();
        renderer.placed_scene_signature = Some(7);
        let rev = renderer.models_rev;
        renderer.set_model_casts_shadow("maps/e1m1", false);
        assert_eq!(renderer.placed_scene_signature, None);
        assert_eq!(renderer.models_rev, rev.wrapping_add(1));

        renderer.placed_scene_signature = Some(9);
        renderer.set_model_casts_shadow("maps/e1m1", false);
        assert_eq!(renderer.placed_scene_signature, Some(9), "no needless re-kick");
        assert_eq!(renderer.models_rev, rev.wrapping_add(1));
    }
}

/// The map-sky lane's device-free half: the clock the scrolling layers ride
/// and the queries a host asks before it has loaded anything.
#[cfg(test)]
mod sky_lane_tests {
    use super::*;
    use crate::model::tests::room_and_sky_glb;

    #[test]
    /// Which sky gets a mip chain, and why. A cylinder strip must NOT: it is
    /// magnified in every view that exists, and the chain is what turns
    /// `atan2`'s branch cut into a one-pixel line down the sky (the hairline
    /// on Doom maps). The other two keep theirs — Quake's swirl and the
    /// equirect poles are real minification.
    fn only_the_cylinder_sky_ships_without_mips() {
        use crate::model::SkyProjection;
        assert!(!sky_wants_mips(SkyProjection::Cylinder), "the seam lives here");
        assert!(sky_wants_mips(SkyProjection::QuakeScroll));
        assert!(sky_wants_mips(SkyProjection::Cube));
    }

    #[test]
    /// The cut itself, in the CPU twin of the shader's mapping: two headings
    /// a hair apart across it land a whole texture period apart in u — the
    /// jump the hardware would have read as "minified to nothing" — while
    /// the colour they name is the same texel. Nothing here can fix that;
    /// only having no mip levels can.
    fn the_longitude_cut_jumps_by_whole_periods() {
        use crate::model::{SkyPart, SkyProjection};
        let part = SkyPart {
            projection: SkyProjection::Cylinder,
            repeat: 4.0,
            speeds: vec![0.0],
            offset: 0.0,
            texture: None,
            v_span: 0.5,
            vertices: Vec::new(),
            indices: Vec::new(),
            min: vec3f(0.0, 0.0, 0.0),
            max: vec3f(0.0, 0.0, 0.0),
            images: Vec::new(),
        };
        let eps = 1.0e-4;
        let left = part.direction_uv(vec3f(eps, 0.0, -1.0), 0, 0.0);
        let right = part.direction_uv(vec3f(-eps, 0.0, -1.0), 0, 0.0);
        let jump = (left[0] - right[0]).abs();
        assert!(
            (jump - part.repeat).abs() < 1.0e-3,
            "one turn of the compass = `repeat` periods of u, got {jump}"
        );
        // Whole periods: the two sides name the same texel, so the picture
        // is continuous even though the coordinate is not.
        assert!((jump - jump.round()).abs() < 1.0e-3);
        // Away from the cut the mapping is smooth — a degree of yaw moves u
        // by a degree's worth, not by a period.
        let a = part.direction_uv(vec3f(0.0, 0.0, 1.0), 0, 0.0);
        let b = part.direction_uv(vec3f(0.017, 0.0, 1.0), 0, 0.0);
        assert!((a[0] - b[0]).abs() < 0.02, "smooth away from the cut");
    }

    #[test]
    fn the_sky_clock_advances_and_can_be_pinned() {
        let mut renderer = Renderer::default();
        assert_eq!(renderer.sky_time(), 0.0);
        renderer.tick_sky(0.25);
        renderer.tick_sky(0.25);
        assert!((renderer.sky_time() - 0.5).abs() < 1.0e-6);

        // A capture pins the clock so the same frame renders the same sky.
        renderer.set_sky_time(3.0);
        assert_eq!(renderer.sky_time(), 3.0);

        // Long sessions wrap rather than losing precision in the offset.
        renderer.set_sky_time(4095.5);
        renderer.tick_sky(1.0);
        assert!(renderer.sky_time() < 1.0, "{}", renderer.sky_time());

        // Garbage dt is ignored rather than poisoning the clock.
        renderer.set_sky_time(2.0);
        renderer.tick_sky(f32::NAN);
        assert_eq!(renderer.sky_time(), 2.0);
    }

    #[test]
    fn a_model_without_a_sky_answers_none() {
        let renderer = Renderer::default();
        assert!(renderer.model_sky("maps/e1m1").is_none());
        assert!(renderer.model_sky_mesh("maps/e1m1").is_none());
    }

    /// What the shader is handed for a Doom sky, computed the way
    /// `draw_sky_faces` computes it — the parse and the draw agreeing on
    /// projection code, repeat and scroll is the contract the GPU cannot
    /// check for us.
    #[test]
    fn the_shader_parameters_follow_the_parsed_sky() {
        let m = StaticModel::parse_glb(&room_and_sky_glb(Some("quake_scroll"), 2)).unwrap();
        let sky = m.sky.as_ref().unwrap();
        let time = 1.5;
        let sky_p = vec4(
            sky.projection.code(),
            sky.repeat,
            sky.scroll(0, time),
            sky.scroll(1, time),
        );
        // 2.0 is the branch the shader's `sky_p.x < 2.5` arm takes.
        assert_eq!(sky_p.x, 2.0);
        assert_eq!(sky_p.y, 4.0);
        // The map's static phase (0.25) plus 1.5 s at 8 and 16 units.
        assert_eq!(sky_p.z, 0.25 + 12.0);
        assert_eq!(sky_p.w, 0.25 + 24.0);
        let sky_q = vec4(sky.v_span, 1.0, 0.0, 0.0);
        assert_eq!(sky_q.x, crate::model::SKY_DEFAULT_V_SPAN);
    }
}
