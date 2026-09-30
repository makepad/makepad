//! The game draw shaders, moved verbatim from gamemaker's game_view.rs.
//! The composite into the host pane (DrawSceneTexture) is makepad-render-graph's; the
//! cube/alpha/sky/terrain family renders the world itself.

use makepad_draw::*;

mod mixins;
mod csm;
mod cube;
mod effects;
mod sky_dome;
mod skinned;
mod pbr;
mod city;
mod grass;
mod foliage;
mod skinned_gpu;
mod world;
mod lm_depth;
mod lm_gather;
mod lm_encode;

/// Register every scene shader block, in the order the single block used to
/// declare them (later blocks spread earlier `mod.draw` objects).
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    mixins::script_mod(vm);
    csm::script_mod(vm);
    cube::script_mod(vm);
    effects::script_mod(vm);
    sky_dome::script_mod(vm);
    skinned::script_mod(vm);
    pbr::script_mod(vm);
    city::script_mod(vm);
    grass::script_mod(vm);
    foliage::script_mod(vm);
    skinned_gpu::script_mod(vm);
    world::script_mod(vm);
    lm_depth::script_mod(vm);
    lm_gather::script_mod(vm);
    lm_encode::script_mod(vm)
}

/// The concatenated shader source, for the tests that pin shader text.
#[cfg(test)]
pub(crate) const SHADER_SOURCE: &str = concat!(
    include_str!("shaders/mixins.rs"),
    include_str!("shaders/csm.rs"),
    include_str!("shaders/cube.rs"),
    include_str!("shaders/effects.rs"),
    include_str!("shaders/sky_dome.rs"),
    include_str!("shaders/skinned.rs"),
    include_str!("shaders/pbr.rs"),
    include_str!("shaders/skinned_gpu.rs"),
    include_str!("shaders/world.rs"),
    include_str!("shaders/lm_depth.rs"),
    include_str!("shaders/lm_gather.rs"),
    include_str!("shaders/lm_encode.rs"),
);

/// DrawCube + per-instance emission (`glow`) and per-instance fog density.
///
/// Instance-field rule: only #[live] instance fields after the deref chain —
/// `DrawVars::as_slice` reads them contiguously. The sun terms and fog colour
/// are deliberately NOT here: they are shader uniforms (see the script block
/// above) set once per frame through [`crate::sun::SunLight::write_uniforms`],
/// which keeps 48 bytes of identical data out of every instance.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCube {
    #[deref]
    pub cube: DrawCube,
    #[live(0.0)]
    pub glow: f32,
    #[live(0.0)]
    pub fog_density: f32,
    /// x = hue degrees, y = saturation, z = value, w reserved.
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub color_adjust_ctl: Vec4f,
}

/// Alpha-blended variant: water, sensor ghosts, blob shadows.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneAlpha {
    #[deref]
    pub cube: DrawSceneCube,
}

/// One firework shell. Every field is an instance: the GPU derives all
/// `SPARKS_PER_SHELL` sparks from these numbers alone.
///
/// Packed into `vec4`s ON PURPOSE. A `Vec3f` instance is tightly packed on the
/// Rust side but a `vec3` obeys 16-byte alignment in the shader ABI, so the
/// three floats are read back misaligned and every field after them shifts —
/// which presents as the whole burst rendering at the world origin, on the
/// ground, instead of where it was placed. The other shaders here get away
/// with `Vec3f` because theirs are declared `uniform`, not instance.
///
/// Four floats at a time is the shape the hardware wants anyway; the packing
/// costs nothing and removes the class of bug entirely.
///
/// # Why this derefs DrawVars and not DrawCube
///
/// It used to deref `DrawCube`, and every instance field read back garbage —
/// the burst rendered at the world origin and the spark size ignored whatever
/// Rust wrote. Encoding the instance values as colour on a fixed clip-space
/// quad settled it: the decoded numbers CHANGED WHEN THE CAMERA ROTATED.
/// Instance data cannot depend on the view, so the shader was reading view
/// memory — the fields were bound at the wrong offsets, not merely wrong.
///
/// Inheriting `DrawCube` brings its own instance fields along, and appending
/// more after them puts these at offsets the script-side layout does not
/// account for. `DrawSceneShadow` — the one shader here that instances
/// correctly — derefs `DrawVars` and declares its uniform buffers, vertex
/// buffer and varyings explicitly, so its instance fields are the only ones
/// and the layout is unambiguous. This now follows that pattern.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFirework {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = burst point, w = seconds since the burst (negative = climbing).
    #[live(vec4(0.0, 30.0, 0.0, 0.0))]
    pub origin_age: Vec4f,
    /// xyz = launch point, w = spark lifetime.
    #[live(vec4(0.0, 0.0, 0.0, 2.0))]
    pub launch_life: Vec4f,
    /// x = spark speed, y = seed, z = spark size, w unused.
    #[live(vec4(12.0, 0.0, 0.6, 0.0))]
    pub params: Vec4f,
    #[live(vec4(1.0, 0.8, 0.4, 1.0))]
    pub color: Vec4f,
    #[live(vec4(1.0, 0.3, 0.1, 1.0))]
    pub color_tail: Vec4f,
}

/// Sky dome, authored-gradient mode: scripts that set their own sky
/// colours keep this four-colour dome. Default-sky worlds draw
/// [`DrawSceneSkyAnalytic`] instead (the split is deliberate — one combined
/// pixel fn sat exactly at a script-shader capacity limit).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSky {
    #[deref]
    pub cube: DrawCube,
    #[live(vec3(0.32, 0.58, 0.9))]
    pub sky_top: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub sky_horizon: Vec3f,
    #[live(vec3(0.68, 0.75, 0.66))]
    pub sky_ground: Vec3f,
    #[live(vec3(0.3, 0.4, 0.3))]
    pub sky_bottom: Vec3f,
}

/// Sky dome, analytic mode: the Preetham daylight model whose
/// sun-dependent halves arrive from [`crate::sky`], plus the setting sun
/// disc and the rotating night star dome. Drawn for worlds that keep the
/// DEFAULT sky colours; authored gradients use [`DrawSceneSky`].
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkyAnalytic {
    #[deref]
    pub cube: DrawCube,
    /// Perez A,B,C,D per channel (E rides in `pz_e`).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_y: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_x: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_yc: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_e: Vec4f,
    /// 1 / F(0, theta_s) per channel.
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub pz_f0: Vec4f,
    /// Zenith Yxy + night blend in w.
    #[live(vec4(1.0, 0.31, 0.32, 0.0))]
    pub zenith: Vec4f,
    /// MODEL sun direction (clamped ~2 deg for Perez) + exposure in w.
    #[live(vec4(0.0, 1.0, 0.0, 0.12))]
    pub sun_e: Vec4f,
    /// TRUE sun direction, unclamped — the disc/Mie/afterglow ride it
    /// below the horizon.
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_true: Vec4f,
    /// Celestial rotation rows (world dir -> star map dir); row 0's w is
    /// the star gain (0 = no map bound).
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub star_r0: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub star_r1: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub star_r2: Vec4f,
    /// Additive shared-model lanes. The legacy fields above remain part of
    /// this public draw type for existing hosts.
    #[live(vec4(0.0, 1.0, 0.0, 0.025))]
    pub sun_model: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.999989))]
    pub sun_dir: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub sun_radiance: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 1.0))]
    pub world_up: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub star_east: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub blend: Vec4f,
    #[live(1.0)]
    pub exposure: f32,
}

/// A map's own sky surfaces, shaded by view direction
/// ([`crate::model::SkyPart`]). Packed model vertex stream, no lighting.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkyMap {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    /// TRUE world camera position (w unused). The ray each fragment samples
    /// by is `world_position - eye`, so this must be the physical camera,
    /// not a stage-transformed one.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub eye: Vec4f,
    /// x = projection code ([`crate::model::SkyProjection::code`]),
    /// y = horizontal repeats per 360 degrees, z = layer-0 scroll offset,
    /// w = layer-1 scroll offset (both in texture units).
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub sky_p: Vec4f,
    /// x = vertical span as a fraction of a half turn (Doom's sky stretch),
    /// y = second-layer alpha gain, zw spare.
    #[live(vec4(0.5, 1.0, 0.0, 0.0))]
    pub sky_q: Vec4f,
    /// Flat multiplier on the sampled colour. 1.0 is the image as authored,
    /// which is what "unlit, full brightness" means.
    #[live(1.0)]
    pub brightness: f32,
}

/// Skinned character mesh (PbrVertex layout, uv in ny_nz_uv.zw, textured).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkinned {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub fur: Vec4f,
    // Keep this base's instance payload a multiple of eight bytes so
    // derived material fields follow it without Rust tail padding.
    #[live(vec2(0.0,0.0))] pub fur_layer: Vec2f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_ctl:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights0:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights1:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights2:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights3:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights4:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights5:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights6:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights7:Vec4f,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    /// 1.0 = show baked AO alone, contrast-stretched (the host's AO debug setting).
    #[live(0.0)]
    pub ao_debug: f32,
    /// 1.0 when this pack has a baked AO atlas bound.
    #[live(0.0)]
    pub ao_enabled: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// Per-character wash over the vertex tint. One rig serves a whole village,
    /// so without this every passer-by is the same knight in the same colours —
    /// the identical-clones failure the prop variety work just fixed. Costs one
    /// vec4 on an instance stream that carries a handful of characters, and one
    /// multiply in the vertex stage.
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub tint: Vec4f,
    /// x = hue degrees, y = saturation, z = value, w reserved.
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub color_adjust_ctl: Vec4f,
    /// This instance's window into the baked light atlas (offset uv, scale
    /// uv). Zero scale = no lightmap: full analytic sun, no lamp light —
    /// dynamics and unbaked models render exactly as before.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub lm_rect: Vec4f,
    /// 1.0 = show the baked light alone (`HostSettings::lm_debug`).
    #[live(0.0)]
    pub lm_debug: f32,
    /// Dynamic-light gate: 1.0 for dynamic instances (sum every light slot),
    /// 0.0 for statics (sum only the transient prefix — their lamp light is
    /// already baked into the atlas, and statics and dynamics of one model
    /// share a draw item, so this must ride the instance stream).
    #[live(0.0)]
    pub dl_apply: f32,
    /// Ground height under a DYNAMIC instance: the baked-shadow sample is
    /// projected along the sun ray down to this plane. Unused by statics.
    #[live(0.0)]
    pub ground_y: f32,
    /// Depth-tie breaker for coplanar stacked statics. The vertex stage
    /// scales the view-space position by (1 - depth_bias), which the
    /// perspective divide cancels — the on-screen image is EXACTLY the
    /// flush geometry, only the depth-buffer value moves toward the
    /// camera. Placement order feeds it (renderer: depth_order * 1e-3),
    /// so a prop resting on a floor plate wins the z-tie against it
    /// deterministically instead of being physically lifted off it.
    #[live(0.0)]
    pub depth_bias: f32,
    /// Q3 / Unreal detail UV scale. Zero disables the overlay.
    #[live(vec2(0.0, 0.0))]
    pub detail_st: Vec2f,
    /// 1 = vertex COLOR_0 is baked lighting (do not multiply the sun).
    #[live(0.0)]
    pub prelit: f32,
    /// x = 1: the base-colour texture's glTF sampler asks for NEAREST
    /// magnification, so a texel larger than a pixel draws as a hard square
    /// (the classic games' look); minified texels stay filtered. y unused
    /// (keeps the payload a multiple of eight bytes). Set per draw from the
    /// layer's material; 0 everywhere else draws exactly as before.
    /// y = the PBR lane's shading terms (`makepadShading`), packed as
    /// rim * 255 * 65536 + clearcoat * 255 * 256 + flake * 255 (0 = plain;
    /// `MaterialSurface::packed_shading`). Packed into this
    /// spare lane on purpose: the model lanes' instance stream is at the
    /// vertex-attribute limit (31 on Metal, 32 on common Vulkan GPUs; one
    /// more vec4 lost the Vulkan device in race).
    #[live(vec2(0.0, 0.0))]
    pub tex_mag: Vec2f,
}

/// [`DrawSceneSkinned`] plus a Cook-Torrance specular lobe, for static props
/// whose glTF material carries real shininess
/// ([`crate::model::PbrMaterial::is_shiny`]).
///
/// The deref chain is what makes the two lanes one code path: everything the
/// renderer sets on a model draw — transform, lightmap window, dynamic-light
/// gate, sun terms — is set through the inherited [`DrawSceneSkinned`], and
/// only the three material lanes below are new. Instance-field rule as
/// everywhere in this file: `#[live]` instance floats AFTER the deref only,
/// so `DrawVars::as_slice` reads the base's lanes and then these.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScenePbr {
    #[deref]
    pub skinned: DrawSceneSkinned,
    /// glTF `metallicFactor`, multiplied by the ORM map's B channel.
    #[live(0.0)]
    pub metallic: f32,
    /// glTF `roughnessFactor`, multiplied by the ORM map's G channel.
    #[live(1.0)]
    pub roughness: f32,
    /// 1.0 when a metallicRoughness texture is bound on slot 6. Zero folds
    /// the sample out of both products, so a factors-only material costs one
    /// 1x1 fetch and nothing else.
    #[live(0.0)]
    pub orm_on: f32,
    #[live(0.0)] pub surface_on:f32,
    #[live(1.0)] pub material_alpha:f32,
    #[live(0.0)] pub alpha_mode:f32,
    #[live(0.5)] pub alpha_cutoff:f32,
    #[live(0.0)] pub normal_scale:f32,
    #[live(0.0)] pub occlusion_strength:f32,
    #[live(vec3(0.0,0.0,0.0))] pub emissive:Vec3f,
    #[live(0.0)] pub double_sided:f32,
    /// Triplanar UV scale (1/metres), 0 = mesh UVs (MaterialSurface).
    #[live(0.0)] pub triplanar:f32,
}

// DrawVars reads inherited instance fields as one contiguous float slice.
// Tail padding in the base shifts every PBR field (AO becomes red emission).
const _: () = {
    let end = std::mem::offset_of!(DrawSceneSkinned, tex_mag) + std::mem::size_of::<Vec2f>();
    assert!(std::mem::size_of::<DrawSceneSkinned>() == end);
    assert!(std::mem::offset_of!(DrawScenePbr, metallic) == end);
};

/// The streamed world's surface shader (`renderer/stream_draw.rs`):
/// [`DrawScenePbr`]'s lighting with the city's materials — curtain-wall and
/// window glass with a sky-and-skyline reflection and a room behind each
/// pane, clear-coated paint, emissive lamps, and the lit-window hours. A
/// layer's `metallic` slot carries its [`crate::stream::StreamSurface`]
/// code; the ORM map's channels are read per that kind.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCity {
    #[deref]
    pub pbr: DrawScenePbr,
    /// x = night factor (0 day .. 1 night: window hours, lamp emission).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub city: Vec4f,
}

/// GPU grass blades (`crate::grass`): DrawScenePbr's lane with its own
/// vertex stage (blades placed on the grass field) and pixel. One instance
/// per patch. It adds NO instance lanes (the PBR stream is at the vertex-
/// attribute limit) and no uniforms (the PBR block is full): the blades
/// never morph, so the field rides the morph weight lanes, see
/// `renderer/grass_draw.rs` for the layout.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneGrass {
    #[deref]
    pub pbr: DrawScenePbr,
}

/// The swaying-foliage lane (`shaders/foliage.rs`): DrawScenePbr's vertex
/// stage and binding with a pixel made for leaves. No instance lanes of its
/// own (the PBR stream is at the vertex-attribute limit).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFoliageLit {
    #[deref]
    pub pbr: DrawScenePbr,
}

/// Camera-space held-model shader: per-pixel PBR (normal map, metallic-
/// roughness, sun lobe, sky reflection) without any world-lighting state.
/// The transform and the layer material are the instance lanes; daylight is
/// uniform per view.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneViewModel {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// 1 in the HDR lane: texels and vertex colours are sRGB-encoded and
    /// must be linearised like every world shader does (`to_scene`), or
    /// the held model renders washed out next to the world.
    #[live(0.0)]
    pub lin: f32,
    /// Sun reaching the camera (0 in shadow): the held model has no shadow
    /// map, so the host tells it whether the eye stands in shade.
    #[live(1.0)]
    pub sun_vis: f32,
    /// The horizon radiance the sky reflection fades to (the world's fog).
    #[live(vec3(0.6, 0.65, 0.7))]
    pub fog_color: Vec3f,
    /// Per layer: the glTF metallic/roughness factors (x the ORM map when
    /// `orm_on`), normal-map strength (0 = none), mask cutout, emission.
    #[live(0.0)]
    pub metallic: f32,
    #[live(1.0)]
    pub roughness: f32,
    #[live(0.0)]
    pub orm_on: f32,
    #[live(0.0)]
    pub normal_scale: f32,
    #[live(0.0)]
    pub alpha_mode: f32,
    #[live(0.5)]
    pub alpha_cutoff: f32,
    #[live(vec3(0.0, 0.0, 0.0))]
    pub emissive: Vec3f,
}

/// Old-school lamp lens flare: one additive billboard per visible light.
/// Instance fields are vec4-packed for the same shader-ABI reason as
/// [`DrawSceneFirework`] (a vec3 instance reads back misaligned).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFlare {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = world position of the glow, w = billboard size in world units.
    #[live(vec4(0.0, 0.0, 0.0, 1.0))]
    pub flare_pos: Vec4f,
    /// rgb = glow colour, w = intensity multiplier.
    #[live(vec4(1.0, 0.9, 0.6, 1.0))]
    pub flare_col: Vec4f,
}

/// Surface-aligned procedural bullet-hole plane. Both instance payloads are
/// vec4-packed to keep the shader ABI aligned.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneDecal {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = lifted world centre, w = full diameter.
    #[live(vec4(0.0, 0.0, 0.0, 0.08))]
    pub decal_pos: Vec4f,
    /// xyz = surface normal, w = in-plane rotation.
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub decal_normal: Vec4f,
    /// rgb = energy tint, w = procedural family (bullet/scorch/energy).
    #[live(vec4(0.105, 0.075, 0.045, 0.0))]
    pub decal_color: Vec4f,
}

/// In-world video screen: one upright textured quad, texture updated per
/// frame by the host. Instance fields are vec4-packed for the same
/// shader-ABI reason as [`DrawSceneFlare`].
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneScreen {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = world position of the screen centre, w = yaw in radians
    /// (yaw == camera orbit yaw faces that camera squarely).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub screen_pos: Vec4f,
    /// x = width, y = height in world units; zw may carry source sheet size.
    /// A NEGATIVE `w` asks for a floor-aligned card and a NEGATIVE `z` for a
    /// ground-anchored, depth-flattened billboard (see the vertex shader).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub screen_size: Vec4f,
    /// 1 = alpha-cut-out sprite artwork (transparent pixels discard), 0 = an
    /// opaque screen. Sprite billboards are cut-outs by nature; a video
    /// frame is not, and must never depend on what its decoder wrote into
    /// the alpha byte.
    #[live(0.0)]
    pub cutout: f32,
    /// 1 = exact level-0 nearest sampling for pixel-art sheets, 0 = ordinary
    /// smooth sampling for the in-world video lane sharing this shader.
    #[live(0.0)]
    pub pixelated: f32,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub tint: Vec4f,
    /// x = hue degrees, y = saturation, z = value, w reserved.
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub color_adjust_ctl: Vec4f,
    /// Sub-rectangle of the texture this quad shows, `(u0, v0, u1, v1)`.
    /// A packed sprite sheet is ONE texture with many windows into it, which
    /// is what keeps a level's actor set at a dozen textures instead of five
    /// hundred. `u0 > u1` draws the X-mirrored pair the source stores once.
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub uv_rect: Vec4f,
}

/// GPU-skinned character mesh: rest geometry + joint-palette texture.
///
/// A sibling of [`DrawSceneSkinned`] (the DrawSceneFoliage pattern): props keep
/// the cheap fetch-free path, characters opt in to the palette blend. Field
/// order mirrors DrawSceneSkinned — every `#[live]` after the deref is an
/// instance field read contiguously by `DrawVars::as_slice`, and characters
/// of one rig batch into a single draw item, so the whole crowd's state
/// rides this stream. `joint_base` is the instance's first texel in the
/// frame's palette texture.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkinnedGpu {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub fur: Vec4f,
    // Keep this base's instance payload a multiple of eight bytes so
    // derived material fields follow it without Rust tail padding.
    #[live(vec2(0.0,0.0))] pub fur_layer: Vec2f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_ctl:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights0:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights1:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights2:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights3:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights4:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights5:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights6:Vec4f,
    #[live(vec4(0.0,0.0,0.0,0.0))] pub morph_weights7:Vec4f,
    #[live(0.0)] pub surface_on:f32,
    #[live(1.0)] pub material_alpha:f32,
    #[live(0.0)] pub alpha_mode:f32,
    #[live(0.5)] pub alpha_cutoff:f32,
    #[live(0.0)] pub normal_scale:f32,
    #[live(0.0)] pub occlusion_strength:f32,
    #[live(vec3(0.0,0.0,0.0))] pub emissive:Vec3f,
    #[live(vec3(0.0,0.0,0.0))] pub eye:Vec3f,
    #[live(0.0)] pub double_sided:f32,
    #[live(0.0)] pub metallic:f32,
    #[live(1.0)] pub roughness:f32,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// Per-character wash over the atlas colours (see DrawSceneSkinned::tint).
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub tint: Vec4f,
    /// x = hue degrees, y = saturation, z = value, w reserved.
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub color_adjust_ctl: Vec4f,
    /// First texel of this character's palette in the joint texture.
    #[live(0.0)]
    pub joint_base: f32,
    /// Ground height under this character: the baked-shadow sample is
    /// projected along the sun ray down to this plane (OnChange only — the
    /// Realtime cascades need no ground plane).
    #[live(0.0)]
    pub ground_y: f32,
}

/// Generated foliage: vertex-coloured mesh with growth reveal and wind sway.
///
/// A sibling of [`DrawSceneSkinned`] rather than a mode inside it — the shared
/// shaders draw most of the world and must not carry wind ALU they never use.
/// Both animation weights ride in the packed vertex's existing alpha lane, so
/// opting in costs vertex instructions but zero extra vertex bytes.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFoliage {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// Reveal threshold in [0, 1]. 1 = fully grown; defaults so a plant that
    /// nobody animates is simply present.
    #[live(1.0)]
    pub growth: f32,
    /// Width of the smoothstep band that hides growth_t's 16-level
    /// quantisation and makes tips unfurl instead of popping.
    #[live(0.12)]
    pub growth_band: f32,
    #[live(vec3(1.0, 0.0, 0.0))]
    pub wind_dir: Vec3f,
    #[live(0.0)]
    pub wind_strength: f32,
    #[live(0.0)]
    pub wind_gust: f32,
    #[live(0.0)]
    pub wind_time: f32,
}

/// Silhouette shadow mesh: all casters' hulls in one geometry, one draw call.
/// Position and per-vertex alpha only — no lighting, no fog (a shadow lies on
/// ground that is already fogged; fogging it again mixes it toward the bright
/// horizon and a distant shadow comes out lighter than what it darkens).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneShadow {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// Global dimmer, so a device can soften shadows without a rebuild.
    #[live(1.0)]
    pub shadow_scale: f32,
    /// Debug overlay: magenta at boosted alpha (the host's shadow debug setting).
    #[live(0.0)]
    pub shadow_debug: f32,
}

/// SDF silhouette shadow — the dynamic shadow quad every character and
/// driven car draws, shaded in the pixel stage from the caster's baked
/// silhouette-SDF atlas (shadow_sdf.rs,
/// [`crate::shadow_sdf::SDF_CELL`]-square cells — the shader's cell math
/// hardcodes 32 to match). Instance fields are vec4-packed for the
/// shader-ABI reason documented on [`DrawSceneFirework`], and the layout is
/// asserted below.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneShadowSdf {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// Global dimmer, mirroring [`DrawSceneShadow::shadow_scale`].
    #[live(1.0)]
    pub shadow_scale: f32,
    /// Magenta debug overlay (the host's shadow debug setting).
    #[live(0.0)]
    pub shadow_debug: f32,
    /// xyz = quad anchor (y at the receiver), w = receiver lift.
    #[live(vec4(0.0, 0.0, 0.0, 0.012))]
    pub sdf_a: Vec4f,
    /// xy = the owning light's horizontal direction (unit, toward the
    /// light — the sprite's local +x axis in world), z = sprite scale
    /// (footprint x anchor compression), w = final alpha.
    #[live(vec4(1.0, 0.0, 1.0, 0.3))]
    pub sdf_b: Vec4f,
    /// x = relative yaw (character yaw - light azimuth, radians), y = gait
    /// phase 0..1, z = idle-to-walk blend, w = atlas pose rows.
    #[live(vec4(0.0, 0.0, 0.0, 1.0))]
    pub sdf_c: Vec4f,
    /// The atlas window in sprite units: xy = (min_along, min_across),
    /// zw = (size_along, size_across) — every cell shares it.
    #[live(vec4(-1.0, -1.0, 2.0, 2.0))]
    pub sdf_d: Vec4f,
    /// x = base edge half-width in encoded-d units, y = widening per sprite
    /// unit toward the shadow tip (contact hardening), zw unused.
    #[live(vec4(0.06, 0.05, 0.0, 0.0))]
    pub sdf_e: Vec4f,
}

/// The firework ABI lesson, enforced at compile time: everything after
/// `DrawVars` must be exactly the instance lanes the script shader reads —
/// 3 floats + 5 vec4s, contiguous. Any accidental field among them (a bool,
/// an Option, a #[rust]) shifts these offsets and corrupts the GPU instance
/// stream, which presents as shadows at garbage positions. (The struct's
/// total size is NOT asserted: DrawVars aligns to 8, so tail padding could
/// hide a stray trailing f32 — the offsets can't.)
const _: () = {
    let base = std::mem::size_of::<DrawVars>();
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, depth_clip) == base);
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, sdf_a) == base + 3 * 4);
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, sdf_c) == base + 3 * 4 + 2 * 16);
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, sdf_e) == base + 3 * 4 + 4 * 16);
};

/// The smooth terrain mesh (PbrVertex layout: per-vertex color).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneTerrain {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// The terrain's window into the baked light atlas; zero = none.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub lm_rect: Vec4f,
    /// World xz rect that window covers: (x0, z0, width, depth).
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub lm_world: Vec4f,
    /// 1.0 = show the baked light alone (`HostSettings::lm_debug`).
    #[live(0.0)]
    pub lm_debug: f32,
}

/// The water sheet (W1): a per-volume grid displaced in the vertex stage by
/// the sim's own wave sum. Wave coefficients ride as UNIFORMS (per draw
/// item, one per volume — a coefficient change starts a new item), so the
/// instance stream stays as small as the terrain's.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneWater {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}

// ---------------------------------------------------------------------------
// GPU lightmap baker draw structs (gpu_lightmap.rs). Instance-field rule
// applies throughout: ONLY #[live] instance lanes after the DrawVars deref,
// vec4/mat4-packed (the DrawSceneFirework ABI lesson).
// ---------------------------------------------------------------------------

/// Sun-view depth pass: geometry from a region's fitted ortho sun camera.
/// `sun_r*` are the camera rows: dot(row.xyz, world) + row.w = ndc x/y/z01.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunDepth {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    /// x = 1: flipped depth test (far map for the shadow-top plane).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub flip_a: Vec4f,
    /// (sx, sy, ox, oy) clip-space tile mapping: the cascade pass renders
    /// CSM_CASCADES tiles of one strip through this same shader; (1,1,0,0)
    /// = the whole target (every atlas pass).
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub tile_a: Vec4f,
    #[live]
    pub morph_ctl: Vec4f,
    #[live]
    pub morph_weights0: Vec4f,
    #[live]
    pub morph_weights1: Vec4f,
    #[live]
    pub morph_weights2: Vec4f,
    #[live]
    pub morph_weights3: Vec4f,
    #[live]
    pub morph_weights4: Vec4f,
    #[live]
    pub morph_weights5: Vec4f,
    #[live]
    pub morph_weights6: Vec4f,
    #[live]
    pub morph_weights7: Vec4f,

}

/// [`DrawLmSunDepth`] for cut-out casters (leaf cards, fences): the same
/// projection, plus the layer's base-colour alpha tested per fragment, so a
/// card casts its leaves and not its rectangle. Only cut-out layers draw
/// through it; every other caster keeps the discard-free-by-texture lane.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunDepthCutout {
    #[deref]
    pub depth: DrawLmSunDepth,
}

/// [`DrawLmSunDepth`] for casters wholly inside their cascade's tile: the
/// same projection with no tile clip, so its pipeline has no `discard` and
/// keeps the GPU's hidden-surface removal.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunDepthInside {
    #[deref]
    pub depth: DrawLmSunDepth,
}

/// Lamp-view depth pass: six 90-degree faces tiled 3x2. `face_r*` are the
/// face view rows; `tile_a` = (sx, sy, ox, oy) clip-space tile mapping;
/// `lamp_range` = (near, far).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampDepth {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub face_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub face_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub face_rz: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub tile_a: Vec4f,
    #[live(vec4(0.25, 8.0, 0.0, 0.0))]
    pub lamp_range: Vec4f,
    #[live]
    pub morph_ctl: Vec4f,
    #[live]
    pub morph_weights0: Vec4f,
    #[live]
    pub morph_weights1: Vec4f,
    #[live]
    pub morph_weights2: Vec4f,
    #[live]
    pub morph_weights3: Vec4f,
    #[live]
    pub morph_weights4: Vec4f,
    #[live]
    pub morph_weights5: Vec4f,
    #[live]
    pub morph_weights6: Vec4f,
    #[live]
    pub morph_weights7: Vec4f,

}

/// [`DrawLmLampDepth`] with no face clip, for draws scissored to their
/// face's tile: its pipeline has no `discard`.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampDepthInside {
    #[deref]
    pub depth: DrawLmLampDepth,
}

/// Skinned sun-view depth pass (Realtime characters in the bake): the
/// DrawLmSunDepth projection over a rig's rest mesh, position-skinned
/// against the frame's joint-palette texture. `skin_a.x` = joint_base.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunDepthSkinned {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    /// x = 1: flipped depth test (far map for the shadow-top plane).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub flip_a: Vec4f,
    /// x = first palette texel of this caster (joint_base); yzw unused
    /// (vec4-packed per this block's instance-lane rule).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub skin_a: Vec4f,
    /// (sx, sy, ox, oy) clip-space tile mapping — see [`DrawLmSunDepth`].
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub tile_a: Vec4f,
    #[live]
    pub morph_ctl: Vec4f,
    #[live]
    pub morph_weights0: Vec4f,
    #[live]
    pub morph_weights1: Vec4f,
    #[live]
    pub morph_weights2: Vec4f,
    #[live]
    pub morph_weights3: Vec4f,
    #[live]
    pub morph_weights4: Vec4f,
    #[live]
    pub morph_weights5: Vec4f,
    #[live]
    pub morph_weights6: Vec4f,
    #[live]
    pub morph_weights7: Vec4f,

}

/// Sun gather over a mesh region's chart, at 4x. `target_a` maps chart uv
/// into the mask scratch; `sun_dir_p` = (sun dir, RAY_OFFSET); `params_a` =
/// (depth bias in z01 units, sun-up flag).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunGatherMesh {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub target_a: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.02))]
    pub sun_dir_p: Vec4f,
    #[live(vec4(0.001, 1.0, 0.0, 0.0))]
    pub params_a: Vec4f,
}

/// Sun gather over the ground heightfield. `quad_a` places the region quad
/// in the target; `ground_a` = world rect (x0, z0, sx, sz); `hf_a` =
/// (origin_x, origin_z, cell, n).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunGatherGround {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.02))]
    pub sun_dir_p: Vec4f,
    #[live(vec4(0.001, 1.0, 0.0, 0.0))]
    pub params_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub ground_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 2.0))]
    pub hf_a: Vec4f,
}

/// 3x3 despeckle vote. `src_a` = (inv_w, inv_h, area_w, area_h).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmDespeckle {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub src_a: Vec4f,
}

/// 4x -> 1x coverage downsample. `rect_a` = dest rect (x, y, w, h) texels.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmDownsample {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub src_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_a: Vec4f,
}

/// Chamfer distance-transform pass. `rect_px` = region rect in atlas
/// texels; `misc_a` = (1/atlas_size, mode).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmChamfer {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// Region-scoped hard zero of a persistent bake target. `quad_a` = the
/// region's padded rect in atlas uv, `fill_a` = the value every texel of it
/// becomes.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmZero {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub fill_a: Vec4f,
}

/// Lamp gather over a mesh region's chart at 1x. `lamp_a` = (pos, radius),
/// `lamp_b` = (color, spot), `lamp_c` = (dir, mode: 0 coverage / 1 lamp),
/// `lamp_d` = (near, far, bias, RAY_OFFSET).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampGatherMesh {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub target_a: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 8.0))]
    pub lamp_a: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub lamp_b: Vec4f,
    #[live(vec4(0.0, -1.0, 0.0, 1.0))]
    pub lamp_c: Vec4f,
    #[live(vec4(0.25, 8.0, 0.05, 0.02))]
    pub lamp_d: Vec4f,
}

/// Lamp gather over the ground heightfield at 1x.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampGatherGround {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub ground_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 2.0))]
    pub hf_a: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 8.0))]
    pub lamp_a: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub lamp_b: Vec4f,
    #[live(vec4(0.0, -1.0, 0.0, 1.0))]
    pub lamp_c: Vec4f,
    #[live(vec4(0.25, 8.0, 0.05, 0.02))]
    pub lamp_d: Vec4f,
}

/// Lamp rim fill / smooth. `misc_a` = (1/atlas_size, mode 0|1 ring, 2 smooth).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampDilate {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// Final atlas encode. `rect_px` = the EXPANDED rect in atlas texels.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmEncode {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// Shadow-top plane conversion. `top_a` = (zr, sun_dir.y, base, range);
/// `params_a` = (sun_up, depth bias z01, RAY_OFFSET).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmTop {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 8.0))]
    pub top_a: Vec4f,
    #[live(vec4(1.0, 0.001, 0.02, 0.0))]
    pub params_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub ground_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 2.0))]
    pub hf_a: Vec4f,
}

/// One ring of min-dilation of the shadow-top plane.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmTopDilate {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// THE baked-shadow penumbra: the smoothstep window every material shader
/// decodes the light atlas's signed-distance A channel through (128 = the
/// silhouette edge; the encode keeps a ±4-texel band, so the window is a
/// pure runtime knob — no re-bake to change it). This pair reads ±2.4
/// texels of the band: on the village ground region (~11.4 texels/unit)
/// that is a ~42 cm penumbra, on model charts (32 texels/unit) ~15 cm —
/// the (0.33, 0.67) window before it measured 24 cm / 8 cm and the user
/// still read the edges as hard. Widening the WINDOW is the whole lever
/// while it stays inside the encoded ±4: the band itself only needs to
/// grow (encode + chamfer step count + this window's world math) if a
/// future look wants penumbras past ±4 texels. Both modes share the atlas
/// encode, so both get it.
///
/// The macro'd shader text cannot interpolate a Rust const, so the pair is
/// written INLINE at every sampler; the test below holds the sites in
/// lockstep with this constant — edit the constant, then the sites it
/// flags. DrawLmTop's lit gate (`lm_a >=`) rides the TOP edge: every texel
/// the window would darken at all must carry a blocker height.
pub const LM_SUN_SOFT: (f32, f32) = (0.2, 0.8);

#[cfg(test)]
mod shader_registration_tests {
    use super::*;
    use makepad_draw::makepad_platform::makepad_script::script_eval;

    /// Cut-out casters inherit `morph_map` from DrawLmSunDepth, so their
    /// alpha texture `tex` sits in a LATER slot. The cascades bind it by
    /// name (gpu_lightmap::cutout_slot): bound at slot 0 it went to the
    /// morph lane, and the alpha test read a stale slot — tree stand-ins
    /// cast nothing, or whole rectangles that a low sun stretched into
    /// long straight-edged bands across the ground.
    #[test]
    fn cutout_casters_bind_their_alpha_texture_by_name() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_draw::script_mod(vm);
            vm.bx.heap.new_module(id!(prelude));
            script_eval!(vm, {
                mod.prelude.widgets_internal = { ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw: mod.draw, }
            });
            vm.bx.heap.new_module(id!(widgets));
            crate::local_shadows::sampling::script_mod(vm);
            crate::clustered::script_mod(vm);
            crate::fast_gi::script_mod(vm);
            super::script_mod(vm);
            crate::local_shadows::script_mod(vm);
            crate::custom_material::register(vm);
            let stock = DrawLmSunDepthCutout::script_new_with_default(vm).depth.draw_vars;
            let material = crate::custom_material::DrawMaterialShadow::script_new_with_default(vm).depth.depth.draw_vars;
            for (name, vars) in [("stock cut-out", stock), ("material caster", material)] {
                let cx = vm.cx();
                let textures = &cx.draw_shaders[vars.draw_shader_id.expect("registered").index].mapping.textures;
                let tex = textures.iter().position(|t| t.id == live_id!(tex)).expect("alpha texture");
                let morph = textures.iter().position(|t| t.id == live_id!(morph_map)).expect("morph lane");
                assert_ne!(tex, morph, "{name}");
                assert_eq!(crate::gpu_lightmap::cutout_slot(cx, &vars), Some(tex), "{name}: the alpha binds where the shader reads it");
            }
        });
    }

    #[test]
    fn cube_family_script_shaders_compile_without_errors() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            makepad_draw::script_mod(vm);
            vm.bx.heap.new_module(id!(prelude));
            script_eval!(vm, {
                mod.prelude.widgets_internal = {
                    ..mod.std,
                    ..mod.pod,
                    ..mod.math,
                    ..mod.sdf,
                    ..mod.shader,
                    draw:mod.draw,
                }
            });
            vm.bx.heap.new_module(id!(widgets));
            crate::local_shadows::sampling::script_mod(vm);
            crate::clustered::script_mod(vm);
            crate::fast_gi::script_mod(vm);
            super::script_mod(vm);
            crate::local_shadows::script_mod(vm);
            crate::custom_material::register(vm);

            // These shaders write numeric payloads (including negatives and
            // triangle ids), not display colors. A successful shader compile
            // alone does not detect an 8-bit pipeline / float-target mismatch.
            // Trace/relight carry exact payloads (Rgba32F); the gathered
            // field is the filtered receiver atlas (Rgba16F).
            for (vars, format) in [
                (crate::fast_gi::DrawGiTrace::script_new_with_default(vm).quad.draw_vars, "Rgba32F"),
                (crate::fast_gi::DrawGiRelight::script_new_with_default(vm).quad.draw_vars, "Rgba32F"),
                (crate::fast_gi::DrawGiGather::script_new_with_default(vm).quad.draw_vars, "Rgba16F"),
            ] {
                let id=vars.draw_shader_id.expect("GI shader registered");
                assert_eq!(format!("{:?}",vm.cx().draw_shaders[id.index].mapping.color_format),format);
                assert!(!vars.options.alpha_blend,"GI payloads must overwrite, not blend");
            }

            let cube_errors = script_eval!(vm, {
                mod.shader.test_compile_draw_errors(mod.draw.DrawSceneCube)
            });
            let alpha_errors = script_eval!(vm, {
                mod.shader.test_compile_draw_errors(mod.draw.DrawSceneAlpha)
            });
            let cube_errors = vm
                .bx
                .heap
                .string_with(cube_errors, |_heap, value| value.to_string())
                .expect("cube compile result should be a string");
            let alpha_errors = vm
                .bx
                .heap
                .string_with(alpha_errors, |_heap, value| value.to_string())
                .expect("alpha compile result should be a string");
            let setup_errors = vm.take_errors();

            assert!(setup_errors.is_empty(), "script setup errors: {setup_errors:#?}");
            assert!(cube_errors.is_empty(), "DrawSceneCube: {cube_errors}");
            assert!(alpha_errors.is_empty(), "DrawSceneAlpha: {alpha_errors}");
            for (name, result) in [
                ("GI trace", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiTrace)})),
                ("GI relight", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiRelight)})),
                ("GI gather", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawGiGather)})),
                ("terrain", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneTerrain)})),
                ("model", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneSkinned)})),
                ("pbr", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawScenePbr)})),
                ("skin", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneSkinnedGpu)})),
                ("custom", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneCustom)})),
                ("city", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneCity)})),
                ("grass", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneGrass)})),
                ("foliage", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawSceneFoliageLit)})),
                ("local shadow skin", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawLocalShadowSkinned)})),
                ("sun depth", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawLmSunDepth)})),
                ("sun skin depth", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawLmSunDepthSkinned)})),
                ("lamp depth", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawLmLampDepth)})),
                ("lamp depth inside", script_eval!(vm, {mod.shader.test_compile_draw_errors(mod.draw.DrawLmLampDepthInside)})),
            ] {
                let errors = vm.bx.heap.string_with(result, |_heap, value| value.to_string()).unwrap();
                assert!(errors.is_empty(), "{name}: {errors}");
            }
            // Cross-target Rust checks do not compile runtime Splash
            // shaders. Exercise GLSL and HLSL emission explicitly too.
            for (name, result) in [
                ("cube GLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawSceneCube, "glsl", false)})),
                ("PBR GLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawScenePbr, "glsl", false)})),
                ("skin GLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawSceneSkinnedGpu, "glsl", false)})),
                ("cube HLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawSceneCube, "hlsl", false)})),
                ("PBR HLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawScenePbr, "hlsl", false)})),
                ("city GLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawSceneCity, "glsl", false)})),
                ("city HLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawSceneCity, "hlsl", false)})),
                ("skin HLSL", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawSceneSkinnedGpu, "hlsl", false)})),
            ] {
                let source = vm.bx.heap.string_with(result, |_heap, value| value.to_string()).unwrap();
                assert!(!source.starts_with("ERRORS:"), "{name}: {source}");
                assert!(source.contains("cluster_lights"), "{name}: missing clustered shader");
            }
            for (name,result) in [
                ("GI trace GLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawGiTrace,"glsl",false)})),
                ("GI relight GLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawGiRelight,"glsl",false)})),
                ("GI gather GLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawGiGather,"glsl",false)})),
                ("GI trace HLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawGiTrace,"hlsl",false)})),
                ("GI relight HLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawGiRelight,"hlsl",false)})),
                ("GI gather HLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawGiGather,"hlsl",false)})),
                // Grass reads GI ambient but no clustered lights (blades are sun-lit).
                ("grass GLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawSceneGrass,"glsl",false)})),
                ("grass HLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawSceneGrass,"hlsl",false)})),
            ] {
                let source=vm.bx.heap.string_with(result,|_heap,value|value.to_string()).unwrap();
                assert!(!source.starts_with("ERRORS:"),"{name}: {source}");
                assert!(source.contains("gi_"),"{name}: missing GI code");
            }
            for (name,result) in [
                ("sun depth GLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawLmSunDepth,"glsl",false)})),
                ("skin depth GLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawLmSunDepthSkinned,"glsl",false)})),
                ("lamp depth HLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawLmLampDepth,"hlsl",false)})),
                ("local skin depth HLSL",script_eval!(vm,{mod.shader.test_compile_draw_source(mod.draw.DrawLocalShadowSkinned,"hlsl",false)})),
            ] {
                let source=vm.bx.heap.string_with(result,|_heap,value|value.to_string()).unwrap();
                assert!(!source.starts_with("ERRORS:"),"{name}: {source}");
                assert!(source.contains("morph_map"),"{name}: missing morph deformation");
            }
            let pbr = DrawScenePbr::script_new_with_default(vm);
            let id = pbr.skinned.draw_vars.draw_shader_id.expect("registered PBR shader");
            let cx = vm.cx();
            let textures = &cx.draw_shaders[id.index].mapping.textures;
            assert!(textures.len()<=pbr.skinned.draw_vars.texture_slots.len(),"PBR exceeds available combined texture bindings");
            // morph_map is sampled by vertex deformation only. All remaining
            // bindings conservatively count against WebGL2's 16-fragment-unit
            // floor, even when a material disables a branch at runtime.
            assert!(textures.iter().filter(|t|t.id!=live_id!(morph_map)).count()<=16,"PBR exceeds WebGL2 fragment texture budget");
            assert!(!textures.iter().any(|t|t.id==live_id!(gi_positions)||t.id==live_id!(gi_movers)),"GI preparation textures leaked into the scene shader");
            assert!(textures.iter().any(|texture|texture.id==live_id!(gi_field)));
            assert_eq!(textures[8].id, live_id!(cluster_data));
            assert_eq!(textures[9].id, live_id!(local_shadow_data));
            assert_eq!(textures[10].id, live_id!(local_shadow_map));
            assert!(textures.iter().any(|texture|texture.id==live_id!(orm_map)));
            assert!(textures.iter().any(|texture|texture.id==live_id!(morph_map)));
            // The grass lane instantiates (its field rides the instance
            // stream: the PBR uniform block has no room) and keeps the
            // rasters on the PBR lane's slots 0 and 1.
            let grass = DrawSceneGrass::script_new_with_default(vm);
            let id = grass.pbr.skinned.draw_vars.draw_shader_id.expect("registered grass shader");
            let cx = vm.cx();
            let mapping = &cx.draw_shaders[id.index].mapping;
            assert_eq!(mapping.textures[0].id, live_id!(tex));
            assert_eq!(mapping.textures[1].id, live_id!(ao_map));
            // Vertex attributes: every backend packs the geometry and the
            // instance records into vec4 chunks, one attribute each. Metal
            // allows 31, common Vulkan GPUs 32; one vec4 over it lost the
            // Vulkan device in race (2026-09-29). New per-draw data must ride
            // a spare lane, never a new instance field.
            let lanes = [
                ("model", DrawSceneSkinned::script_new_with_default(vm).draw_vars.draw_shader_id),
                ("pbr", DrawScenePbr::script_new_with_default(vm).skinned.draw_vars.draw_shader_id),
                ("custom", crate::custom_material::DrawSceneCustom::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id),
                ("city", DrawSceneCity::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id),
                ("grass", DrawSceneGrass::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id),
                ("foliage", DrawSceneFoliageLit::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id),
            ];
            for (name, id) in lanes {
                let m = &vm.cx().draw_shaders[id.expect("registered").index].mapping;
                let n = m.geometries.total_slots.div_ceil(4) + m.instances.total_slots.div_ceil(4);
                assert!(n <= 31, "{name}: {n} vertex attributes (Metal allows 31)");
            }
        });
    }
}

#[cfg(test)]
mod lm_soft_tests {
    use super::LM_SUN_SOFT;

    /// Every atlas sampler must decode the sun SDF through the SAME window,
    /// or the penumbra differs per material (a cube's shadow edge softer
    /// than the terrain's beside it). Scans the shader source for the
    /// decode idiom and pins each site to [`LM_SUN_SOFT`].
    #[test]
    fn every_sampler_decodes_the_same_penumbra_window() {
        let src = crate::shaders::SHADER_SOURCE;
        let expect = format!("smoothstep({}, {}, ", LM_SUN_SOFT.0, LM_SUN_SOFT.1);
        let mut sites = 0;
        for line in src.lines() {
            // The decode idiom: a smoothstep whose LAST argument is the
            // atlas A channel (`, lm.w)` / `, lmg.w)`) — the shadow-top
            // height compares also smoothstep near `v_lmg.w` but take it
            // as the edge argument, not the sample.
            if line.contains("smoothstep(")
                && (line.contains(", lm.w)") || line.contains(", lmg.w)"))
            {
                assert!(
                    line.contains(&expect),
                    "atlas decode window drifted from LM_SUN_SOFT {:?}: {}",
                    LM_SUN_SOFT,
                    line.trim()
                );
                sites += 1;
            }
        }
        assert!(
            sites >= 5,
            "expected the cube/skinned/skinned-gpu/terrain samplers, found {sites}"
        );
    }

    /// DrawLmTop's lit gate must sit at the decode window's TOP edge: a
    /// texel the window darkens AT ALL (outer penumbra included) needs a
    /// blocker height in the top plane, or that fringe shadows dynamics at
    /// any altitude (the Realtime "road shadows the villagers" mechanism).
    #[test]
    fn top_plane_lit_gate_matches_the_window_edge() {
        let src = crate::shaders::SHADER_SOURCE;
        // Needle assembled in two halves so this test's own source lines
        // never match the scan (the first test dodges the same trap by
        // splitting its needle across lines).
        let gate = format!("if lm_a{}", " >= ");
        let expect = format!("{}{} {{", gate, LM_SUN_SOFT.1);
        let mut sites = 0;
        for line in src.lines() {
            if line.contains(&gate) {
                assert!(
                    line.contains(&expect),
                    "DrawLmTop lit gate drifted from LM_SUN_SOFT.1 {}: {}",
                    LM_SUN_SOFT.1,
                    line.trim()
                );
                sites += 1;
            }
        }
        assert_eq!(sites, 1, "expected exactly the DrawLmTop gate");
    }
}

macro_rules! depth_morph_binding {
    ($kind:ty) => { impl $kind {
        pub(crate) fn set_morph(&mut self,cx:&Cx,morph:Option<&crate::asset_morph::DepthMorph>) {
            self.morph_ctl=morph.map_or(Vec4f::default(),|m|m.control);
            if let Some(morph)=morph {
                self.morph_weights0=morph.weights[0];
                self.morph_weights1=morph.weights[1];
                self.morph_weights2=morph.weights[2];
                self.morph_weights3=morph.weights[3];
                self.morph_weights4=morph.weights[4];
                self.morph_weights5=morph.weights[5];
                self.morph_weights6=morph.weights[6];
                self.morph_weights7=morph.weights[7];
                if let Some(shader)=self.draw_vars.draw_shader_id {
                    if let Some(slot)=cx.draw_shaders[shader.index].mapping.textures.iter().position(|texture|texture.id==live_id!(morph_map)) {self.draw_vars.set_texture(slot,&morph.texture);}
                }
            }
        }
    }};
}
depth_morph_binding!(DrawLmSunDepth);
depth_morph_binding!(DrawLmSunDepthSkinned);
depth_morph_binding!(DrawLmLampDepth);
