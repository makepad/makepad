//! Opt-in procedural model materials. Stock models pay no additional shader cost.
use makepad_draw::*;
use crate::shaders::DrawScenePbr;

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCustom {
    #[deref] pub pbr: DrawScenePbr,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))] pub params: Vec4f,
}

// The engine owns vertex, lighting and fog: the whole DrawScenePbr pixel
// (GGX sun lobe, sky reflection, clustered-light specular, emission) runs
// unchanged, and only its albedo `surface` hook is replaced. A recoloured
// car therefore keeps the clear-coat response of its stock material. Only
// the albedo hook is taken from a game's spec; arbitrary pixel/vertex
// overrides are not applied.
script_mod! {
    use mod.prelude.widgets_internal.*
    mod.draw.DrawSceneCustom = mod.std.set_type_default() do #(DrawSceneCustom::script_shader(vm)) {
        ..mod.draw.DrawScenePbr
        surface: fn(base: vec4) -> vec4 { return base }
    }
}

/// Install shader declarations in the owning isolate. Registration grants
/// no IO or host-widget access. GameMaterials only copies its allowlisted hook.
pub fn register(vm: &mut ScriptVm) -> ScriptValue {
    if vm.bx.heap.type_default_for_id(DrawSceneCustom::script_type_id_static()).is_some() {
        return NIL;
    }
    if vm.bx.heap.type_default_for_id(DrawScenePbr::script_type_id_static()).is_none() {
        crate::local_shadows::sampling::script_mod(vm);
        crate::clustered::script_mod(vm);
        crate::fast_gi::script_mod(vm);
        crate::shaders::script_mod(vm);
        crate::local_shadows::script_mod(vm);
    }
    script_mod(vm)
}

impl DrawSceneCustom {
    /// Construct a fresh allowlisted surface override. Source edits reuse a
    /// ScriptIp (body + opcode offset), while the draw function cache hashes
    /// only those locations. Invalidate this object's function-hash shortcut
    /// so a new body is frontend-checked; generated-code caching still safely
    /// shares identical shaders. Call only when declarations change, not for
    /// per-frame parameter updates.
    pub fn from_surface(vm: &mut ScriptVm, surface: ScriptObject, params: Vec4f) -> Result<Self, String> {
        if !vm.bx.heap.is_fn(surface) { return Err("surface is not a shader function".into()); }
        register(vm);
        let base = vm.bx.heap.type_default_for_id(Self::script_type_id_static())
            .ok_or_else(|| "custom material shader registration failed".to_string())?;
        let obj = vm.bx.heap.new_with_proto_no_vec(base.into());
        let result = vm.bx.heap.set_value(obj, id!(surface).into(), surface.into(), NoTrap);
        if !result.is_nil() || vm.bx.heap.object_method(obj, id!(surface).into(), NoTrap).as_object() != Some(surface) {
            return Err("custom material surface override was not installed".into());
        }
        let hash = DrawVars::compute_shader_functions_hash(&vm.bx.heap, obj);
        vm.host.cx_mut().draw_shaders.cache_functions_to_shader.remove(&hash);
        // #[deref] construction applies DrawScenePbr's defaults and can
        // already have compiled its stock shader. A failed override leaves
        // an existing ID untouched, so clear it before testing this candidate.
        let mut draw = Self::script_new(vm);
        draw.draw_vars.draw_shader_id = None;
        draw.draw_vars.geometry_id = None;
        draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
        draw.params = params;
        if !draw.draw_vars.can_instance() {
            return Err("surface shader failed frontend compilation; see shader diagnostic in app log".into());
        }
        Ok(draw)
    }
}
