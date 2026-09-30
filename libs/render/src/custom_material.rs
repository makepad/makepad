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
use makepad_draw::makepad_platform::makepad_script::script_eval;
use makepad_render_material::{self as material, builtin, Builtin, HookMask, HookSet, MaterialDesc, MaterialError, ShadowVariant, VariantPlan};
use makepad_scene::{BaseKind, Blend};
use crate::shaders::{DrawLmSunDepthCutout, DrawScenePbr};

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCustom {
    #[deref] pub pbr: DrawScenePbr,
    /// The material's four keyable floats (`self.params` in the hooks).
    /// The lane's instance stream is at the 31-attribute limit, so this is
    /// the whole per-draw budget; more data rides textures.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))] pub params: Vec4f,
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
            let texel = self.tex.sample_as_bgra_repeat(self.v_uv)
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

    // `clip` emptied: the discard-free colour variant (see opaque.rs).
    mod.draw.mat_clip_none = fn() {}

}

/// Install shader declarations in the owning isolate. Registration grants
/// no IO or host-widget access.
pub fn register(vm: &mut ScriptVm) -> ScriptValue {
    if vm.bx.heap.type_default_for_id(DrawSceneCustom::script_type_id_static()).is_some() {
        builtin::register(vm);
        return NIL;
    }
    if vm.bx.heap.type_default_for_id(DrawScenePbr::script_type_id_static()).is_none() {
        crate::local_shadows::sampling::script_mod(vm);
        crate::clustered::script_mod(vm);
        crate::fast_gi::script_mod(vm);
        crate::shaders::script_mod(vm);
        crate::local_shadows::script_mod(vm);
    }
    builtin::register(vm);
    script_mod(vm)
}

/// A built material: the colour draw, and what its plan derived.
pub struct CustomMaterial {
    pub draw: DrawSceneCustom,
    pub plan: VariantPlan,
    /// The colour program without `clip`, for draws that cut no pixel
    /// (only built when no hook can discard; see opaque.rs).
    pub opaque_variant: Option<DrawShaderId>,
    /// The derived caster (`ShadowVariant::Derived`), when it compiled.
    pub shadow: Option<DrawShaderId>,
    pub cutoff: f32,
    /// How far the vertex hook may move geometry (culling grows by it).
    pub bounds_pad: f32,
    /// The program samples the IBL texture (bind it on `detail_map`).
    pub ibl: bool,
    /// Problems that did not stop the colour program (a caster that did not
    /// compile casts the stock shadow instead).
    pub warnings: Vec<material::Diagnostic>,
}

impl From<DrawSceneCustom> for CustomMaterial {
    /// A colour draw built elsewhere (Sandbox's albedo surfaces): the stock
    /// caster, no derived variants.
    fn from(draw: DrawSceneCustom) -> Self {
        Self {
            draw,
            plan: VariantPlan { builtins: Vec::new(), can_discard: true, shadow: ShadowVariant::Stock },
            opaque_variant: None,
            shadow: None,
            cutoff: 0.0,
            bounds_pad: 0.0,
            ibl: false,
            warnings: Vec::new(),
        }
    }
}

/// Build one program object on the lane and instance a draw struct from it.
fn instance_custom(vm: &mut ScriptVm, obj: ScriptObject) -> Result<DrawSceneCustom, MaterialError> {
    // Source edits reuse a ScriptIp (body + opcode offset), while the draw
    // function cache hashes only those locations: forget this object's
    // shortcut so a new body is compiled. Generated-code caching still
    // shares identical shaders.
    let hash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, obj);
    vm.host.cx_mut().draw_shaders.cache_functions_to_shader.remove(&hash);
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

fn with_override(vm: &mut ScriptVm, obj: ScriptObject, method: LiveId, function: ScriptValue) -> Option<ScriptObject> {
    let function = function.as_object().filter(|f| vm.bx.heap.is_fn(*f))?;
    let variant = vm.bx.heap.new_with_proto_no_vec(obj.into());
    vm.bx.heap.set_value(variant, method.into(), function.into(), NoTrap).is_nil().then_some(variant)
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
        draw.params = params;
        let cutoff = match desc.blend {
            Blend::Mask { cutoff } => cutoff,
            _ => 0.0,
        };
        match desc.blend {
            Blend::Mask { cutoff } => {
                draw.pbr.alpha_mode = 1.0;
                draw.pbr.alpha_cutoff = cutoff;
            }
            Blend::Over => {
                draw.pbr.alpha_mode = 2.0;
                draw.draw_vars.options.alpha_blend = true;
                draw.draw_vars.options.depth_write = false;
            }
            _ => {}
        }
        let mut warnings = Vec::new();
        let opaque_variant = if plan.can_discard || desc.blend != Blend::Opaque {
            None
        } else {
            let clip = script_eval!(vm, { mod.draw.mat_clip_none });
            with_override(vm, obj, id!(clip), clip).and_then(|v| instance_custom(vm, v).ok()).and_then(|d| d.draw_vars.draw_shader_id)
        };
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
        Ok(CustomMaterial {
            draw,
            ibl: plan.builtins.contains(&Builtin::Ibl),
            plan,
            opaque_variant,
            shadow,
            cutoff,
            bounds_pad: hooks.bounds_pad,
            warnings,
        })
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
        let hash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, obj);
        vm.host.cx_mut().draw_shaders.cache_functions_to_shader.remove(&hash);
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
        Ok(CustomMaterial { plan: VariantPlan { builtins: vec![Builtin::Error], can_discard: false, shadow: ShadowVariant::Stock }, ..CustomMaterial::from(draw) })
    }

    /// Sandbox's game.material: one albedo `surface(base: vec4) -> vec4`.
    pub fn from_surface(vm: &mut ScriptVm, surface: ScriptObject, params: Vec4f) -> Result<Self, String> {
        if !vm.bx.heap.is_fn(surface) { return Err("surface is not a shader function".into()); }
        let hooks = HookSet::new().with(material::Hook::Surface, surface);
        Self::build(vm, &MaterialDesc::default(), &hooks, HookMask::ALBEDO, params)
            .map(|m| m.draw)
            .map_err(|e| format!("surface shader failed frontend compilation: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert!(m.ibl && m.opaque_variant.is_none(), "a surface hook can cut pixels");
            assert!(m.shadow.is_some(), "displaced, masked caster: {:?}", m.warnings);
            assert_eq!((m.cutoff, m.draw.pbr.alpha_mode, m.bounds_pad), (0.4, 1.0, 0.05));
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
            let (stock, variant) = (m.draw.draw_vars.draw_shader_id.unwrap(), m.opaque_variant.expect("no hook can cut"));
            assert_ne!(stock, variant);
            let cx = vm.cx();
            let (a, b) = (&cx.draw_shaders[stock.index].mapping, &cx.draw_shaders[variant.index].mapping);
            assert_eq!(a.instances.total_slots, b.instances.total_slots);
            assert_eq!(a.dyn_uniforms.total_slots, b.dyn_uniforms.total_slots);
            assert!(a.textures.iter().map(|t| t.id).eq(b.textures.iter().map(|t| t.id)));
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
            assert_eq!(unlit.draw.params.w, 2.0);
            let error = DrawSceneCustom::error_material(vm).unwrap();
            assert!(error.draw.draw_vars.can_instance());
            assert_ne!(error.draw.draw_vars.draw_shader_id, unlit.draw.draw_vars.draw_shader_id);
            let obj = spec(vm, "test://albedo.splash", "surface: fn(base: vec4) -> vec4 { return vec4(base.xyz * 0.5, base.w) }");
            let surface = HookSet::from_spec(vm, obj, HookMask::ALBEDO).unwrap().get(Hook::Surface).unwrap();
            let draw = DrawSceneCustom::from_surface(vm, surface, vec4(1.0, 2.0, 3.0, 4.0)).unwrap();
            assert_eq!(draw.params, vec4(1.0, 2.0, 3.0, 4.0));
            let stock = DrawSceneCustom::script_new_with_default(vm).draw_vars.draw_shader_id;
            assert_ne!(draw.draw_vars.draw_shader_id, stock);
            assert!(vm.take_errors().is_empty());
        });
    }
}
