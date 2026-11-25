//! Splash materials on the model lanes (KERNELS.md §3.3): the Pbr lane with
//! a material's hooks installed (makepad-render-material), the Unlit kind,
//! the error material, opt-in IBL, and the derived shadow caster.
//!
//! The engine owns the lane: its vertex stage, cascades, clustered lights
//! and fog run unchanged, and the hooks replace only what they name (their
//! stock bodies are in shaders/pbr.rs, skinned.rs and clustered.rs). A
//! recoloured car keeps the clear-coat response of its stock material. A
//! host passes the hooks it allows: Sandbox's game.material allows the
//! albedo `surface` hook only (`HookMask::ALBEDO`).
use makepad_draw::*;
use makepad_draw::makepad_platform::makepad_script::makepad_shader::ReturnComponent;
use makepad_render_material::{self as material, builtin, Builtin, HookMask, HookSet, MaterialDesc, MaterialError, ShadowVariant, VariantPlan};
use makepad_scene::{BaseKind, Blend};
use crate::renderer::variants::{self, Features};
use crate::gpu_lightmap::TracedSunBatch;
use crate::shaders::{DrawLmSunDepthCutout, DrawScenePbr, DrawTracedSun};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCustom {
    #[deref] pub pbr: DrawScenePbr,
}

impl DrawSceneCustom {
    /// Set the material's four keyable floats (`self.params` in the hooks).
    pub fn set_params(&mut self, cx: &Cx, params: Vec4f) {
        self.pbr.skinned.draw_vars.set_uniform(cx, live_id!(params), &[params.x, params.y, params.z, params.w]);
    }

    /// The material's four keyable floats as last set.
    pub fn params(&self, cx: &Cx) -> Vec4f {
        let p = self.pbr.skinned.uniform_value(cx, live_id!(params));
        vec4(p[0], p[1], p[2], p[3])
    }
}

/// The derived shadow caster: the sun-depth projection with the material's
/// `vertex` hook, and for a masked material its `surface` alpha.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMaterialShadow {
    #[deref] pub depth: DrawLmSunDepthCutout,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))] pub params: Vec4f,
    /// Alpha under this cuts the caster (Mask); 0 casts every fragment.
    #[live(0.0)] pub cutoff: f32,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    // Default BGRA8 and source-over for this base and generated variants; alpha_blend starts false.
    // An Over recipe enables blending/no depth writes; layer binding can select both again per draw.
    // Scene colour targets are BGRA8/RGBA16F: clear is the host background, zero, or white for contact.
    // Ordinary passes are single-sample; XR may use MSAA. Depth clears to 1; surviving fragments combine in submission order.
    // Blended layers follow opaque layers (far-to-near within each lane), depth-tested without depth writes.
    // surface/finish hooks supply straight RGB; stock shading premultiplies finish alpha for coverage over prior colour.
    // Nonzero final RGB with alpha 0 adds light; stock premultiplication makes a zero-alpha finish contribute none.
    // Blend-off replaces colour/alpha; on the MSAA alpha-to-coverage path alpha selects samples, with straight RGB.
    // Linear-light output decodes using the draw's premultiplied flag; data-format declarations always write raw.
    // Earlier draws in this pass supply colour only through blending, never target reads; auxiliary MRT alpha is payload.
    mod.draw.DrawSceneCustom = mod.std.set_type_default() do #(DrawSceneCustom::script_shader(vm)) {
        ..mod.draw.DrawScenePbr
        // The material's four keyable floats (`self.params` in the hooks),
        // the whole per-draw budget; more data rides textures. A uniform: on
        // the instance stream they took the lane one output register past
        // what D3D11's `vs_5_0` allows, and they are one per material draw.
        params: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The control of an IBL program's two lookups (render-material's
        // lane functions read it; the renderer writes it per draw,
        // renderer/stock_ibl.rs): y = the lane's scale on the map (1 under
        // HDR output, the map's own exposure in the legacy lane, as the
        // rig's fill carries it), z = 1 while the fill comes from the map
        // (0: the stock IBL program keeps an authored fill).
        // x is the skin lane's switch (shaders/skinned_gpu.rs has the same
        // uniform); a program here is IBL by construction. The default is
        // the lookups as they are without a lane: the map's own scale, its
        // fill. Only a program with Builtin::Ibl reads it.
        ibl_ctl: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        surface: fn(base: vec4) -> vec4 { return base }
        // Mesh hook ABI: these values belong to the interpolated surface.
        // The shared PBR composition takes its own point and normal.
        mat_time: fn() -> float { return self.draw_pass.time }
        // The same clock for the vertex hook: a helper binds to one stage.
        mat_vtime: fn() -> float { return self.draw_pass.time }
        mat_pos: fn() -> vec3 { return self.v_csm.xyz }
        mat_uv: fn() -> vec2 { return self.v_uv }
        mat_geo_normal: fn() -> vec3 { return normalize(self.v_csm_n) }
    }

    mod.draw.DrawMaterialShadow = mod.std.set_type_default() do #(DrawMaterialShadow::script_shader(vm)) {
        ..mod.draw.DrawLmSunDepthCutout
        v_csm: varying(vec4f)
        v_csm_n: varying(vec3f)
        // What a colour lane's hooks may read for their alpha: the vertex
        // colour, and neutral stand-ins for the instance tint, the scene
        // transform and the eye (the caster's alpha never depends on them).
        v_tint: varying(vec4f)
        tint: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        to_scene: fn(c: vec3) -> vec3 { return c }
        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }
        mat_vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p }
        surface: fn(base: vec4) -> vec4 { return base }
        mat_time: fn() -> float { return self.draw_pass.time }
        // The same clock for the vertex hook: a helper binds to one stage.
        mat_vtime: fn() -> float { return self.draw_pass.time }
        mat_pos: fn() -> vec3 { return self.v_csm.xyz }
        mat_uv: fn() -> vec2 { return self.v_uv }
        mat_geo_normal: fn() -> vec3 { return normalize(self.v_csm_n) }
        vertex: fn() {
            var pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            if self.morph_ctl.w > 0.5 { pos = pos + self.morph_delta(self.geom.ao_uv, 0.0) }
            let n = self.oct_decode(unpack2f16(self.geom.nrm))
            self.v_uv = unpack2f16(self.geom.uv)
            self.v_tint = unpack4u8(self.geom.color)
            pos = self.mat_vertex(pos, n, self.v_uv)
            let wp = self.transform * vec4(pos, 1.0)
            self.v_csm = vec4(wp.xyz, 0.0)
            self.v_csm_n = (self.transform * vec4(n, 0.0)).xyz
            let nx = dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq * (1.0 - self.flip_a.y) + self.flip_a.z,
                1.0
            )
        }
        // The caster's alpha: 1 here; the Mask variant installs
        // mat_shadow_alpha_mask, the surface hook's alpha of the texel.
        mat_shadow_alpha: fn() -> float { return 1.0 }
        mat_shadow_alpha_mask: fn() -> float {
            let texel = self.tex.sample_repeat(self.v_uv)
            return self.surface(texel).w
        }
        pixel: fn() {
            if abs(self.v_clip.x) > 1.001 || abs(self.v_clip.y) > 1.001 {
                discard()
            }
            if self.mat_shadow_alpha() < self.cutoff {
                discard()
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }
    }

}

/// Install shader declarations in the owning isolate. Registration grants
/// no IO or host-widget access.
pub fn register(vm: &mut ScriptVm) -> ScriptValue {
    if vm.heap().type_default_for_id(DrawSceneCustom::script_type_id_static()).is_some() {
        builtin::register(vm);
        return NIL;
    }
    // The scene shaders exactly as `crate::script_mod` registers them (the
    // shared stdlib, the shadow sampling this build uses, ...): a second,
    // partial list here made different shaders (and shader-pack keys)
    // depending on which registration ran first.
    let scene = vm.heap().type_default_for_id(DrawScenePbr::script_type_id_static()).is_none();
    keep_for_reload(vm, scene);
    register_with(vm, scene)
}

/// Outside any `script_mod` of the VM's: its live-edit reload passes run the
/// registration again (the scene shaders too where it registered them), so
/// the materials it builds after an edit build on the edited shaders.
fn keep_for_reload(vm: &mut ScriptVm, scene: bool) {
    vm.keep_for_reload(live_id!(render_materials), move |vm| {
        register_with(vm, scene);
    });
}

fn register_with(vm: &mut ScriptVm, scene: bool) -> ScriptValue {
    if scene {
        crate::script_mod(vm);
    }
    builtin::register(vm);
    script_mod(vm)
}

/// [`register`] spread over calls, for a host that prepares a VM over frames
/// before its first material builds there: a VM without the scene shaders
/// registers them a module a call (`crate::script_mod_steps`; 14-23 ms in one
/// call in such a VM), then the materials' own module, then makes the
/// material draw once, which builds the stock shaders its type derives from
/// (the first material's build made them otherwise). [`register`] then finds
/// it all in place.
#[derive(Default)]
pub struct Registration {
    next: usize,
    scene: Option<bool>,
    made: bool,
}

impl Registration {
    /// Take the next step: true when one was taken, false once none is left.
    pub fn step(&mut self, vm: &mut ScriptVm) -> bool {
        if vm.heap().type_default_for_id(DrawSceneCustom::script_type_id_static()).is_none() {
            let scene = *self.scene.get_or_insert_with(|| vm.heap().type_default_for_id(DrawScenePbr::script_type_id_static()).is_none());
            if scene {
                if let Some((_, step)) = crate::script_mod_steps().nth(self.next) {
                    step(vm);
                    self.next += 1;
                    return true;
                }
            }
            keep_for_reload(vm, scene);
            register_with(vm, false);
            return true;
        }
        if std::mem::replace(&mut self.made, true) {
            return false;
        }
        DrawSceneCustom::script_new(vm);
        true
    }
}

/// Runs `f` in the VM that owns a material's program (a level isolate's);
/// false when that VM is busy. A material without one uses the Cx's VM.
pub type MaterialVm = Rc<dyn Fn(&mut Cx, &mut dyn FnMut(&mut ScriptVm)) -> bool>;

/// Direction of the camera ray at the rasterised face of a closed proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TracedLayerRole {
    Outside,
    Inside,
}

/// Availability of the program's required passes for the current device and pass layouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TracedSurfaceState {
    Waiting,
    Ready,
    Failed(String),
}

/// The outside scene pass is required by every surface. Include the other
/// passes that must be ready before the surface can draw. Sun is required
/// for a tile volume only while daylight Realtime cascades are active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TracedSurfacePasses {
    pub inside: bool,
    pub sun: bool,
    pub lamp: bool,
}

impl TracedSurfacePasses {
    pub const OUTSIDE: Self = Self { inside: false, sun: false, lamp: false };
    pub const ALL: Self = Self { inside: true, sun: true, lamp: true };

    fn requires(self, pass: TracedPass) -> bool {
        match pass {
            TracedPass::Outside => true,
            TracedPass::Inside => self.inside,
            TracedPass::Sun => self.sun,
            TracedPass::Lamp => self.lamp,
        }
    }
}

pub(crate) const TRACED_WEBGL_FAILURE: &str = "ray-traced surfaces aren't available on WebGL2 yet";

/// One occupied 8³ tile. Bounds are in tile-local cells, with an exclusive upper end.
#[derive(Clone, Copy, Debug)]
pub struct TracedTile {
    /// Model-grid coordinates of this tile's cell (0,0,0).
    pub origin_cells: [i32; 3],
    pub lo: [u8; 3],
    pub hi: [u8; 3],
    pub atlas_tile: [u8; 3],
    pub dominant_material: u8,
    pub palette_page: u8,
}

impl TracedTile {
    /// Append one conservative box to a static model's seven-float vertex stream.
    /// Adjacent boxes convert a shared grid corner to the same model-space metres.
    pub fn append_box(&self, vertices: &mut Vec<f32>, indices: &mut Vec<u32>, cell_pitch: f32) {
        assert!((0..3).all(|i| self.lo[i] < self.hi[i] && self.hi[i] <= 8));
        assert!(cell_pitch.is_finite() && cell_pitch > 0.0);
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        assert_eq!(vertices.len() % stride, 0);
        let start = u32::try_from(vertices.len() / stride).expect("tile vertex index");
        assert!(start <= u32::MAX - 7, "tile vertex indices exceed the model index format");
        let [x, y, z] = self.atlas_tile;
        let tile = f32::from_bits(u32::from_le_bytes([x, y, z, self.dominant_material]));
        let bounds = |i: usize| self.lo[i] | (self.hi[i] << 4);
        let aux = f32::from_bits(u32::from_le_bytes([bounds(0), bounds(1), bounds(2), self.palette_page]));
        for corner in 0..8 {
            let cell = [0, 1, 2].map(|i| f32::from(if corner & (1 << i) == 0 { self.lo[i] } else { self.hi[i] }));
            let [px, py, pz] = [0, 1, 2].map(|i| {
                let grid = f64::from(self.origin_cells[i]) + f64::from(cell[i]);
                (grid * f64::from(cell_pitch)) as f32
            });
            vertices.extend([px, py, pz, pack_pair_f16(cell[2], 0.0), pack_pair_f16(cell[0], cell[1]), tile, aux]);
        }
        for [a, b, c, d] in [[0,2,3,1], [4,5,7,6], [0,4,6,2], [1,3,7,5], [0,1,5,4], [2,6,7,3]] {
            indices.extend([a, b, c, a, c, d].map(|i| start + i));
        }
    }
}

/// A unit sphere by default. A material can replace the trace-space hooks
/// while the renderer retains its ray, clipping, depth and pass entry points.
#[derive(Clone, Default)]
pub struct TracedSurfaceRecipe {
    hooks: Vec<(LiveId, ScriptObjectRef)>,
    tile_cells_per_unit: Option<f32>,
    /// Material inputs shared by the program's passes. Each compiled pass
    /// resolves these names in its own texture layout.
    pub textures: Rc<[(LiveId, Texture)]>,
    pub host_vm: Option<MaterialVm>,
}

#[derive(Debug)]
pub(crate) enum TracedDrawError {
    VolumeBinding(&'static str),
    Other(String),
}

const DUPLICATE_TRACE_VOLUME: &str = "duplicate traced texture binding trace_volume";

impl From<String> for TracedDrawError {
    fn from(reason: String) -> Self { Self::Other(reason) }
}

impl TracedSurfaceRecipe {
    fn build_in_vm<T>(&self, cx: &mut Cx, mut build: impl FnMut(&mut ScriptVm) -> T) -> Option<T> {
        let mut result = None;
        let mut run = |vm: &mut ScriptVm| result = Some(build(vm));
        let ran = match &self.host_vm {
            Some(host) => host(cx, &mut run),
            None => cx.try_with_vm(&mut run).is_some(),
        };
        result.filter(|_| ran)
    }

    /// March an R8 atlas of 8³ tiles with a 256-wide palette. Bind them as
    /// `trace_volume` and `trace_palette` in `textures`. Each palette page has
    /// three rows: sRGB base colour and alpha, linear metallic/roughness in RG, and linear emission RGB.
    /// Use the same model-space cell pitch for [`TracedTile::append_box`].
    pub fn tiles(cell_pitch: f32) -> Self {
        assert!(cell_pitch > 0.0 && cell_pitch.is_finite() && cell_pitch.recip().is_finite());
        Self { tile_cells_per_unit: Some(cell_pitch.recip()), ..Self::default() }
    }

    pub fn set_hook(&mut self, vm: &mut ScriptVm, name: LiveId, function: ScriptObject) -> Result<(), String> {
        let allowed = [
            live_id!(traced_point), live_id!(traced_scale), live_id!(traced_min), live_id!(traced_max),
            live_id!(traced_hit), live_id!(traced_base), live_id!(traced_mr), live_id!(traced_emission),
        ];
        if !allowed.contains(&name) || !vm.is_fn(function) {
            return Err(format!("invalid traced surface hook {name}"));
        }
        self.hooks.retain(|(key, _)| *key != name);
        self.hooks.push((name, vm.new_object_ref(function)));
        Ok(())
    }

    fn shader_object(&self, vm: &mut ScriptVm, shader: LiveId) -> Result<ScriptObject, String> {
        let module = vm.value(vm.modules(), id!(draw)).as_object().ok_or("missing draw module")?;
        let base = vm.value(module, shader).as_object().ok_or_else(|| format!("missing traced surface shader {shader}"))?;
        let obj = vm.heap_mut().new_with_proto_no_vec(base.into());
        for (name, hook) in &self.hooks {
            if hook.heap_key() != vm.heap_key() || !vm.set_value(obj, *name, hook.as_object()).is_nil() {
                return Err(format!("could not install traced surface hook {name} in its VM"));
            }
        }
        Ok(obj)
    }

    pub(crate) fn outside_object(&self, vm: &mut ScriptVm, features: Features) -> Result<ScriptObject, String> {
        let shader = if self.tile_cells_per_unit.is_some() { id!(DrawSceneTracedTiles) } else { id!(DrawSceneTraced) };
        let obj = self.shader_object(vm, shader)?;
        variants::scene_variant(vm, obj, features, true, features.reflections)
            .ok_or_else(|| "could not specialize the outside surface shader".to_owned())
    }

    fn outside_draw(&self, vm: &mut ScriptVm, features: Features) -> Result<Box<DrawScenePbr>, TracedDrawError> {
        let obj = self.outside_object(vm, features)?;
        let mut draw = Box::new(DrawScenePbr::script_new(vm));
        draw.skinned.draw_vars.draw_shader_id = None;
        draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
        let vars = &mut draw.skinned.draw_vars;
        let id = vars.draw_shader_id.ok_or_else(|| makepad_draw::makepad_platform::shader_error::take()
            .unwrap_or_else(|| "outside surface shader did not compile".to_owned()))?;
        Self::bind_textures(vm.cx(), vars, id, &self.textures)?;
        if let Some(scale) = self.tile_cells_per_unit {
            vars.set_uniform(vm.cx(), live_id!(trace_cells_per_unit), &[scale]);
        }
        Ok(draw)
    }

    fn sun_binding(&self) -> Result<usize, &'static str> {
        self.tile_cells_per_unit.ok_or("the traced Sun draw requires a tile recipe")?;
        let mut volumes = self.textures.iter().enumerate().filter(|(_, (name, _))| *name == live_id!(trace_volume));
        let (binding, _) = volumes.next().ok_or("missing traced texture binding trace_volume")?;
        if volumes.next().is_some() {
            return Err(DUPLICATE_TRACE_VOLUME);
        }
        Ok(binding)
    }

    fn build_sun_draw(&self, vm: &mut ScriptVm, binding: usize) -> Result<DrawTracedSun, TracedDrawError> {
        let obj = self.shader_object(vm, id!(DrawTracedSunTiles))?;
        let mut draw = DrawTracedSun::script_new(vm);
        draw.draw_vars.draw_shader_id = None;
        draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
        let vars = &mut draw.draw_vars;
        let id = vars.draw_shader_id.ok_or_else(|| makepad_draw::makepad_platform::shader_error::take()
            .unwrap_or_else(|| "traced Sun shader did not compile".to_owned()))?;
        Self::bind_textures(vm.cx(), vars, id, std::slice::from_ref(&self.textures[binding]))?;
        vars.set_uniform(vm.cx(), live_id!(trace_cells_per_unit), &[self.tile_cells_per_unit.unwrap()]);
        Ok(draw)
    }

    fn bind_textures(cx: &Cx, vars: &mut DrawVars, id: DrawShaderId, textures: &[(LiveId, Texture)]) -> Result<(), TracedDrawError> {
        let inputs = &cx.draw_shaders[id.index].mapping.textures;
        for (i, (name, texture)) in textures.iter().enumerate() {
            let slot = inputs.iter().position(|input| input.id == *name)
                .filter(|&slot| slot < vars.texture_slots.len())
                .ok_or_else(|| format!("surface pass has no texture slot for {name}"))?;
            if textures[..i].iter().any(|(other, _)| other == name) {
                // Outside can find the duplicate before Sun checks its binding.
                if *name == live_id!(trace_volume) {
                    return Err(TracedDrawError::VolumeBinding(DUPLICATE_TRACE_VOLUME));
                }
                return Err(format!("duplicate traced texture binding {name}").into());
            }
            vars.set_texture(slot, texture);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TracedPass { Outside, Inside, Sun, Lamp }

const TRACED_PASSES: [TracedPass; 4] = [TracedPass::Outside, TracedPass::Inside, TracedPass::Sun, TracedPass::Lamp];

/// A pass can replace its attachments without changing its ID.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TracedScene {
    features: Features,
    pass: DrawPassId,
    layout: Option<PassLayout>,
    realtime_sun: bool,
    sun: Option<(DrawPassId, PassLayout)>,
}

impl TracedScene {
    pub(crate) fn new(cx: &Cx, pass: DrawPassId, features: Features, realtime_sun: bool, sun: Option<DrawPassId>) -> Self {
        Self { features, pass, layout: cx.pass_layout(pass), realtime_sun,
            sun: sun.and_then(|id| cx.pass_layout(id).map(|layout| (id, layout))) }
    }

    fn same_exterior(self, other: Self) -> bool {
        self.features == other.features && self.pass == other.pass && self.layout == other.layout
    }

    pub(crate) fn with_features(self, features: Features) -> Self { Self { features, ..self } }
}

pub(crate) struct TracedPasses {
    required: TracedSurfacePasses,
    active: Option<TracedScene>,
    shaders: [Option<DrawShaderId>; 4],
    ready: [bool; 4],
    failure: Option<String>,
    sun_failure: Option<String>,
}

impl TracedPasses {
    fn new(required: TracedSurfacePasses) -> Self {
        Self { required, active: None, shaders: [None; 4], ready: [false; 4], failure: None, sun_failure: None }
    }

    pub(crate) fn failure(&self, realtime_sun: bool) -> Option<&str> {
        self.failure.as_deref().or(self.sun_failure.as_deref().filter(|_| realtime_sun))
    }

    fn required_for(&self, realtime_sun: bool) -> TracedSurfacePasses {
        TracedSurfacePasses { sun: self.required.sun && realtime_sun, ..self.required }
    }

    fn ready_for(&self, passes: TracedSurfacePasses) -> bool {
        TRACED_PASSES.into_iter().all(|pass| !passes.requires(pass) || self.ready[pass as usize])
    }

    pub(crate) fn state(&self, features: Features, realtime_sun: bool, sun: Option<DrawPassId>) -> TracedSurfaceState {
        let required = self.required_for(realtime_sun);
        if let Some(reason) = self.failure(realtime_sun) {
            TracedSurfaceState::Failed(reason.to_owned())
        } else if self.active.is_some_and(|scene| scene.features == features
            && (!required.sun || scene.sun.is_some_and(|(id, _)| Some(id) == sun))) && self.ready_for(required) {
            TracedSurfaceState::Ready
        } else {
            TracedSurfaceState::Waiting
        }
    }

    pub(crate) fn refresh(&mut self, mut probe: impl FnMut(TracedPass, DrawShaderId, Option<DrawPassId>) -> Result<bool, String>) {
        let Some(scene) = self.active else { return };
        if self.failure(scene.realtime_sun).is_some() { return; }
        for pass in TRACED_PASSES {
            if !self.required.requires(pass) || (pass == TracedPass::Sun && self.sun_failure.is_some()) { continue; }
            let i = pass as usize;
            let Some(shader) = self.shaders[i] else { continue };
            let target = match pass {
                TracedPass::Outside | TracedPass::Inside => Some(scene.pass),
                TracedPass::Sun => {
                    let Some((id, _)) = scene.sun else { self.ready[i] = false; continue };
                    Some(id)
                }
                TracedPass::Lamp => None,
            };
            match probe(pass, shader, target) {
                Ok(ready) => self.ready[i] = ready,
                Err(reason) => {
                    if pass == TracedPass::Sun {
                        self.sun_failure = Some(reason);
                        if !scene.realtime_sun { continue; }
                    } else {
                        self.failure = Some(reason);
                    }
                    return;
                }
            }
        }
    }

    pub(crate) fn set_shader(&mut self, pass: TracedPass, shader: DrawShaderId) {
        let index = pass as usize;
        if self.shaders[index] != Some(shader) {
            self.shaders[index] = Some(shader);
            self.ready[index] = false;
        }
    }
}

pub(crate) struct TracedProgram {
    recipe: Option<TracedSurfaceRecipe>,
    pub(crate) passes: TracedPasses,
    outside: HashMap<Features, Box<DrawScenePbr>>,
    pub(crate) sun_batch: Option<Rc<RefCell<TracedSunBatch>>>,
    sun_binding: Result<usize, &'static str>,
    prepared_redraw: Option<u64>,
    generation: u64,
    /// Preview can draw Outside while the full program still reports Waiting.
    preview: bool,
    /// The install this program came from, from one counter for the whole process. A copy for
    /// another renderer keeps it, so each side can tell a replaced program from the same one,
    /// even across renderers rebuilt from scratch.
    pub(crate) serial: u64,
    /// On a copy: whether the source renderer itself had this program ready when last mirrored.
    pub(crate) source_ready: bool,
}

static TRACED_SERIALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Unique to one program install or one build of its Sun draw.
pub(crate) fn next_traced_serial() -> u64 {
    TRACED_SERIALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

impl TracedProgram {
    pub(crate) fn new(recipe: TracedSurfaceRecipe, required: TracedSurfacePasses, preview: bool, webgl: bool) -> Self {
        let mut passes = TracedPasses::new(TracedSurfacePasses { sun: required.sun && recipe.tile_cells_per_unit.is_some(), ..required });
        let sun_binding = recipe.sun_binding();
        if webgl { passes.failure = Some(TRACED_WEBGL_FAILURE.into()); }
        let sun_batch = (passes.required.sun && !webgl).then(Default::default);
        Self { recipe: (!webgl).then_some(recipe), passes, outside: Default::default(), sun_batch, sun_binding,
            prepared_redraw: None, generation: 0, preview, serial: 0, source_ready: false }
    }

    /// The same install for another renderer: the recipe is shared, and nothing compiled is.
    /// The copy builds and prewarms its own draws for its own pass target.
    pub(crate) fn installed_again(&self) -> Self {
        let webgl = self.recipe.is_none();
        let mut copy = Self::new(self.recipe.clone().unwrap_or_default(), self.passes.required, self.preview, webgl);
        copy.serial = self.serial;
        copy.passes.failure.clone_from(&self.passes.failure);
        copy.passes.sun_failure.clone_from(&self.passes.sun_failure);
        copy
    }

    /// Carry a failure the source program reached before this copy could.
    pub(crate) fn follow_failure_of(&mut self, source: &Self) {
        if self.passes.failure.is_none() && source.passes.failure.is_some() {
            self.passes.failure.clone_from(&source.passes.failure);
        }
        if self.passes.sun_failure.is_none() && source.passes.sun_failure.is_some() {
            self.passes.sun_failure.clone_from(&source.passes.sun_failure);
        }
    }

    pub(crate) fn wants_sun(&self) -> bool { self.passes.required.sun }

    pub(crate) fn sun(&self, scene: TracedScene) -> Option<&Rc<RefCell<TracedSunBatch>>> {
        let required = self.passes.required_for(scene.realtime_sun);
        if !required.sun || scene.sun.is_none()
            || !self.passes.active.is_some_and(|active| active.same_exterior(scene) && active.sun == scene.sun)
            || self.passes.failure(true).is_some() || !self.passes.ready_for(required) {
            return None;
        }
        self.sun_batch.as_ref().filter(|batch| batch.borrow().draw.is_some())
    }

    pub(crate) fn prepared_this_draw(&self, cx: &Cx, scene: TracedScene) -> bool {
        self.prepared_redraw == Some(cx.redraw_id()) && self.generation == cx.reload_generation
            && self.passes.active == Some(scene)
    }

    pub(crate) fn prepare_sun(&mut self, cx: &mut Cx,
        has_volume_format: impl FnOnce(&TextureFormat) -> Result<bool, &'static str>,
    ) -> Result<(), TracedDrawError> {
        let Some(batch) = &self.sun_batch else { return Ok(()) };
        if batch.borrow().draw.is_some() { return Ok(()); }
        let recipe = self.recipe.as_ref().expect("installed Sun recipe");
        let binding = self.sun_binding.map_err(TracedDrawError::VolumeBinding)?;
        // Both passes bind the handle. Its owner decides when to publish its texels.
        if !has_volume_format(recipe.textures[binding].1.get_format(cx)).map_err(TracedDrawError::VolumeBinding)? { return Ok(()); }
        if let Some(draw) = recipe.build_in_vm(cx, |vm| recipe.build_sun_draw(vm, binding)).transpose()? {
            self.passes.set_shader(TracedPass::Sun, draw.draw_vars.draw_shader_id.expect("compiled Sun draw"));
            let mut batch = batch.borrow_mut();
            batch.draw_serial = next_traced_serial();
            batch.draw = Some(draw);
        }
        Ok(())
    }

    pub(crate) fn prepare(&mut self, cx: &mut Cx, scene: TracedScene, name: &str) {
        self.prepare_with_volume_support(cx, scene, name, Cx::sampled_volume_support);
    }

    pub(crate) fn prepare_with_volume_support(
        &mut self, cx: &mut Cx, scene: TracedScene, name: &str, volume_support: impl Fn(&Cx) -> Option<Result<(), &'static str>>,
    ) {
        self.prepared_redraw = Some(cx.redraw_id());
        let Some(recipe) = &self.recipe else { return };
        if self.generation != cx.reload_generation {
            self.generation = cx.reload_generation;
            self.outside.clear();
            if let Some(batch) = &self.sun_batch { batch.borrow_mut().draw = None; }
            self.passes = TracedPasses::new(self.passes.required);
        }
        if self.passes.failure(scene.realtime_sun).is_some() { return }
        let wants_sun = self.wants_sun();
        let diagnostic = |cx: &Cx, error| match error {
            TracedDrawError::VolumeBinding(binding) => {
                let device = if wants_sun { volume_support(cx).and_then(Result::err) } else { None };
                device.unwrap_or(binding).to_owned()
            }
            TracedDrawError::Other(reason) => reason,
        };
        let features = scene.features;
        if !self.outside.contains_key(&features) {
            match recipe.build_in_vm(cx, |vm| recipe.outside_draw(vm, features)) {
                Some(Ok(draw)) => { self.outside.insert(features, draw); }
                Some(Err(error)) => {
                    let reason = diagnostic(cx, error);
                    log!("render: traced surface {name} outside shader failed: {reason}");
                    self.passes.failure = Some(reason);
                    return;
                }
                None => { self.passes.active = None; return }
            }
        }
        let id = self.outside[&features].skinned.draw_vars.draw_shader_id.unwrap();
        self.passes.set_shader(TracedPass::Outside, id);
        if self.passes.sun_failure.is_none() {
            if let Err(error) = self.prepare_sun(cx, |format| match format {
                TextureFormat::Unknown => Ok(false),
                TextureFormat::VecVolume(_) => Ok(true),
                _ => Err("trace_volume must be a sampled 3D texture"),
            }) {
                let reason = diagnostic(cx, error);
                log!("render: traced surface {name} Sun draw failed: {reason}");
                self.passes.sun_failure = Some(reason);
            }
        }
        // Off has no attachments. Warm their layout without retaining an atlas.
        if scene.sun.is_none() && self.passes.sun_failure.is_none() {
            if let Some(id) = self.passes.shaders[TracedPass::Sun as usize] {
                cx.draw_shader_ready_for(id, &crate::gpu_lightmap::cascade_layout());
            }
        }
        self.passes.active = Some(scene);
        self.passes.refresh(|pass, shader, target| {
            let layout = match pass {
                TracedPass::Sun => scene.sun.map(|(_, layout)| layout),
                TracedPass::Outside | TracedPass::Inside => scene.layout,
                TracedPass::Lamp => None,
            };
            if layout.is_some_and(|layout| cx.draw_shader_ready_for(shader, &layout)) { return Ok(true); }
            let reason = match target {
                Some(pass) => cx.draw_shader_failure_in(shader, pass),
                None => cx.draw_shader_failure(shader),
            };
            if let Some(reason) = reason {
                log!("render: traced surface {name} {pass:?} pipeline failed: {reason}");
                return Err(reason.to_owned());
            }
            Ok(false)
        });
    }

    pub(crate) fn outside(&mut self, scene: TracedScene) -> Option<&mut DrawScenePbr> {
        let required = if self.preview { TracedSurfacePasses::OUTSIDE } else { self.passes.required_for(scene.realtime_sun) };
        let matches = self.passes.active.is_some_and(|active| {
            active.same_exterior(scene) && (!required.sun || active.sun == scene.sun)
        });
        if !matches {
            self.passes.active = None;
            return None;
        }
        if required.sun {
            self.sun(scene)?;
        } else if self.passes.failure(scene.realtime_sun).is_some() || !self.passes.ready_for(required) {
            return None;
        }
        let active = self.passes.shaders[TracedPass::Outside as usize];
        self.outside.get_mut(&scene.features)
            .filter(|draw| draw.skinned.draw_vars.draw_shader_id == active)
            .map(Box::as_mut)
    }
}

/// How a material was built: what builds it again on the lanes' defaults as
/// they are after a live edit (`CustomMaterial::rebuild`). The objects it
/// names live in the program's heap, held there by `_keep`.
#[derive(Clone)]
enum Recipe {
    /// [`DrawSceneCustom::build_with`] (`build`, `unlit` and `from_surface`
    /// come through it).
    Hooks { desc: MaterialDesc, hooks: HookSet, mask: HookMask, params: Vec4f, helpers: Vec<(LiveId, ScriptObject)>, _keep: Rc<[ScriptObjectRef]> },
    /// [`DrawSceneCustom::error_material`].
    Error,
}

impl Recipe {
    fn build(&self, vm: &mut ScriptVm) -> Result<CustomMaterial, MaterialError> {
        match self {
            Recipe::Hooks { desc, hooks, mask, params, helpers, .. } => DrawSceneCustom::build_with(vm, desc, hooks, *mask, *params, helpers),
            Recipe::Error => DrawSceneCustom::error_material(vm),
        }
    }
}

/// `MAKEPAD_SHADER_VARIANTS=0`: every material draws its stock shader.
fn variants_off() -> bool {
    std::env::var_os("MAKEPAD_SHADER_VARIANTS").is_some_and(|v| v == "0")
}

/// What the program's `surface` hook returns as alpha, as proved by the compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceAlpha {
    NoHook,
    One,
    Base,
    Unproven,
}

/// A built material: the colour draw, and what its plan derived.
pub struct CustomMaterial {
    pub draw: DrawSceneCustom,
    pub surface_alpha: SurfaceAlpha,
    pub plan: VariantPlan,
    /// The blend the program declares before any layer is bound.
    pub(crate) blend: Blend,
    /// The colour program, which the feature variants derive from
    /// (renderer/variants.rs), and its stock shader.
    program: Option<ScriptObjectRef>,
    stock: Option<DrawShaderId>,
    /// The VM that owns `program`, when it is not the Cx's.
    pub host_vm: Option<MaterialVm>,
    /// The variants built so far per feature set: the colour shader and,
    /// for a material no hook of which can cut a pixel, the one without
    /// `clip`. `None`: not available (its layout differs), stock instead.
    variants: HashMap<Features, Option<(DrawShaderId, Option<DrawShaderId>)>>,
    /// The derived caster (`ShadowVariant::Derived`), when it compiled.
    pub shadow: Option<DrawShaderId>,
    pub cutoff: f32,
    /// How far the vertex hook may move geometry (culling grows by it).
    pub bounds_pad: f32,
    /// The program samples the IBL texture (bind it on `detail_map`).
    pub ibl: bool,
    /// A texture the host binds on `detail_map` for this program (a
    /// planar reflection, a screen): wins over the IBL.
    pub texture: Option<Texture>,
    /// Problems that did not stop the colour program (a caster that did not
    /// compile casts the stock shadow instead).
    pub warnings: Vec<material::Diagnostic>,
    /// Why it does not draw as written, once known: a feature variant that
    /// did not build, or a shader the backend's compiler refused. It draws
    /// the stock look meanwhile; [`Self::take_failure`] hands it over once.
    failure: Option<String>,
    failure_taken: bool,
    /// What builds it again, and the reload walk (`Cx::reload_generation`)
    /// it was built after.
    recipe: Recipe,
    pub(crate) built_at: u64,
    /// This frame's decision (`Renderer::prepare_custom_lanes`): its
    /// variant for the frame's features is built and its pipeline made, so
    /// its own lane draws the layers that name it. Until then the stock
    /// lanes draw them, and its lane leaves them.
    pub(crate) lane_ready: bool,
}

impl CustomMaterial {
    fn new(vm: &mut ScriptVm, draw: DrawSceneCustom, plan: VariantPlan, program: Option<ScriptObjectRef>, recipe: Recipe) -> Self {
        Self {
            stock: draw.draw_vars.draw_shader_id,
            recipe,
            built_at: vm.cx().reload_generation,
            draw,
            surface_alpha: SurfaceAlpha::NoHook,
            plan,
            blend: Blend::Opaque,
            program,
            host_vm: None,
            variants: HashMap::new(),
            shadow: None,
            cutoff: 0.0,
            bounds_pad: 0.0,
            ibl: false,
            texture: None,
            warnings: Vec::new(),
            failure: None,
            failure_taken: false,
            lane_ready: false,
        }
    }

    /// Whether this is the error material ([`DrawSceneCustom::error_material`]).
    pub fn is_error_material(&self) -> bool {
        matches!(self.recipe, Recipe::Error)
    }

    /// Why this material does not draw as written (a variant that did not
    /// build, or a pipeline the backend refused, with the compiler's own
    /// words), the first time that is known; `None` after, and while it
    /// draws. A few lookups a frame, no allocation until it fails.
    pub fn take_failure(&mut self, cx: &Cx) -> Option<&str> {
        if self.failure_taken { return None; }
        if self.failure.is_none() {
            let built = self.variants.values().flatten().flat_map(|&(full, no_clip)| std::iter::once(full).chain(no_clip));
            let log = self.stock.into_iter().chain(built).find_map(|id| cx.draw_shader_failure(id));
            self.failure = log.map(|log| format!("the GPU's shader compiler refused it: {}", log.trim()));
        }
        self.failure_taken = self.failure.is_some();
        self.failure.as_deref()
    }

    /// The colour program's own shader (the one built for every feature).
    pub fn stock_shader(&self) -> Option<DrawShaderId> {
        self.stock
    }

    /// Whether the program composes light: the Unlit, Flat and error
    /// programs do not, so their variants drop the lighting the lane
    /// computes for the composition.
    fn lit(&self) -> bool {
        !self.plan.builtins.iter().any(|b| matches!(b, Builtin::Unlit | Builtin::Flat | Builtin::Error))
    }

    /// The blend the colour draw is set to (0 opaque, 1 mask, 2 over): a
    /// uniform of the lane, read back from the draw.
    pub fn alpha_mode(&self, cx: &Cx) -> f32 {
        self.draw.pbr.skinned.uniform_value(cx, live_id!(alpha_mode))[0]
    }

    /// Whether no draw of this material cuts a pixel (no hook can discard,
    /// opaque blend): it may draw through the variant without `clip`.
    pub fn cuts_no_pixel(&self, cx: &Cx) -> bool {
        !self.plan.can_discard && self.alpha_mode(cx) == 0.0
    }

    /// The shaders for this frame's `features` (renderer/variants.rs): the
    /// one the material draws through, and the one without `clip` when no
    /// draw of it cuts a pixel. Built on first use; until then (its VM is
    /// busy), or when a variant's layout differs, the stock shader.
    pub(crate) fn shaders(&mut self, cx: &mut Cx, features: Features) -> (Option<DrawShaderId>, Option<DrawShaderId>) {
        let Some(stock) = self.stock else { return (None, None) };
        if variants_off() {
            return (Some(stock), None);
        }
        let features = self.variant_key(features);
        if !self.variants.contains_key(&features) {
            let Some(program) = self.program.clone() else { return (Some(stock), None) };
            let (lit, opaque) = (self.lit(), self.cuts_no_pixel(cx) && std::env::var_os("MAKEPAD_OPAQUE_VARIANT").is_none_or(|v| v != "0"));
            let mut built = None;
            let mut failed = None;
            let mut run = |vm: &mut ScriptVm| {
                // Only against the heap that owns the program.
                if vm.heap_key() != program.heap_key() { built = Some(None); return; }
                let full = build_variant(vm, program.as_object(), stock, features, lit).unwrap_or_else(|e| { failed = Some(e); None });
                let no_clip = if opaque { build_variant(vm, program.as_object(), stock, Features { clip: false, ..features }, lit).ok().flatten() } else { None };
                built = Some(full.map(|full| (full, no_clip)));
            };
            let ran = match &self.host_vm {
                Some(host) => host(cx, &mut run),
                None => cx.try_with_vm(|vm| run(vm)).is_some(),
            };
            match built {
                Some(built) if ran => { self.variants.insert(features, built); }
                _ => return (Some(stock), None),
            }
            if let Some(e) = failed {
                // Drawn stock meanwhile: its host hears why (`take_failure`).
                self.failure.get_or_insert_with(|| format!("it did not build for this frame's lighting: {e}"));
            }
        }
        match self.variants[&features] {
            Some((full, no_clip)) => (Some(full), no_clip),
            None => (Some(stock), None),
        }
    }

    /// The shader [`Self::shaders`] draws `features` through, when it is
    /// there without building anything: `None` while its variant for them
    /// is still to be built.
    pub(crate) fn built_shader(&self, features: Features) -> Option<DrawShaderId> {
        let stock = self.stock?;
        if variants_off() {
            return Some(stock);
        }
        match self.variants.get(&self.variant_key(features)) {
            Some(Some((full, _))) => Some(*full),
            Some(None) => Some(stock),
            None => self.program.is_none().then_some(stock),
        }
    }

    /// The feature set a variant is built and kept for. Arbitrary
    /// light/lighting hooks can make ambient specular non-additive: those
    /// programs keep their own reflection until they expose a separable term.
    fn variant_key(&self, features: Features) -> Features {
        Features { clip: true, reflections: features.reflections && !self.plan.builtins.contains(&Builtin::HookedCompose), ..features }
    }

    /// The material built again from its recipe in the VM that owns its
    /// program, on the lanes' defaults as a live edit left them, with what
    /// its host set since (params, texture, its VM). `None` when that VM is
    /// busy: the next frame.
    pub(crate) fn rebuild(&self, cx: &mut Cx) -> Option<Result<CustomMaterial, MaterialError>> {
        let params = self.draw.params(cx);
        let heap = self.program.as_ref().map(|p| p.heap_key());
        let mut built = None;
        let mut run = |vm: &mut ScriptVm| {
            // Only in the heap that owns the program (as `shaders`).
            built = Some(match heap.is_none_or(|heap| heap == vm.heap_key()) {
                true => self.recipe.build(vm),
                false => Err(MaterialError::new("its VM entered another heap than its program's".to_string())),
            })
        };
        let ran = match &self.host_vm {
            Some(host) => host(cx, &mut run),
            None => cx.try_with_vm(|vm| run(vm)).is_some(),
        };
        let built = built.filter(|_| ran)?;
        Some(built.map(|mut m| {
            m.draw.set_params(cx, params);
            m.texture = self.texture.clone();
            m.host_vm = self.host_vm.clone();
            m
        }))
    }
}

/// `program`'s variant for `features` (the stock shader when it replaces
/// nothing), `None` when its layout differs, the error when it does not build.
fn build_variant(vm: &mut ScriptVm, program: ScriptObject, stock: DrawShaderId, features: Features, lit: bool) -> Result<Option<DrawShaderId>, MaterialError> {
    let Some(obj) = variants::variant_object(vm, program, features, lit) else { return Ok(Some(stock)) };
    let Some(variant) = instance_custom(vm, obj)?.draw_vars.draw_shader_id else { return Ok(None) };
    if !variants::same_layout(vm.cx(), stock, variant) {
        log!("render: a material's variant {features:?} has another layout than its shader; drawing through that");
        return Ok(None);
    }
    Ok(Some(variant))
}

/// Build one program object on the lane and instance a draw struct from it.
fn instance_custom(vm: &mut ScriptVm, obj: ScriptObject) -> Result<DrawSceneCustom, MaterialError> {
    // #[deref] construction applies DrawScenePbr's defaults and can already
    // have compiled its stock shader: clear it before testing this candidate.
    let mut draw = DrawSceneCustom::script_new(vm);
    draw.draw_vars.draw_shader_id = None;
    draw.draw_vars.geometry_id = None;
    draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
    if !draw.draw_vars.can_instance() {
        let detail = makepad_draw::makepad_platform::shader_error::take().unwrap_or_else(|| "the compiler gave no message".into());
        return Err(MaterialError::new(format!("the material's shader did not build: {detail}")));
    }
    Ok(draw)
}

impl DrawSceneCustom {
    /// A Splash material: `hooks` (checked against `mask`) on the kind the
    /// description names, with every variant its plan derives. Errors carry
    /// the author's source lines.
    pub fn build(vm: &mut ScriptVm, desc: &MaterialDesc, hooks: &HookSet, mask: HookMask, params: Vec4f) -> Result<CustomMaterial, MaterialError> {
        Self::build_with(vm, desc, hooks, mask, params, &[])
    }

    /// [`Self::build`] plus the author's own helper functions (called by the
    /// hooks). A helper may add a name, never replace one the lane has:
    /// that would reach past the hook set.
    pub fn build_with(vm: &mut ScriptVm, desc: &MaterialDesc, hooks: &HookSet, mask: HookMask, params: Vec4f, helpers: &[(LiveId, ScriptObject)]) -> Result<CustomMaterial, MaterialError> {
        if !matches!(desc.blend, Blend::Opaque | Blend::Mask { .. } | Blend::Over) {
            return Err(MaterialError::new("the model lane draws Opaque, Mask and Over materials; Add, Multiply and Screen blend on the line and point lanes"));
        }
        register(vm);
        let plan = material::plan(desc, hooks)?;
        let base = vm.heap().type_default_for_id(Self::script_type_id_static())
            .ok_or_else(|| MaterialError::new("custom material shader registration failed"))?;
        let mut overrides = Vec::new();
        for (name, f) in helpers {
            let existing = vm.object_method(base, *name);
            let taken = !(existing.is_nil() || existing.is_err())
                || material::HOOKS.iter().any(|h| LiveId::from_str(h.method) == *name);
            if taken || !vm.is_fn(*f) {
                return Err(MaterialError::new(format!("helper `{name}` would replace a function the lane owns; give it another name")));
            }
            overrides.push((*name, *f));
        }
        for b in &plan.builtins {
            overrides.extend(builtin::overrides(vm, *b));
        }
        let obj = material::install_program(vm, &material::ProgramRequest { base, kind: desc.kind, hooks, mask, overrides: &overrides })?;
        // Compiled once; the front end is asked for located errors only
        // when that fails.
        let mut draw = match instance_custom(vm, obj) {
            Ok(draw) => draw,
            Err(e) => return Err(material::diagnose(vm, obj, hooks).err().unwrap_or(e)),
        };
        let surface_alpha = if hooks.has(material::Hook::Surface) {
            let alpha = draw.draw_vars.draw_shader_id.and_then(|shader| {
                let facts = vm.cx().draw_shader_return_facts(shader, live_id!(surface))?;
                (facts.len == 4).then_some(facts.components[3])
            });
            match alpha {
                Some(ReturnComponent::Const(1.0)) => SurfaceAlpha::One,
                Some(ReturnComponent::Param { name, component: 3 }) if name == live_id!(base) => {
                    SurfaceAlpha::Base
                }
                _ => SurfaceAlpha::Unproven,
            }
        } else {
            SurfaceAlpha::NoHook
        };
        draw.set_params(vm.cx(), params);
        let cutoff = match desc.blend {
            Blend::Mask { cutoff } => cutoff,
            _ => 0.0,
        };
        match desc.blend {
            Blend::Mask { cutoff } => {
                draw.pbr.skinned.draw_vars.set_uniform(vm.cx(), live_id!(alpha_mode), &[1.0]);
                draw.pbr.skinned.draw_vars.set_uniform(vm.cx(), live_id!(alpha_cutoff), &[cutoff]);
            }
            Blend::Over => {
                draw.pbr.skinned.draw_vars.set_uniform(vm.cx(), live_id!(alpha_mode), &[2.0]);
                draw.draw_vars.options.alpha_blend = true;
                draw.draw_vars.options.depth_write = false;
            }
            _ => {}
        }
        let mut warnings = Vec::new();
        let shadow = match plan.shadow {
            ShadowVariant::Derived { vertex, mask } => match Self::build_shadow(vm, hooks, vertex, mask, &overrides[..helpers.len()]) {
                Ok(id) => Some(id),
                Err(e) => {
                    warnings.extend(e.diagnostics.into_iter().map(|mut d| {
                        d.message = format!("its shadow caster did not build, so it casts the stock shadow: {}", d.message);
                        d
                    }));
                    None
                }
            },
            _ => None,
        };
        let ibl = plan.builtins.contains(&Builtin::Ibl);
        let program = vm.new_object_ref(obj);
        let keep = hooks.iter().map(|(_, f)| f).chain(helpers.iter().map(|(_, f)| *f)).map(|f| vm.new_object_ref(f)).collect();
        let recipe = Recipe::Hooks { desc: *desc, hooks: hooks.clone(), mask, params, helpers: helpers.to_vec(), _keep: keep };
        Ok(CustomMaterial {
            blend: desc.blend,
            surface_alpha,
            ibl,
            shadow,
            cutoff,
            bounds_pad: hooks.bounds_pad,
            warnings,
            ..CustomMaterial::new(vm, draw, plan, Some(program), recipe)
        })
    }

    fn build_shadow(vm: &mut ScriptVm, hooks: &HookSet, vertex: bool, mask: bool, helpers: &[(LiveId, ScriptObject)]) -> Result<DrawShaderId, MaterialError> {
        let base = vm.heap().type_default_for_id(DrawMaterialShadow::script_type_id_static())
            .ok_or_else(|| MaterialError::new("material shadow shader registration failed"))?;
        let mut caster_hooks = HookSet::new();
        // The author's helpers, where the caster has no function of that name.
        let mut overrides: Vec<(LiveId, ScriptObject)> = helpers.iter().copied().filter(|(name, _)| {
            let v = vm.object_method(base, *name);
            v.is_nil() || v.is_err()
        }).collect();
        if vertex {
            if let Some(f) = hooks.get(material::Hook::Vertex) { caster_hooks.set(material::Hook::Vertex, f); }
        }
        if mask {
            if let Some(f) = hooks.get(material::Hook::Surface) { caster_hooks.set(material::Hook::Surface, f); }
            if let Some(alpha) = vm.value(base, id!(mat_shadow_alpha_mask)).as_object() {
                overrides.push((id!(mat_shadow_alpha), alpha));
            }
        }
        let obj = material::build_program(vm, &material::ProgramRequest { base, kind: BaseKind::Pbr, hooks: &caster_hooks, mask: HookMask::ALL, overrides: &overrides })?;
        let mut draw = DrawMaterialShadow::script_new(vm);
        draw.draw_vars.draw_shader_id = None;
        draw.draw_vars.geometry_id = None;
        draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
        draw.draw_vars.draw_shader_id.filter(|_| draw.draw_vars.can_instance())
            .ok_or_else(|| MaterialError::new("the shadow caster's shader did not build"))
    }

    /// Unlit: the base colour lifted by `stops` of HDR intensity (params.w;
    /// 0 = as authored), no light, shadow or reflection. An instance's own
    /// params replace the material's, so a host keeps w for the intensity.
    pub fn unlit(vm: &mut ScriptVm, hooks: &HookSet, mask: HookMask, stops: f32) -> Result<CustomMaterial, MaterialError> {
        let desc = MaterialDesc { kind: BaseKind::Unlit, ..Default::default() };
        Self::build(vm, &desc, hooks, mask, vec4(0.0, 0.0, 0.0, stops))
    }

    /// The error material: hatched magenta, unlit. What a material that did
    /// not compile draws as in a preview.
    pub fn error_material(vm: &mut ScriptVm) -> Result<CustomMaterial, MaterialError> {
        register(vm);
        let overrides = builtin::overrides(vm, Builtin::Error);
        let base = vm.heap().type_default_for_id(Self::script_type_id_static())
            .ok_or_else(|| MaterialError::new("custom material shader registration failed"))?;
        let obj = material::build_program(vm, &material::ProgramRequest { base, kind: BaseKind::Unlit, hooks: &HookSet::new(), mask: HookMask::NONE, overrides: &overrides })?;
        let draw = instance_custom(vm, obj)?;
        let program = vm.new_object_ref(obj);
        Ok(CustomMaterial::new(vm, draw, VariantPlan { builtins: vec![Builtin::Error], can_discard: false, shadow: ShadowVariant::Stock }, Some(program), Recipe::Error))
    }

    /// Sandbox's game.material: one albedo `surface(base: vec4) -> vec4`
    /// (a host whose program lives in another VM than the Cx's sets the
    /// material's `host_vm`).
    pub fn from_surface(vm: &mut ScriptVm, surface: ScriptObject, params: Vec4f) -> Result<CustomMaterial, String> {
        if !vm.is_fn(surface) { return Err("surface is not a shader function".into()); }
        let hooks = HookSet::new().with(material::Hook::Surface, surface);
        Self::build(vm, &MaterialDesc::default(), &hooks, HookMask::ALBEDO, params)
            .map_err(|e| format!("surface shader failed frontend compilation: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    use makepad_render_material::{frontend_errors_for, Hook, ShaderBackend};
    use makepad_scene::LightingModel;

    fn with_vm(f: impl FnOnce(&mut ScriptVm)) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            vm.capture_errors();
            makepad_draw::script_mod(vm);
            vm.heap_mut().new_module(id!(prelude));
            script_eval!(vm, { mod.prelude.widgets_internal = { ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw } });
            vm.heap_mut().new_module(id!(widgets));
            register(vm);
            f(vm);
        });
    }

    /// A material document's hooks, evaluated from Splash source under a
    /// file name (so errors can name its lines).
    fn spec(vm: &mut ScriptVm, file: &str, code: &str) -> ScriptObject {
        let code = format!("use mod.prelude.widgets_internal.*\n{{\n{code}\n}}");
        // Hosts evaluate documents with `line: 1`, so rows report 1-based.
        let value = vm.eval(ScriptMod::new("", "", &file, 1, 0, &code, &[]));
        value.as_object().unwrap_or_else(|| panic!("spec did not evaluate: {:?}", vm.take_errors()))
    }

    #[test]
    fn traced_tile_march_variants() {
        with_vm(|vm| {
            let recipe = TracedSurfaceRecipe::tiles(0.125);
            let features = Features { clip: true, reflections: true, gi: true, gi_debug: true,
                csm: true, csm_debug: true, cluster: true, local_shadows: true, ..Default::default() };
            let program = recipe.outside_object(vm, features).expect("tiled outside scene");
            crate::shaders::inspect_scene_backends(vm, program, "tile march", Some(true));
            let compiled = crate::shaders::compile_scene_webgl2(vm, program, "tile march");
            let frag = compiled.view().glsl_fragment;
            let hit = crate::shaders::lowered_fn_body(frag, "traced_hit").expect("tile intersection");
            let (_, walk) = hit.split_once("for (").expect("tile walk must reach the emitted shader");
            assert!(walk.lines().next().unwrap().contains("24") && walk.contains("texelFetch(tex_trace_volume,"),
                "bounded integer volume load must occur inside the walk");
            let read: std::collections::HashSet<_> = frag.lines()
                .filter(|line| !line.trim_start().starts_with("uniform "))
                .flat_map(|line| line.split(|c: char| c != '_' && !c.is_ascii_alphanumeric()))
                .filter(|word| word.starts_with("tex_")).collect();
            assert!(read.contains("tex_trace_volume") && read.contains("tex_trace_palette"), "both tile resources must contribute");
            let draw = recipe.outside_draw(vm, features).expect("tiled scene binding layout");
            let id = draw.skinned.draw_vars.draw_shader_id.unwrap();
            let mut cells_per_unit = [0.0];
            draw.skinned.draw_vars.get_uniform(vm.cx_mut(), live_id!(trace_cells_per_unit), &mut cells_per_unit);
            assert_eq!(cells_per_unit, [8.0], "the scene must convert cell distance back to model-space depth");
            let slots = &vm.cx().draw_shaders[id.index].mapping.textures;
            let names: std::collections::HashSet<_> = slots.iter().map(|slot| format!("tex_{}", slot.id)).collect();
            assert!(read.iter().all(|name| names.contains(*name)), "emitted fragment sampler without a binding");
            let vertex_only = slots.iter().any(|s| s.id == live_id!(morph_map)) && !read.contains("tex_morph_map");
            let fragment_slots = slots.len() - usize::from(vertex_only);
            assert!(slots.len() <= makepad_draw::DRAW_CALL_TEXTURE_SLOTS && fragment_slots <= 16 && read.len() <= 16,
                "tile scene uses {} fragment units, {fragment_slots} fragment slots and {} combined slots", read.len(), slots.len());
            println!("tile march: {} fragment units, {fragment_slots} fragment slots, {} combined slots", read.len(), slots.len());

            let sun = recipe.shader_object(vm, id!(DrawTracedSunTiles)).expect("tiled Sun shader");
            crate::shaders::inspect_draw_backends(vm, sun, "tile Sun", Some(crate::shaders::TracedOutput::Sun));
            let mut draw = DrawTracedSun::script_new(vm);
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), sun.into());
            let id = draw.draw_vars.draw_shader_id.expect("tiled Sun binding layout");
            let slots = &vm.cx().draw_shaders[id.index].mapping.textures;
            assert!(matches!(slots.as_slice(), [volume] if volume.id == live_id!(trace_volume)),
                "the only Sun texture slot must be the volume");
        });
    }

    const EVERY_HOOK: &str = r#"
        vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p + n * (0.05 * sin(p.y * 8.0 + self.mat_vtime())) }
        surface: fn(base: vec4) -> vec4 { return vec4(base.xyz * vec3(1.0, 0.5, 0.2), base.w) }
        emission: fn(e: vec3) -> vec3 { return e + vec3(self.params.x, 0.0, 0.0) }
        normal: fn(n: vec3) -> vec3 { return normalize(n + vec3(0.0, 0.1, 0.0)) }
        metal_rough: fn(mr: vec2) -> vec2 { return vec2(mr.x, clamp(mr.y * 0.5, 0.05, 1.0)) }
        light: fn(radiance: vec3, l: vec3, n: vec3, v: vec3, brdf: vec3) -> vec3 { return radiance * step(0.3, dot(n, l)) }
        lighting: fn(direct: vec3, ambient: vec3) -> vec3 { return direct * 0.8 + ambient }
        finish: fn(c: vec4) -> vec4 { return vec4(c.xyz + 0.02 * self.mat_geo_normal() * fract(self.mat_pos().x + self.mat_uv().x), c.w) }
        bounds_pad: 0.05
    "#;

    #[test]
    fn every_hook_compiles_on_every_backend_with_its_variants() {
        with_vm(|vm| {
            let obj = spec(vm, "test://every.splash", EVERY_HOOK);
            let hooks = HookSet::from_spec(vm, obj, HookMask::ALL).unwrap();
            assert_eq!(hooks.bounds_pad, 0.05);
            for h in [Hook::Vertex, Hook::Surface, Hook::Emission, Hook::Normal, Hook::MetalRough, Hook::Light, Hook::Lighting, Hook::Finish] {
                assert!(hooks.has(h), "{h:?}");
            }
            let desc = MaterialDesc { ibl: true, blend: Blend::Mask { cutoff: 0.4 }, ..Default::default() };
            let m = DrawSceneCustom::build(vm, &desc, &hooks, HookMask::ALL, vec4(0.5, 0.0, 0.0, 0.0)).unwrap_or_else(|e| panic!("{e}"));
            assert!(m.draw.draw_vars.can_instance());
            assert_eq!(m.surface_alpha, SurfaceAlpha::Base);
            assert_eq!(m.plan.builtins, vec![Builtin::HookedCompose, Builtin::Ibl]);
            assert_eq!(m.blend, Blend::Mask { cutoff: 0.4 });
            assert!(m.ibl && !m.cuts_no_pixel(vm.cx()), "a masked surface hook can cut pixels");
            assert!(m.shadow.is_some(), "displaced, masked caster: {:?}", m.warnings);
            assert_eq!((m.cutoff, m.alpha_mode(vm.cx()), m.bounds_pad), (0.4, 1.0, 0.05));
            let program = m.program.as_ref().expect("hooked PBR recipe").as_object();
            for features in [
                Features::default(),
                Features { csm: true, cluster: true, local_shadows: true, gi: true, reflections: true, ..Default::default() },
            ] {
                let features = m.variant_key(features);
                let obj = variants::variant_object(vm, program, features, m.lit()).unwrap_or(program);
                let label = format!("hooked PBR {features:?}");
                crate::shaders::check_scene_backends(vm, obj, &label);
                crate::shaders::compile_scene_webgl2(vm, obj, &label);
            }
            for (body, expected) in [
                ("base", SurfaceAlpha::Base),
                ("vec4(base.xyz, 1.0)", SurfaceAlpha::One),
                ("vec4(base.xyz, fract(self.world.x))", SurfaceAlpha::Unproven),
            ] {
                let code = format!("surface: fn(base: vec4) -> vec4 {{ return {body} }}");
                let obj = spec(vm, "test://surface-alpha.splash", &code);
                let hooks = HookSet::from_spec(vm, obj, HookMask::ALBEDO).unwrap();
                let program = DrawSceneCustom::build(
                    vm, &MaterialDesc::default(), &hooks, HookMask::ALBEDO, Vec4f::default(),
                ).unwrap();
                assert_eq!(program.surface_alpha, expected, "{body}");
            }
            assert!(vm.take_errors().is_empty());
        });
    }

    /// The lowered source of a program object, for GLSL or HLSL.
    fn lowered(vm: &mut ScriptVm, program: ScriptObject, glsl: bool) -> String {
        let draw = vm.value(vm.modules(), id!(draw)).as_object().unwrap();
        vm.set_value(draw, id!(program_under_test), program);
        let source = if glsl {
            script_eval!(vm, { mod.shader.test_compile_draw_source(mod.draw.program_under_test, "glsl", false) })
        } else {
            script_eval!(vm, { mod.shader.test_compile_draw_source(mod.draw.program_under_test, "hlsl", false) })
        };
        crate::shaders::test_source(&vm.heap().string_with(source, |_, s| s.to_string()).unwrap_or_default())
    }

    /// An IBL program (the stock IBL program and the built-in item IBL
    /// material are this one: the PBR lane, IBL, no hooks) takes the
    /// reflection and the fill through the lane's control: both lookups x
    /// `ibl_ctl.y` (the lane's scale, so the lookups carry the exposure the
    /// rig's fill carries), and the fill from the map only while
    /// `ibl_ctl.z` is set, else the fill the lane passes in (an authored
    /// ambient). Never written, the control is neutral:
    /// the raw lookups. The stock PBR lane's own program has no control.
    #[test]
    fn an_ibl_program_takes_its_lookups_through_the_lanes_control() {
        use crate::shaders::lowered_fn_body;
        with_vm(|vm| {
            let m = DrawSceneCustom::build(vm, &MaterialDesc { ibl: true, ..Default::default() }, &HookSet::new(), HookMask::ALL, Vec4f::default()).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(m.plan.builtins, vec![Builtin::Ibl]);
            assert_eq!(m.draw.pbr.skinned.uniform_value(vm.cx(), live_id!(ibl_ctl)), [1.0, 1.0, 1.0, 0.0], "never written: the map's own scale, the fill from the map");
            let program = m.program.as_ref().unwrap().as_object();
            for glsl in [true, false] {
                let source = lowered(vm, program, glsl);
                assert!(!source.is_empty() && !source.starts_with("ERRORS:"), "glsl {glsl}: {source}");
                // `sky_env_lit` (the reflection of the surface and of the
                // clear coat, beside the key's lobe) and `mat_ambient` (the
                // fill) are the lane functions.
                let reflection = lowered_fn_body(&source, "sky_env_lit").unwrap_or_else(|| panic!("glsl {glsl}: no sky_env_lit"));
                assert!(reflection.contains("mat_ibl_lookup(p_r, p_rough)") && reflection.contains("ibl_ctl.y"), "glsl {glsl}: the reflection carries the lane's scale: {reflection}");
                assert!(reflection.contains("key_spec.y") && reflection.contains("p_vis"), "glsl {glsl}: the reflection takes the key's share out by the lobe's visibility, only with a Map key: {reflection}");
                assert!(reflection.contains("key_tint"), "glsl {glsl}: the reflection uses the key's colour and measured fit: {reflection}");
                assert!(reflection.contains("l_e.xyz - ((l_d * l_lum) * l_kk)"), "glsl {glsl}: the removal is the exact-at-zero form e - (d L) kk: {reflection}");
                let fill = lowered_fn_body(&source, "mat_ambient").unwrap_or_else(|| panic!("glsl {glsl}: no mat_ambient"));
                assert!(fill.contains("mat_ibl_ambient(p_n, p_a) * ") && fill.contains("ibl_ctl.y"), "glsl {glsl}: the fill carries the lane's scale: {fill}");
                assert!(fill.contains("ibl_ctl.z > 0.5") && fill.contains("return p_a;"), "glsl {glsl}: the fill is the map's only while the switch is set, else the lane's own: {fill}");
                // Nothing in the lit surface or the coat reads the map past them.
                for stage in ["shade_mr_surface_at", "clear_coat"] {
                    let body = lowered_fn_body(&source, stage).unwrap_or_else(|| panic!("glsl {glsl}: no {stage}"));
                    assert!(!body.contains("mat_ibl_"), "glsl {glsl}: {stage} takes a lookup at the map's raw scale");
                }
            }
            // No environment, no change: the stock PBR lane's program has
            // neither the control nor a lookup.
            let stock = script_eval!(vm, { mod.shader.test_compile_draw_source(mod.draw.DrawScenePbr, "glsl", false) });
            let stock = vm.heap().string_with(stock, |_, s| s.to_string()).unwrap_or_default();
            assert!(!stock.is_empty() && !stock.starts_with("ERRORS:"));
            assert!(!stock.contains("ibl_ctl") && !stock.contains("mat_ibl_"), "the stock PBR lane's program is as it was");
            assert!(vm.take_errors().is_empty());
        });
    }

    /// Car paint's clear coat mirrors `sky_env` and, on the stock PBR lane,
    /// paints a treeline band along its horizon (`coat_treeline`: the
    /// analytic sky has none). An IBL program mirrors the map, which has a
    /// horizon of its own: there `coat_treeline` is the identity, so no
    /// analytic ridge covers the map's reflection, and the rest of the
    /// coat is the stock coat line for line. The stock lane's band is the
    /// one it was, statement for statement (no environment, no change).
    #[test]
    fn an_ibl_programs_coat_mirrors_the_maps_horizon_and_the_stock_coat_keeps_its_treeline() {
        use crate::shaders::lowered_fn_body;
        with_vm(|vm| {
            let m = DrawSceneCustom::build(vm, &MaterialDesc { ibl: true, ..Default::default() }, &HookSet::new(), HookMask::ALL, Vec4f::default()).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(m.plan.builtins, vec![Builtin::Ibl]);
            let ibl_program = m.program.as_ref().unwrap().as_object();
            let stock_program = script_eval!(vm, { mod.draw.DrawScenePbr }).as_object().unwrap();
            // Both programs lower for every backend.
            for backend in [ShaderBackend::Metal, ShaderBackend::Hlsl, ShaderBackend::Glsl, ShaderBackend::Wgsl] {
                for (name, program) in [("the IBL program", ibl_program), ("the stock PBR program", stock_program)] {
                    let errors = frontend_errors_for(vm, program, backend);
                    assert!(errors.is_empty(), "{name}, {backend:?}: {errors:?}");
                }
            }
            for glsl in [true, false] {
                let (ibl, stock) = (lowered(vm, ibl_program, glsl), lowered(vm, stock_program, glsl));
                for (name, source) in [("the IBL program", &ibl), ("the stock PBR program", &stock)] {
                    assert!(!source.is_empty() && !source.starts_with("ERRORS:"), "glsl {glsl}, {name}: {source}");
                }
                let body = |source: &str, name: &str, function: &str| lowered_fn_body(source, function).unwrap_or_else(|| panic!("glsl {glsl}: {name} has no {function}"));
                // Under a map: no noise ridge and no analytic horizon
                // colour in the coat, and the function it calls for car
                // paint returns the reflection as it took it.
                let coat = body(&ibl, "the IBL program", "clear_coat");
                assert!(!coat.contains("tn_noise(") && !coat.contains("sun_ground"), "glsl {glsl}: an IBL program's coat paints the analytic treeline over the map's reflection: {coat}");
                assert_eq!(body(&ibl, "the IBL program", "coat_treeline").trim(), "return p_env;", "glsl {glsl}: under a map the treeline is the identity");
                // The stock lane: the band it had, every constant of it,
                // over the reflection it is handed.
                let band = body(&stock, "the stock PBR program", "coat_treeline");
                // The tree's share, splat to the colour's three channels.
                let splat = if glsl { "fog_color * 0.12)), vec3((l_tree * 0.85)));" } else { "fog_color * 0.12)), ((float3)((l_tree * 0.85))));" };
                for part in [
                    "(p_r.z, p_r.x);",
                    "((0.035 + (0.05 * ", "((l_az * 9", "), 0.5)))) + (0.03 * ", "((l_az * 37", "), 1.5))));",
                    "((1.0 - smoothstep((l_ridge - 0.01), (l_ridge + 0.01), p_r.y)) * smoothstep(-0.02, 0.0, p_r.y));",
                    "(p_env, ((", "sun_ground * 0.28) + (", splat,
                ] {
                    assert!(band.contains(part), "glsl {glsl}: the stock lane's treeline is the band it was, `{part}`: {band}");
                }
                assert_eq!((band.matches("tn_noise(").count(), band.trim().lines().count()), (2, 4), "glsl {glsl}: two noise taps, four statements: {band}");
                // Car paint only (a toy's rim skips it), and the IBL
                // program's coat is this coat: only the function differs.
                let stock_coat = body(&stock, "the stock PBR program", "clear_coat");
                // GLSL and HLSL name a shader function alike (`io_<name>`).
                let call = "if ((l_rim < 0.5)) {\nl_env = io_coat_treeline(l_env, l_r);\n}\n";
                assert!(stock_coat.contains(call) && !stock_coat.contains("tn_noise("), "glsl {glsl}: the coat paints the band through the function, on car paint: {stock_coat}");
                assert_eq!(coat, stock_coat, "glsl {glsl}: an IBL program's coat is the stock coat");
            }
            assert!(vm.take_errors().is_empty());
        });
    }

    #[test]
    fn a_plain_opaque_material_gets_a_discard_free_variant_with_the_stock_layout() {
        with_vm(|vm| {
            let obj = spec(vm, "test://finish.splash", "finish: fn(c: vec4) -> vec4 { return vec4(c.xyz * 0.5, c.w) }");
            let hooks = HookSet::from_spec(vm, obj, HookMask::ALL).unwrap();
            let m = DrawSceneCustom::build(vm, &MaterialDesc::default(), &hooks, HookMask::ALL, Vec4f::default()).unwrap();
            assert!(m.cuts_no_pixel(vm.cx()), "no hook can cut");
            let stock = m.stock_shader().unwrap();
            let program = m.program.as_ref().unwrap().as_object();
            // Every feature on but `clip`, and every feature off: each has
            // the stock layout (build_variant checks it).
            let all = Features { clip: false, gi: true, gi_screen: true, gi_debug: true, csm: true, settled: false, csm_debug: true, cluster: true, local_shadows: true, reflections: false };
            for features in [all, Features { settled: true, ..all }, Features::default()] {
                let variant = build_variant(vm, program, stock, features, true).ok().flatten().expect("the stock layout");
                assert_ne!(stock, variant);
            }
            assert_eq!(m.shadow, None, "no displacement, no mask: the stock caster");
            assert!(m.plan.builtins.is_empty(), "no lighting hook: the stock composition");
        });
    }

    #[test]
    fn masks_arity_and_engine_owned_keys_are_refused_before_compiling() {
        with_vm(|vm| {
            let obj = spec(vm, "test://v.splash", "vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p }");
            let e = HookSet::from_spec(vm, obj, HookMask::ALBEDO).unwrap_err();
            assert!(e.to_string().contains("does not accept the `vertex` hook"), "{e}");
            let obj = spec(vm, "test://p.splash", "pixel: fn() { return vec4(1.0) }");
            let e = HookSet::from_spec(vm, obj, HookMask::ALL).unwrap_err();
            assert!(e.to_string().contains("`pixel` is engine-owned"), "{e}");
            let obj = spec(vm, "test://a.splash", "finish: fn(c: vec4, extra: float) -> vec4 { return c }");
            let e = HookSet::from_spec(vm, obj, HookMask::ALL).unwrap_err();
            assert!(e.to_string().contains("takes 1 parameter"), "{e}");
            let obj = spec(vm, "test://u.splash", "lighting: fn(direct: vec3, ambient: vec3) -> vec3 { return direct }");
            let hooks = HookSet::from_spec(vm, obj, HookMask::ALL).unwrap();
            let desc = MaterialDesc { lighting: LightingModel::Unlit, ..Default::default() };
            assert!(DrawSceneCustom::build(vm, &desc, &hooks, HookMask::ALL, Vec4f::default()).is_err());
            let desc = MaterialDesc { blend: Blend::Add, ..Default::default() };
            assert!(DrawSceneCustom::build(vm, &desc, &HookSet::new(), HookMask::ALL, Vec4f::default()).is_err());
        });
    }

    #[test]
    fn compile_errors_name_the_hook_and_the_document_line() {
        with_vm(|vm| {
            let obj = spec(vm, "test://broken.splash", "surface: fn(base: vec4) -> vec4 { return base }\nfinish: fn(c: vec4) -> vec4 {\n    return no_such_function(c)\n}");
            let hooks = HookSet::from_spec(vm, obj, HookMask::ALL).unwrap();
            let e = match DrawSceneCustom::build(vm, &MaterialDesc::default(), &hooks, HookMask::ALL, Vec4f::default()) {
                Err(e) => e,
                Ok(_) => panic!("a call to an undefined function must not build"),
            };
            let d = e.diagnostics.iter().find(|d| d.file.is_some()).unwrap_or_else(|| panic!("no located diagnostic: {e}"));
            assert_eq!(d.file.as_deref(), Some("test://broken.splash"), "{e}");
            assert_eq!(d.line, 5, "the call's line (the spec opens with two wrapper lines): {e}");
            assert_eq!(d.hook, Some(Hook::Finish), "{e}");
            let _ = vm.take_errors();
        });
    }

    #[test]
    fn unlit_error_and_albedo_materials_build() {
        with_vm(|vm| {
            let unlit = DrawSceneCustom::unlit(vm, &HookSet::new(), HookMask::ALL, 2.0).unwrap();
            assert_eq!(unlit.plan.builtins, vec![Builtin::Unlit]);
            assert_eq!(unlit.draw.params(vm.cx()).w, 2.0);
            let error = DrawSceneCustom::error_material(vm).unwrap();
            assert!(error.draw.draw_vars.can_instance());
            assert_ne!(error.draw.draw_vars.draw_shader_id, unlit.draw.draw_vars.draw_shader_id);
            let obj = spec(vm, "test://albedo.splash", "surface: fn(base: vec4) -> vec4 { return vec4(base.xyz * 0.5, base.w) }");
            let surface = HookSet::from_spec(vm, obj, HookMask::ALBEDO).unwrap().get(Hook::Surface).unwrap();
            let albedo = DrawSceneCustom::from_surface(vm, surface, vec4(1.0, 2.0, 3.0, 4.0)).unwrap();
            assert_eq!(albedo.draw.params(vm.cx()), vec4(1.0, 2.0, 3.0, 4.0));
            let stock = DrawSceneCustom::script_new_with_default(vm).draw_vars.draw_shader_id;
            assert_ne!(albedo.draw.draw_vars.draw_shader_id, stock);
            // The unlit programs' variants drop the lighting with the
            // features; the lit one keeps it.
            assert!(!unlit.lit() && !error.lit() && albedo.lit());
            for m in [&unlit, &error] {
                let program = m.program.as_ref().unwrap().as_object();
                let lit_on = Features { clip: true, csm: true, cluster: true, local_shadows: true, ..Default::default() };
                assert!(build_variant(vm, program, m.stock_shader().unwrap(), lit_on, m.lit()).ok().flatten().is_some_and(|v| Some(v) != m.stock_shader()));
            }
            assert!(vm.take_errors().is_empty());
        });
    }
}
