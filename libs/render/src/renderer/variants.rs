//! The model lanes' shader variants: each lane draws through a shader
//! built for what this frame actually uses, never the uber-shader.
//!
//! A model lane's shader (DrawSceneSkinned, DrawScenePbr, DrawSceneCity,
//! DrawSceneFoliageLit) carries every optional feature of the renderer:
//! fast GI sampling and its debug views, the sun cascades and their debug
//! view, the clustered lights, the `clip` hook. Each is gated by a uniform
//! at run time, so a scene without GI still compiled (and a driver still
//! inlined) all of it: the PBR lane was about 600 K of inlined GLSL, which
//! a cold ANGLE / D3D11 driver took ~14 s to link on first draw.
//!
//! So a lane draws through a variant: the same draw struct type with the
//! functions of every feature that is off this frame replaced by what the
//! stock function returns when its uniform switches it off (`gi_ambient`
//! returns its fallback, `csm_vis` 1, `cluster_lights` nothing, a debug view
//! its input colour, `clip` nothing for a draw that cuts no pixel). The
//! pixels are the stock shader's by construction; its instance, uniform and
//! texture layout is the stock one (checked once per variant), so the lane
//! writes its uniforms as before and only the shader id differs. Variants
//! are built on first use per (lane, features) and kept: a scene compiles
//! the few it uses.
//!
//! Draws that cut no pixel (no alpha test, dither, occluder fade, fur or
//! mask) go through the variant without `clip`: a pipeline that can discard
//! costs a tile GPU its hidden-surface removal (on the Mac mini, the race's
//! props cost 2.4 ms more at 1580x1520 with the hook than without it).

use super::*;
use std::collections::HashMap;

/// Which lane's shader a variant replaces.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ModelLane {
    Diffuse = 0,
    Pbr = 1,
    /// The streamed world's DrawSceneCity (stream_draw.rs).
    City = 2,
    /// Swaying trees and bushes (shaders/foliage.rs).
    Foliage = 3,
}

/// The optional features a lane's shader keeps.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub(crate) struct Features {
    /// The `clip` hook (alpha test, dither, occluder fade, fur, masks).
    pub clip: bool,
    /// Fast GI sampling (`gi_ambient`, `gi_display`).
    pub gi: bool,
    /// The GI debug views (in `gi_display`).
    pub gi_debug: bool,
    /// The sun cascades (`csm_vis`, `csm_vis_fast`).
    pub csm: bool,
    /// The cascade debug view (`csm_debug_view`).
    pub csm_debug: bool,
    /// The clustered lights (`cluster_lights`).
    pub cluster: bool,
    /// The lamps' shadow maps (`local_shadow_visibility`).
    pub local_shadows: bool,
}

/// One variant: not built yet, unavailable (its layout differs from the
/// stock shader's: the lane draws through the stock one), or built.
#[derive(Default, Clone, Copy)]
pub(super) enum VariantShader {
    #[default]
    Unbuilt,
    Unavailable,
    Built(DrawShaderId),
}

/// The variants built so far, and each lane's stock shader.
#[derive(Default)]
pub(super) struct LaneVariants {
    shaders: HashMap<(ModelLane, Features), VariantShader>,
    stock: HashMap<ModelLane, DrawShaderId>,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    // What each switched-off feature's functions become (see the module
    // docs): exactly what the stock function returns when its uniform has
    // the feature off.
    mod.draw.scene_variant = {
        clip: fn() {}
        gi_ambient: fn(wp: vec3, normal: vec3, fallback: vec3) -> vec3 { return fallback }
        gi_display: fn(color: vec4, wp: vec3, n: vec3) -> vec4 { return color }
        // GI on, no debug view: fast_gi's gi_display without its debug
        // branches.
        gi_display_plain: fn(color: vec4, wp: vec3, n: vec3) -> vec4 {
            if self.gi_on <= 0.0 || color.w < 0.999 { return color }
            let noise = fract(sin(dot(wp, vec3(127.1, 311.7, 74.7))) * 43758.5453) - 0.5
            return vec4(max(color.xyz + vec3(noise, noise, noise) * (1.0 / 255.0), vec3(0.0, 0.0, 0.0)), color.w)
        }
        csm_vis: fn(wp: vec3, n: vec3, ndl: float) -> float { return 1.0 }
        csm_debug_view: fn(color: vec4, wp: vec3, n: vec3) -> vec4 { return color }
        cluster_lights: fn(wp: vec3, normal: vec3, eye: vec3, albedo: vec3, roughness: float, metallic: float, pbr: float) -> vec3 { return vec3(0.0, 0.0, 0.0) }
        local_shadow_visibility: fn(index: float, wp: vec3, normal: vec3, light_pos: vec3, radius: float) -> float { return 1.0 }
    }
}

impl Renderer {
    /// The features this frame's lane uniforms leave on (what
    /// `bind_model_lane` writes), with `clip` on.
    pub(super) fn lane_features(&self) -> Features {
        let (gi, gi_debug) = self.gi.shader_features();
        let csm = self.gpu_baker.csm_binding().is_some_and(|(frame, _, _)| frame.on);
        Features { clip: true, gi, gi_debug, csm, csm_debug: csm && self.shadow_debug, cluster: self.clustered.active(self.clustered_enabled), local_shadows: self.clustered.shadows_active(self.clustered_enabled) }
    }

    /// The lane's shaders for this frame's features: the one it draws
    /// through, and the one without `clip` for draws that cut no pixel.
    /// `None` when a variant is not available (the VM was busy: next frame;
    /// or its layout differs): the lane then draws through its stock shader
    /// (whose id it is given).
    pub(super) fn lane_shaders(&mut self, cx: &mut Cx, lane: ModelLane, stock: Option<DrawShaderId>) -> (Option<DrawShaderId>, Option<DrawShaderId>) {
        if std::env::var_os("MAKEPAD_SHADER_VARIANTS").is_some_and(|v| v == "0") {
            return (self.lane_variants.stock.get(&lane).copied().or(stock), None);
        }
        let features = self.lane_features();
        let full = self.variant(cx, lane, features);
        let opaque = if std::env::var_os("MAKEPAD_OPAQUE_VARIANT").is_some_and(|v| v == "0") {
            None
        } else {
            self.variant(cx, lane, Features { clip: false, ..features })
        };
        let stock = self.lane_variants.stock.get(&lane).copied().or(stock);
        (full.or(stock), opaque)
    }

    fn variant(&mut self, cx: &mut Cx, lane: ModelLane, features: Features) -> Option<DrawShaderId> {
        let slot = self.lane_variants.shaders.get(&(lane, features)).copied().unwrap_or_default();
        if matches!(slot, VariantShader::Unbuilt) {
            if let Some(built) = cx.try_with_vm(|vm| build(vm, lane, features)) {
                let shader = match built {
                    Some((stock, variant)) => {
                        self.lane_variants.stock.insert(lane, stock);
                        VariantShader::Built(variant)
                    }
                    None => VariantShader::Unavailable,
                };
                self.lane_variants.shaders.insert((lane, features), shader);
                return match shader {
                    VariantShader::Built(id) => Some(id),
                    _ => None,
                };
            }
        }
        match slot {
            VariantShader::Built(id) => Some(id),
            _ => None,
        }
    }
}

/// `base` (a lane's type default, or a material's program) with the
/// functions of every feature `features` leaves off replaced by their
/// switched-off stubs. `lit` false: the program composes no light (an unlit
/// or flat material), so the lighting the lane computes for its compose
/// hook (cascades, clustered lights and their shadows, GI ambient) is never
/// read and goes too; its debug views and GI's display dither stay as the
/// frame has them.
pub(crate) fn variant_object(vm: &mut ScriptVm, base: ScriptObject, features: Features, lit: bool) -> Option<ScriptObject> {
    // The stubs, registered once per VM.
    let draw = vm.bx.heap.value(vm.bx.heap.modules, id!(draw).into(), NoTrap).as_object()?;
    if vm.bx.heap.value(draw, id!(scene_variant).into(), NoTrap).as_object().is_none() {
        script_mod(vm);
    }
    let stubs = vm.bx.heap.value(draw, id!(scene_variant).into(), NoTrap).as_object()?;
    let obj = vm.bx.heap.new_with_proto_no_vec(base.into());
    // (switched off, the function it replaces, the stub it becomes)
    let replace: [(bool, LiveId, LiveId); 9] = [
        (!features.clip, id!(clip), id!(clip)),
        (!features.gi || !lit, id!(gi_ambient), id!(gi_ambient)),
        (!features.gi, id!(gi_display), id!(gi_display)),
        (features.gi && !features.gi_debug, id!(gi_display), id!(gi_display_plain)),
        // The cascade debug view reads csm_vis: kept while it is on.
        (!features.csm || (!lit && !features.csm_debug), id!(csm_vis), id!(csm_vis)),
        (!features.csm || (!lit && !features.csm_debug), id!(csm_vis_fast), id!(csm_vis)),
        (!features.csm_debug, id!(csm_debug_view), id!(csm_debug_view)),
        (!features.cluster || !lit, id!(cluster_lights), id!(cluster_lights)),
        (!features.local_shadows || !lit, id!(local_shadow_visibility), id!(local_shadow_visibility)),
    ];
    let mut replaced = false;
    for (off, method, stub) in replace {
        if !off {
            continue;
        }
        // Only a function the program has (a lane without the feature keeps
        // its own code).
        let own = vm.bx.heap.object_method(base, method.into(), NoTrap);
        if !own.as_object().is_some_and(|f| vm.bx.heap.is_fn(f)) {
            continue;
        }
        let f = vm.bx.heap.value(stubs, stub.into(), NoTrap);
        let f = f.as_object().filter(|f| vm.bx.heap.is_fn(*f))?;
        if !vm.bx.heap.set_value(obj, method.into(), f.into(), NoTrap).is_nil() {
            return None;
        }
        replaced = true;
    }
    replaced.then_some(obj)
}

/// Whether `variant` writes its uniforms and textures where `stock` does
/// (the lane binds by the stock layout).
pub(crate) fn same_layout(cx: &Cx, stock: DrawShaderId, variant: DrawShaderId) -> bool {
    let (a, b) = (&cx.draw_shaders[stock.index].mapping, &cx.draw_shaders[variant.index].mapping);
    a.instances.total_slots == b.instances.total_slots
        && a.dyn_instances.total_slots == b.dyn_instances.total_slots
        && a.dyn_uniforms.total_slots == b.dyn_uniforms.total_slots
        && a.scope_uniforms.total_slots == b.scope_uniforms.total_slots
        && a.textures.iter().map(|t| t.id).eq(b.textures.iter().map(|t| t.id))
}

/// Build the variant of `lane` with `features`: (stock shader, variant).
fn build(vm: &mut ScriptVm, lane: ModelLane, features: Features) -> Option<(DrawShaderId, DrawShaderId)> {
    let type_id = match lane {
        ModelLane::Diffuse => DrawSceneSkinned::script_type_id_static(),
        ModelLane::Pbr => DrawScenePbr::script_type_id_static(),
        ModelLane::City => crate::shaders::DrawSceneCity::script_type_id_static(),
        ModelLane::Foliage => crate::shaders::DrawSceneFoliageLit::script_type_id_static(),
    };
    let base = vm.bx.heap.type_default_for_id(type_id)?;
    let stock = match lane {
        ModelLane::Pbr => DrawScenePbr::script_new_with_default(vm).skinned.draw_vars.draw_shader_id,
        ModelLane::Diffuse => DrawSceneSkinned::script_new_with_default(vm).draw_vars.draw_shader_id,
        ModelLane::City => crate::shaders::DrawSceneCity::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id,
        ModelLane::Foliage => crate::shaders::DrawSceneFoliageLit::script_new_with_default(vm).pbr.skinned.draw_vars.draw_shader_id,
    }?;
    // Every feature on and `clip` kept: the stock shader itself.
    let Some(obj) = variant_object(vm, base, features, true) else { return Some((stock, stock)) };
    // As DrawSceneCustom::from_surface: construction applies the stock
    // defaults (and may compile the stock shader); clear it, then apply the
    // variant object.
    let variant = match lane {
        ModelLane::Pbr => {
            let mut draw = DrawScenePbr::script_new(vm);
            draw.skinned.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            draw.skinned.draw_vars.draw_shader_id
        }
        ModelLane::Diffuse => {
            let mut draw = DrawSceneSkinned::script_new(vm);
            draw.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            draw.draw_vars.draw_shader_id
        }
        ModelLane::City => {
            let mut draw = crate::shaders::DrawSceneCity::script_new(vm);
            draw.pbr.skinned.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            draw.pbr.skinned.draw_vars.draw_shader_id
        }
        ModelLane::Foliage => {
            let mut draw = crate::shaders::DrawSceneFoliageLit::script_new(vm);
            draw.pbr.skinned.draw_vars.draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            draw.pbr.skinned.draw_vars.draw_shader_id
        }
    }?;
    if !same_layout(vm.cx(), stock, variant) {
        log!("render: the {} lane's variant {features:?} has another layout than the stock shader; drawing through the stock one", ["diffuse", "PBR", "city", "foliage"][lane as usize]);
        return None;
    }
    Some((stock, variant))
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
    use makepad_draw::makepad_platform::makepad_script::script_eval;

    #[test]
    fn every_lane_builds_its_variants_with_the_stock_layout() {
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
            makepad_render_graph::pass_stdlib(vm);
            crate::local_shadows::sampling::script_mod(vm);
            crate::clustered::script_mod(vm);
            crate::fast_gi::script_mod(vm);
            crate::shaders::script_mod(vm);
            crate::local_shadows::script_mod(vm);
            script_mod(vm);
            let draw = vm.bx.heap.value(vm.bx.heap.modules, id!(draw).into(), NoTrap).as_object().unwrap();
            // `build` checks the layout against the stock shader itself; the
            // GLSL program must define every function it calls (a stub
            // shared by two names was emitted under one of them only).
            for features in [
                Features::default(),
                Features { clip: true, ..Default::default() },
                Features { gi: true, ..Default::default() },
                Features { csm: true, cluster: true, ..Default::default() },
                Features { gi: true, gi_debug: true, csm: true, csm_debug: true, cluster: true, local_shadows: true, clip: false },
            ] {
                for lane in [ModelLane::Diffuse, ModelLane::Pbr, ModelLane::City, ModelLane::Foliage] {
                    let name = ["diffuse", "PBR", "city", "foliage"][lane as usize];
                    assert!(build(vm, lane, features).is_some(), "{name} variant {features:?}");
                    let type_id = match lane {
                        ModelLane::Diffuse => DrawSceneSkinned::script_type_id_static(),
                        ModelLane::Pbr => DrawScenePbr::script_type_id_static(),
                        ModelLane::City => crate::shaders::DrawSceneCity::script_type_id_static(),
                        ModelLane::Foliage => crate::shaders::DrawSceneFoliageLit::script_type_id_static(),
                    };
                    let base = vm.bx.heap.type_default_for_id(type_id).unwrap();
                    for lit in [true, false] {
                        let Some(obj) = variant_object(vm, base, features, lit) else { continue };
                        vm.bx.heap.set_value(draw, id!(variant_under_test).into(), obj.into(), NoTrap);
                        let source = script_eval!(vm, { mod.shader.test_compile_draw_source(mod.draw.variant_under_test, "glsl", false) });
                        let source = vm.bx.heap.string_with(source, |_, s| s.to_string()).unwrap_or_default();
                        assert!(!source.is_empty(), "{name} variant {features:?} lit {lit}: no GLSL source");
                        // `<type> io_name(..){` at the start of a line.
                        let defined: std::collections::HashSet<&str> = source.lines()
                            .filter(|l| l.trim_end().ends_with('{'))
                            .filter_map(|l| l.split_whitespace().nth(1).and_then(|w| w.split('(').next()).filter(|n| n.starts_with("io_")))
                            .collect();
                        for (i, _) in source.match_indices("io_") {
                            let name_end = source[i..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(source.len(), |e| i + e);
                            if source[name_end..].starts_with('(') && (i == 0 || !source[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_')) {
                                let called = &source[i..name_end];
                                assert!(defined.contains(called), "{name} variant {features:?} lit {lit}: calls {called}, which the GLSL source does not define");
                            }
                        }
                    }
                }
            }
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
