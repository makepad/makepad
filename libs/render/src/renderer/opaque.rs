//! Opaque model draws without `discard`.
//!
//! Both model lanes (DrawSceneSkinned and its DrawScenePbr sibling) cut
//! pixels in a few cases: an alpha-tested texel, the streamed-LOD dither,
//! the follow-camera occluder fade, fur shells and masked materials. Every
//! one of those goes through the shaders' `clip` hook. A pipeline that can
//! discard at all costs a tile GPU its hidden-surface removal for every
//! pixel it draws (Apple GPUs then shade each covered layer instead of only
//! the front one): on the Mac mini, the race's props cost 2.4 ms more at
//! 1580x1520 with the hook than without it.
//!
//! So each lane also has a variant with `clip` emptied, and a draw that can
//! never cut a pixel goes through it. The variant is the same draw struct
//! type with one function replaced, so its instance, uniform and texture
//! layout is the stock one (checked once), and the lane swaps only the
//! shader id around its `add_instance`.

use super::*;
use makepad_draw::makepad_platform::makepad_script::script_eval;

/// Which lane's shader a variant replaces.
#[derive(Clone, Copy)]
pub(super) enum OpaqueLane {
    Diffuse = 0,
    Pbr = 1,
    /// The streamed world's DrawSceneCity (stream_draw.rs).
    City = 2,
    /// Swaying trees and bushes (shaders/foliage.rs).
    Foliage = 3,
}

/// One lane's variant: not built yet, unavailable (the VM was busy, or the
/// layout differs), or its shader.
#[derive(Default, Clone, Copy)]
pub(super) enum OpaqueShader {
    #[default]
    Unbuilt,
    Unavailable,
    Built(DrawShaderId),
}

script_mod! {
    use mod.prelude.widgets_internal.*
    // The opaque variants' `clip` (see the module docs).
    mod.draw.scene_clip_opaque = fn() {}
}

impl Renderer {
    /// The lane's no-discard shader, once built and compiled for this
    /// frame's target; `None` draws through the stock shader.
    pub(super) fn opaque_shader(&mut self, cx: &mut Cx, lane: OpaqueLane) -> Option<DrawShaderId> {
        if std::env::var_os("MAKEPAD_OPAQUE_VARIANT").is_some_and(|v| v == "0") {
            return None;
        }
        let slot = &mut self.opaque_shaders[lane as usize];
        if matches!(slot, OpaqueShader::Unbuilt) {
            if let Some(built) = cx.try_with_vm(|vm| build(vm, lane)) {
                *slot = built.map_or(OpaqueShader::Unavailable, OpaqueShader::Built);
            }
        }
        match *slot {
            OpaqueShader::Built(id) if cx.draw_shader_ready(id, self.hdr_output) => Some(id),
            _ => None,
        }
    }
}

fn build(vm: &mut ScriptVm, lane: OpaqueLane) -> Option<DrawShaderId> {
    static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !REGISTERED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        script_mod(vm);
    }
    let clip = script_eval!(vm, { mod.draw.scene_clip_opaque });
    let clip = clip.as_object().filter(|f| vm.bx.heap.is_fn(*f))?;
    let type_id = match lane {
        OpaqueLane::Diffuse => DrawSceneSkinned::script_type_id_static(),
        OpaqueLane::Pbr => DrawScenePbr::script_type_id_static(),
        OpaqueLane::City => crate::shaders::DrawSceneCity::script_type_id_static(),
        OpaqueLane::Foliage => crate::shaders::DrawSceneFoliageLit::script_type_id_static(),
    };
    let base = vm.bx.heap.type_default_for_id(type_id)?;
    let obj = vm.bx.heap.new_with_proto_no_vec(base.into());
    if !vm.bx.heap.set_value(obj, id!(clip).into(), clip.into(), NoTrap).is_nil() {
        return None;
    }
    // As DrawSceneCustom::from_surface: construction applies the stock
    // defaults (and may compile the stock shader); clear it, then apply the
    // variant object.
    let (stock, variant) = match lane {
        OpaqueLane::Pbr => {
            let stock = DrawScenePbr::script_new_with_default(vm).skinned.draw_vars.draw_shader_id;
            let mut draw = DrawScenePbr::script_new(vm);
            draw.skinned.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            (stock, draw.skinned.draw_vars.draw_shader_id)
        }
        OpaqueLane::Diffuse => {
            let stock = DrawSceneSkinned::script_new_with_default(vm).draw_vars.draw_shader_id;
            let mut draw = DrawSceneSkinned::script_new(vm);
            draw.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            (stock, draw.draw_vars.draw_shader_id)
        }
        OpaqueLane::City => {
            let stock = crate::shaders::DrawSceneCity::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id;
            let mut draw = crate::shaders::DrawSceneCity::script_new(vm);
            draw.pbr.skinned.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            (stock, draw.pbr.skinned.draw_vars.draw_shader_id)
        }
        OpaqueLane::Foliage => {
            let stock = crate::shaders::DrawSceneFoliageLit::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id;
            let mut draw = crate::shaders::DrawSceneFoliageLit::script_new(vm);
            draw.pbr.skinned.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            (stock, draw.pbr.skinned.draw_vars.draw_shader_id)
        }
    };
    let (stock, variant) = (stock?, variant?);
    // The lane writes its uniforms and textures by the stock layout.
    let cx = vm.cx();
    let (a, b) = (&cx.draw_shaders[stock.index].mapping, &cx.draw_shaders[variant.index].mapping);
    let same = stock != variant
        && a.instances.total_slots == b.instances.total_slots
        && a.dyn_instances.total_slots == b.dyn_instances.total_slots
        && a.dyn_uniforms.total_slots == b.dyn_uniforms.total_slots
        && a.scope_uniforms.total_slots == b.scope_uniforms.total_slots
        && a.textures.iter().map(|t| t.id).eq(b.textures.iter().map(|t| t.id));
    if !same {
        log!("render: the {} opaque variant's layout differs from the stock shader; drawing every model through the stock one", ["diffuse", "PBR", "city", "foliage"][lane as usize]);
        return None;
    }
    Some(variant)
}

/// Whether the follow-camera occluder fade can reach a body at `center`
/// with bounding radius `radius`: the widest point of the shaders' cone
/// (0.55 m + 0.22 m per metre of eye-to-focus length, plus its 0.5 m
/// smoothstep) around the eye-to-focus segment, and the 1.2 m lens sphere.
pub(super) fn in_occluder_fade(eye: Vec3f, focus: Vec3f, center: Vec3f, radius: f32) -> bool {
    let of = focus - eye;
    let ol = of.length().max(0.001);
    let t = ((center - eye).dot(of) / ol).clamp(0.0, ol);
    let closest = eye + of * (t / ol);
    (center - closest).length() - radius < (1.05 + 0.22 * ol).max(1.2) + 0.1
}

/// A world-space bounding sphere of the local box `min..max` under
/// `transform` (the box centre, and its half diagonal times the largest
/// axis scale).
pub(super) fn bounding_sphere(min: Vec3f, max: Vec3f, transform: &Mat4f) -> (Vec3f, f32) {
    let c = (min + max) * 0.5;
    let t = &transform.v;
    let center = vec3f(
        t[0] * c.x + t[4] * c.y + t[8] * c.z + t[12],
        t[1] * c.x + t[5] * c.y + t[9] * c.z + t[13],
        t[2] * c.x + t[6] * c.y + t[10] * c.z + t[14],
    );
    let scale = [vec3f(t[0], t[1], t[2]), vec3f(t[4], t[5], t[6]), vec3f(t[8], t[9], t[10])]
        .iter()
        .map(|v| v.length())
        .fold(0.0f32, f32::max);
    (center, (max - min).length() * 0.5 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_lanes_build_a_variant_with_the_stock_layout() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_draw::script_mod(vm);
            vm.bx.heap.new_module(id!(prelude));
            script_eval!(vm, {
                mod.prelude.widgets_internal = {
                    ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw,
                }
            });
            vm.bx.heap.new_module(id!(widgets));
            crate::local_shadows::sampling::script_mod(vm);
            crate::clustered::script_mod(vm);
            crate::fast_gi::script_mod(vm);
            crate::shaders::script_mod(vm);
            crate::local_shadows::script_mod(vm);
            // `build` checks the layout against the stock shader itself.
            assert!(build(vm, OpaqueLane::Diffuse).is_some(), "diffuse variant");
            assert!(build(vm, OpaqueLane::Pbr).is_some(), "PBR variant");
            assert!(build(vm, OpaqueLane::City).is_some(), "city variant");
        });
    }

    #[test]
    fn occluder_fade_reach() {
        let eye = vec3f(0.0, 2.0, 10.0);
        let focus = vec3f(0.0, 1.0, 0.0);
        // A pillar between the camera and the body.
        assert!(in_occluder_fade(eye, focus, vec3f(0.3, 1.5, 5.0), 0.5));
        // Right beside the lens.
        assert!(in_occluder_fade(eye, focus, vec3f(1.0, 2.0, 10.5), 0.1));
        // Off to the side, and far behind the body.
        assert!(!in_occluder_fade(eye, focus, vec3f(8.0, 1.0, 5.0), 1.0));
        assert!(!in_occluder_fade(eye, focus, vec3f(0.0, 1.0, -20.0), 2.0));
    }

    #[test]
    fn bounding_sphere_scales() {
        let mut m = Mat4f::identity();
        m.v[0] = 2.0;
        m.v[5] = 2.0;
        m.v[10] = 2.0;
        m.v[12] = 5.0;
        let (c, r) = bounding_sphere(vec3f(-1.0, 0.0, -1.0), vec3f(1.0, 2.0, 1.0), &m);
        assert!((c - vec3f(5.0, 2.0, 0.0)).length() < 1e-5);
        assert!((r - 3.0f32.sqrt() * 2.0).abs() < 1e-4);
    }
}
