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
use makepad_render_material::{self as material, builtin, Builtin, HookMask, HookSet, MaterialDesc, MaterialError, ShadowVariant, VariantPlan};
use makepad_scene::{BaseKind, Blend};
use crate::renderer::variants::{self, Features};
use crate::shaders::{DrawLmSunDepthCutout, DrawScenePbr};
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
    mod.draw.DrawSceneCustom = mod.std.set_type_default() do #(DrawSceneCustom::script_shader(vm)) {
        ..mod.draw.DrawScenePbr
        // The material's four keyable floats (`self.params` in the hooks),
        // the whole per-draw budget; more data rides textures. A uniform: on
        // the instance stream they took the lane one output register past
        // what D3D11's `vs_5_0` allows, and they are one per material draw.
        params: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        surface: fn(base: vec4) -> vec4 { return base }
        // What a hook may read besides its arguments: the clock, the true
        // world position, the mesh uv and the geometric normal.
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
    if vm.bx.heap.type_default_for_id(DrawSceneCustom::script_type_id_static()).is_some() {
        builtin::register(vm);
        return NIL;
    }
    // The scene shaders exactly as `crate::script_mod` registers them (the
    // shared stdlib, the shadow sampling this build uses, ...): a second,
    // partial list here made different shaders (and shader-pack keys)
    // depending on which registration ran first.
    if vm.bx.heap.type_default_for_id(DrawScenePbr::script_type_id_static()).is_none() {
        crate::script_mod(vm);
    }
    builtin::register(vm);
    script_mod(vm)
}

/// Runs `f` in the VM that owns a material's program (a level isolate's);
/// false when that VM is busy. A material without one uses the Cx's VM.
pub type MaterialVm = Rc<dyn Fn(&mut Cx, &mut dyn FnMut(&mut ScriptVm)) -> bool>;

/// A built material: the colour draw, and what its plan derived.
pub struct CustomMaterial {
    pub draw: DrawSceneCustom,
    pub plan: VariantPlan,
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
}

impl CustomMaterial {
    fn new(draw: DrawSceneCustom, plan: VariantPlan, program: Option<ScriptObjectRef>) -> Self {
        Self {
            stock: draw.draw_vars.draw_shader_id,
            draw,
            plan,
            program,
            host_vm: None,
            variants: HashMap::new(),
            shadow: None,
            cutoff: 0.0,
            bounds_pad: 0.0,
            ibl: false,
            texture: None,
            warnings: Vec::new(),
        }
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
        if std::env::var_os("MAKEPAD_SHADER_VARIANTS").is_some_and(|v| v == "0") {
            return (Some(stock), None);
        }
        let features = Features { clip: true, ..features };
        if !self.variants.contains_key(&features) {
            let Some(program) = self.program.clone() else { return (Some(stock), None) };
            let (lit, opaque) = (self.lit(), self.cuts_no_pixel(cx) && std::env::var_os("MAKEPAD_OPAQUE_VARIANT").is_none_or(|v| v != "0"));
            let mut built = None;
            let mut run = |vm: &mut ScriptVm| {
                // Only against the heap that owns the program.
                if vm.bx.heap.heap_key() != program.heap_key() { built = Some(None); return; }
                let full = build_variant(vm, program.as_object(), stock, features, lit);
                let no_clip = if opaque { build_variant(vm, program.as_object(), stock, Features { clip: false, ..features }, lit) } else { None };
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
        }
        match self.variants[&features] {
            Some((full, no_clip)) => (Some(full), no_clip),
            None => (Some(stock), None),
        }
    }
}

impl From<DrawSceneCustom> for CustomMaterial {
    /// A colour draw built elsewhere: the stock caster, no derived
    /// variants.
    fn from(draw: DrawSceneCustom) -> Self {
        Self::new(draw, VariantPlan { builtins: Vec::new(), can_discard: true, shadow: ShadowVariant::Stock }, None)
    }
}

/// `program`'s variant for `features` (the stock shader when it replaces
/// nothing), `None` when it does not build or its layout differs.
fn build_variant(vm: &mut ScriptVm, program: ScriptObject, stock: DrawShaderId, features: Features, lit: bool) -> Option<DrawShaderId> {
    let Some(obj) = variants::variant_object(vm, program, features, lit) else { return Some(stock) };
    let variant = instance_custom(vm, obj).ok()?.draw_vars.draw_shader_id?;
    if !variants::same_layout(vm.cx(), stock, variant) {
        log!("render: a material's variant {features:?} has another layout than its shader; drawing through that");
        return None;
    }
    Some(variant)
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
        let detail = makepad_draw::makepad_platform::shader_error::take().unwrap_or_else(|| "see the shader diagnostic in the app log".into());
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
        let base = vm.bx.heap.type_default_for_id(Self::script_type_id_static())
            .ok_or_else(|| MaterialError::new("custom material shader registration failed"))?;
        let mut overrides = Vec::new();
        for (name, f) in helpers {
            let existing = vm.bx.heap.object_method(base, (*name).into(), NoTrap);
            let taken = !(existing.is_nil() || existing.is_err())
                || material::HOOKS.iter().any(|h| LiveId::from_str(h.method) == *name);
            if taken || !vm.bx.heap.is_fn(*f) {
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
        let program = vm.bx.heap.new_object_ref(obj);
        Ok(CustomMaterial { ibl, shadow, cutoff, bounds_pad: hooks.bounds_pad, warnings, ..CustomMaterial::new(draw, plan, Some(program)) })
    }

    fn build_shadow(vm: &mut ScriptVm, hooks: &HookSet, vertex: bool, mask: bool, helpers: &[(LiveId, ScriptObject)]) -> Result<DrawShaderId, MaterialError> {
        let base = vm.bx.heap.type_default_for_id(DrawMaterialShadow::script_type_id_static())
            .ok_or_else(|| MaterialError::new("material shadow shader registration failed"))?;
        let mut caster_hooks = HookSet::new();
        // The author's helpers, where the caster has no function of that name.
        let mut overrides: Vec<(LiveId, ScriptObject)> = helpers.iter().copied().filter(|(name, _)| {
            let v = vm.bx.heap.object_method(base, (*name).into(), NoTrap);
            v.is_nil() || v.is_err()
        }).collect();
        if vertex {
            if let Some(f) = hooks.get(material::Hook::Vertex) { caster_hooks.set(material::Hook::Vertex, f); }
        }
        if mask {
            if let Some(f) = hooks.get(material::Hook::Surface) { caster_hooks.set(material::Hook::Surface, f); }
            if let Some(alpha) = vm.bx.heap.value(base, id!(mat_shadow_alpha_mask).into(), NoTrap).as_object() {
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
        let base = vm.bx.heap.type_default_for_id(Self::script_type_id_static())
            .ok_or_else(|| MaterialError::new("custom material shader registration failed"))?;
        let obj = material::build_program(vm, &material::ProgramRequest { base, kind: BaseKind::Unlit, hooks: &HookSet::new(), mask: HookMask::NONE, overrides: &overrides })?;
        let draw = instance_custom(vm, obj)?;
        let program = vm.bx.heap.new_object_ref(obj);
        Ok(CustomMaterial::new(draw, VariantPlan { builtins: vec![Builtin::Error], can_discard: false, shadow: ShadowVariant::Stock }, Some(program)))
    }

    /// Sandbox's game.material: one albedo `surface(base: vec4) -> vec4`
    /// (a host whose program lives in another VM than the Cx's sets the
    /// material's `host_vm`).
    pub fn from_surface(vm: &mut ScriptVm, surface: ScriptObject, params: Vec4f) -> Result<CustomMaterial, String> {
        if !vm.bx.heap.is_fn(surface) { return Err("surface is not a shader function".into()); }
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
            vm.bx.captured_errors = Some(Vec::new());
            makepad_draw::script_mod(vm);
            vm.bx.heap.new_module(id!(prelude));
            script_eval!(vm, { mod.prelude.widgets_internal = { ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw } });
            vm.bx.heap.new_module(id!(widgets));
            register(vm);
            f(vm);
        });
    }

    /// A material document's hooks, evaluated from Splash source under a
    /// file name (so errors can name its lines).
    fn spec(vm: &mut ScriptVm, file: &str, code: &str) -> ScriptObject {
        let code = format!("use mod.prelude.widgets_internal.*\n{{\n{code}\n}}");
        // Hosts evaluate documents with `line: 1`, so rows report 1-based.
        let value = vm.eval(ScriptMod { file: file.into(), code, line: 1, ..Default::default() });
        value.as_object().unwrap_or_else(|| panic!("spec did not evaluate: {:?}", vm.take_errors()))
    }

    const EVERY_HOOK: &str = r#"
        vertex: fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p + n * (0.05 * sin(p.y * 8.0 + self.mat_vtime())) }
        surface: fn(base: vec4) -> vec4 { return vec4(base.xyz * vec3(1.0, 0.5, 0.2), base.w) }
        emission: fn(e: vec3) -> vec3 { return e + vec3(self.params.x, 0.0, 0.0) }
        normal: fn(n: vec3) -> vec3 { return normalize(n + vec3(0.0, 0.1, 0.0)) }
        metal_rough: fn(mr: vec2) -> vec2 { return vec2(mr.x, clamp(mr.y * 0.5, 0.05, 1.0)) }
        light: fn(radiance: vec3, l: vec3, n: vec3, v: vec3, brdf: vec3) -> vec3 { return radiance * step(0.3, dot(n, l)) }
        lighting: fn(direct: vec3, ambient: vec3) -> vec3 { return direct * 0.8 + ambient }
        finish: fn(c: vec4) -> vec4 { return vec4(c.xyz + vec3(0.02, 0.0, 0.0) * fract(self.mat_pos().x), c.w) }
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
            assert_eq!(m.plan.builtins, vec![Builtin::HookedCompose, Builtin::Ibl]);
            assert!(m.ibl && !m.cuts_no_pixel(vm.cx()), "a surface hook can cut pixels");
            assert!(m.shadow.is_some(), "displaced, masked caster: {:?}", m.warnings);
            assert_eq!((m.cutoff, m.alpha_mode(vm.cx()), m.bounds_pad), (0.4, 1.0, 0.05));
            // The same program lowers for every backend, not only Metal.
            let base = vm.bx.heap.type_default_for_id(DrawSceneCustom::script_type_id_static()).unwrap();
            let mut overrides = builtin::overrides(vm, Builtin::HookedCompose);
            overrides.extend(builtin::overrides(vm, Builtin::Ibl));
            let prog = material::build_program(vm, &material::ProgramRequest { base, kind: BaseKind::Pbr, hooks: &hooks, mask: HookMask::ALL, overrides: &overrides }).unwrap();
            for backend in [ShaderBackend::Metal, ShaderBackend::Hlsl, ShaderBackend::Glsl, ShaderBackend::Wgsl] {
                let errors = frontend_errors_for(vm, prog, backend);
                assert!(errors.is_empty(), "{backend:?}: {errors:?}");
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
            let all = Features { clip: false, gi: true, gi_debug: true, csm: true, csm_debug: true, cluster: true, local_shadows: true };
            for features in [all, Features::default()] {
                let variant = build_variant(vm, program, stock, features, true).expect("the stock layout");
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
                assert!(build_variant(vm, program, m.stock_shader().unwrap(), lit_on, m.lit()).is_some_and(|v| Some(v) != m.stock_shader()));
            }
            assert!(vm.take_errors().is_empty());
        });
    }
}
