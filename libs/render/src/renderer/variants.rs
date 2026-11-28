//! The model lanes' shader variants: each lane draws through a shader
//! built for what this frame actually uses, never the uber-shader.
//!
//! A model lane's shader (DrawSceneSkinned, DrawScenePbr, DrawSceneCity,
//! DrawSceneFoliageLit), and the host's level draws (DrawSceneCube,
//! DrawSceneAlpha, DrawSceneTerrain, while the host keeps their stock
//! programs), carry every optional feature of the renderer:
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
//! its input colour, `clip` nothing for a draw that cuts no pixel; with the
//! clustered lights on, the per-draw light slots sum nothing and a lamp
//! pool fills no shadow, as their `cluster_on` branches do). The
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
use crate::GiReceivers;
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
    /// The host's level draws (SceneDraws): slabs, blended slabs, terrain.
    /// They draw through a variant only while the host keeps the stock
    /// program (a themed one keeps its own shader).
    Cube = 4,
    Alpha = 5,
    Terrain = 6,
    /// GPU-skinned characters (shaders/skinned_gpu.rs).
    SkinnedGpu = 7,
    /// Far stand-ins drawn inside the foliage lane (shaders/impostor.rs).
    Impostor = 8,
    /// Generated terrain tiles' paint (shaders/splat.rs).
    Splat = 9,
}

/// The lanes a load prepares (`Renderer::prepare_lane_variant`), in the order
/// a world's first frame meets them: level slabs and terrain, placed models,
/// foliage and its stand-ins, generated tiles, the streamed city, GPU-skinned
/// characters, blended slabs. `true`: the lane also draws through its variant
/// without `clip` (`lane_shaders`); the level draws take only theirs.
const STAGED_LANES: [(ModelLane, bool); 10] = [
    (ModelLane::Cube, false),
    (ModelLane::Terrain, false),
    (ModelLane::Diffuse, true),
    (ModelLane::Pbr, true),
    (ModelLane::Foliage, true),
    (ModelLane::Impostor, true),
    (ModelLane::Splat, true),
    (ModelLane::City, true),
    (ModelLane::SkinnedGpu, true),
    (ModelLane::Alpha, false),
];

/// The optional features a lane's shader keeps.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub(crate) struct Features {
    /// The `clip` hook (alpha test, dither, occluder fade, fur, masks).
    pub clip: bool,
    /// Fast GI sampling (`gi_ambient`, `gi_display`).
    pub gi: bool,
    /// Receive GI from the screen cache at the top of the splat pixel.
    pub gi_screen: bool,
    /// The GI debug views (in `gi_display`).
    pub gi_debug: bool,
    /// The sun cascades (`csm_vis`, `csm_vis_fast`).
    pub csm: bool,
    /// Use the fast cascade sample throughout the settled tier's shader.
    pub settled: bool,
    /// The cascade debug view (`csm_debug_view`).
    pub csm_debug: bool,
    /// The clustered lights (`cluster_lights`).
    pub cluster: bool,
    /// The lamps' shadow maps (`local_shadow_visibility`).
    pub local_shadows: bool,
    /// Extra surface/specular outputs, installed only on opaque lit variants.
    pub reflections: bool,
}

impl Features {
    fn with_gi_screen(self, lane: ModelLane, receivers: GiReceivers) -> Option<Self> {
        (lane == ModelLane::Splat && self.gi && receivers == GiReceivers::Fast)
            .then_some(Self { gi_screen: true, ..self })
    }
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

/// The variants built so far, and each lane's stock shader, as of the
/// reload walk (`Cx::reload_generation`) they were built after.
#[derive(Default)]
pub(super) struct LaneVariants {
    shaders: HashMap<(ModelLane, Features), VariantShader>,
    stock: HashMap<ModelLane, DrawShaderId>,
    no_receivers: HashMap<usize, VariantShader>,
    view_model_staged: bool,
    generation: u64,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    // What each switched-off feature's functions become (see the module
    // docs): exactly what the stock function returns when its uniform has
    // the feature off.
    mod.draw.scene_variant = {
        clip: fn() {}
        gi_ambient: fn(wp: vec3, normal: vec3, fallback: vec3) -> vec3 { return fallback }
        gi_ambient_at: fn(wp: vec3, n_geo: vec3, n: vec3, fallback: vec3) -> vec3 { return fallback }
        // Only the GI debug views read the screen cache's hit test.
        gi_screen_hit: fn(wp: vec3, n_geo: vec3) -> float { return -1.0 }
        gi_specular_occlusion: fn(ambient: vec3, hemisphere: vec3) -> float { return 1.0 }
        gi_display: fn(color: vec4, wp: vec3, n: vec3) -> vec4 {
            if color.w < 0.999 { return color }
            return vec4(self.output_dither(color.xyz, wp), color.w)
        }
        // GI on, no debug view: fast_gi's gi_display without its debug
        // branches.
        gi_display_plain: fn(color: vec4, wp: vec3, n: vec3) -> vec4 {
            if color.w < 0.999 { return color }
            return vec4(self.output_dither(color.xyz, wp), color.w)
        }
        csm_vis: fn(wp: vec3, n: vec3, ndl: float) -> float { return 1.0 }
        csm_debug_view: fn(color: vec4, wp: vec3, n: vec3) -> vec4 { return color }
        cluster_lights: fn(wp: vec3, normal: vec3, eye: vec3, albedo: vec3, roughness: float, metallic: float, pbr: float) -> vec3 { return vec3(0.0, 0.0, 0.0) }
        cluster_lights_f0: fn(wp: vec3, normal: vec3, eye: vec3, albedo: vec3, roughness: float, metallic: float, pbr: float, f0: vec3) -> vec3 { return vec3(0.0, 0.0, 0.0) }
        local_shadow_visibility: fn(index: float, wp: vec3, normal: vec3, light_pos: vec3, radius: float) -> float { return 1.0 }
        // Clustered lights on: the per-draw light slots are not summed and a
        // lamp pool does not fill the sun's shadow (their cluster_on branch).
        dl_sum: fn(wp: vec3, n: vec3) -> vec3 { return vec3(0.0, 0.0, 0.0) }
        sun_filled: fn(sun_vis: float, local: vec3) -> float { return sun_vis }
    }
}

impl Renderer {
    /// After a live edit's reload walk, what this renderer derived from the
    /// scene shaders' registered defaults is derived again: every installed
    /// material (the engine's and the host's alike) is built again from its
    /// recipe in the VM that owns it, and the lanes' stock shaders and
    /// variants on their next use. Code the edit left alone compiles to the
    /// shader (and pipeline) it had; an edited shader draws from the next
    /// frame on. The lane draws held from before take the edit themselves
    /// (`DrawVars::refresh_after_reload`), each where its lane picks its
    /// shaders. A material whose VM is busy is built on the next frame.
    pub(super) fn follow_reload(&mut self, cx: &mut Cx) {
        let generation = cx.reload_generation;
        if self.lane_variants.generation == generation {
            return;
        }
        for (name, m) in self.custom_draws.iter_mut() {
            if m.built_at == generation {
                continue;
            }
            match m.rebuild(cx) {
                None => return,
                Some(Ok(built)) => **m = built,
                Some(Err(e)) => log!("render: material {name} does not build on the edited shaders ({e}); it draws as it did"),
            }
            m.built_at = generation;
        }
        self.lane_variants = LaneVariants { generation, ..Default::default() };
        self.stock_ibl_forget_failure();
        self.item_materials_forget_failures();
    }

    /// The features this frame's lane uniforms leave on (what
    /// `bind_model_lane` writes), with `clip` on.
    pub(super) fn lane_features(&self) -> Features {
        let (gi, gi_debug) = self.gi.shader_features();
        let settled = crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode()).static_csm;
        let csm = settled || self.gpu_baker.csm_binding().is_some_and(|(frame, _, _)| frame.on);
        Features { clip: true, gi, gi_screen: false, gi_debug, csm, settled, csm_debug: csm && self.shadow_debug, cluster: self.clustered.active(self.clustered_enabled), local_shadows: self.clustered.shadows_active(self.clustered_enabled), reflections: self.ssr.active() }
    }

    /// A traced caster selects its Outside shader before the cascades, light grid
    /// and GI field are updated. Keep the configured lighting code in that shader.
    /// The scene uniforms switch sampling on when each producer has a binding.
    pub(super) fn traced_caster_features(&self) -> Features {
        let gi = self.gi.mode() == crate::GiMode::Fast && self.gi.config().strength > 0.0;
        Features {
            clip: true, gi, gi_screen: false, gi_debug: gi && self.gi.debug() != crate::GiDebug::Off,
            // Keep realtime sampling available across Off and night. Both bind csm_p.x = 0.
            csm: true, settled: crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode()).static_csm,
            csm_debug: self.shadow_debug, cluster: self.clustered_enabled, local_shadows: self.clustered_enabled,
            reflections: self.ssr.active(),
        }
    }

    /// The feature sets a world's frames draw a lane with, most likely
    /// first. They are this frame's, with the targets a world's first frame
    /// allocates counted as there: the sun cascades and the cluster
    /// grid, which a load staged before any world drew sees off. The lamps'
    /// shadows stay as this frame has them. In Realtime the cascades come
    /// both on and off: they follow the sun's side of the horizon, which is
    /// the world's and its hour's, and a day cycle crosses it in play.
    /// The settled tier stages its cascade variant once. At night the uniform switches sampling off.
    pub(super) fn world_frame_features(&self) -> Vec<Features> {
        let now = self.lane_features();
        let cluster = self.clustered_enabled;
        let world = Features { cluster, local_shadows: cluster && self.clustered.shadows.active(), ..now };
        let off = Features { csm: false, csm_debug: false, ..world };
        let on = Features { csm: true, csm_debug: self.shadow_debug, ..world };
        let tiers = crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode());
        if tiers.csm {
            vec![on, off]
        } else if tiers.static_csm {
            vec![on]
        } else {
            vec![off]
        }
    }

    /// Build the next stock lane variant a world's frames draw through (one
    /// per lane and `world_frame_features` set, with the one without `clip`
    /// where the lane has it). Fast receivers also stage the hoisted splat pair.
    /// A world's first frame built them otherwise, 4-11 ms each,
    /// 40-70 ms of a cold launch's first frame; a host's load
    /// prepares them while it stages. `Some(true)`: one was built;
    /// `Some(false)`: the VM is busy; `None` once every one is built (they
    /// are kept, so a later load finds them so).
    pub fn prepare_lane_variant(&mut self, cx: &mut Cx) -> Option<bool> {
        // The incoming world's SSR pass is first scheduled after staging.
        // Warm its stock view model on devices that can run that pass.
        self.follow_reload(cx);
        if self.lane_variants.generation != cx.reload_generation {
            return Some(false);
        }
        if !self.lane_variants.view_model_staged && cx.gpu_info().mixed_output_passes {
            let Some(stock) = cx.try_with_vm(|vm| DrawSceneViewModel::script_new_with_default(vm).draw_vars.draw_shader_id) else {
                return Some(false);
            };
            let variant = stock.map_or(VariantShader::Unavailable, |id| self.no_receiver_variant(cx, id));
            self.lane_variants.view_model_staged = !matches!(variant, VariantShader::Unbuilt);
            return Some(self.lane_variants.view_model_staged);
        }
        if std::env::var_os("MAKEPAD_SHADER_VARIANTS").is_some_and(|v| v == "0") {
            return None;
        }
        let opaque = !std::env::var_os("MAKEPAD_OPAQUE_VARIANT").is_some_and(|v| v == "0");
        let receivers = self.gi.receivers();
        let unbuilt = |lv: &LaneVariants, key| matches!(lv.shaders.get(&key), None | Some(VariantShader::Unbuilt));
        let (lane, features) = self.world_frame_features().into_iter()
            .flat_map(|features| STAGED_LANES.into_iter().flat_map(move |(lane, without_clip)| {
                std::iter::once(features).chain(features.with_gi_screen(lane, receivers)).flat_map(move |features| {
                    let bare = (without_clip && opaque).then_some((lane, Features { clip: false, ..features }));
                    std::iter::once((lane, features)).chain(bare)
                })
            }))
            .find(|key| unbuilt(&self.lane_variants, *key))?;
        self.variant(cx, lane, features);
        Some(!unbuilt(&self.lane_variants, (lane, features)))
    }

    /// Keep a colour-only draw's shading and clear the two SSR targets it covers.
    pub(super) fn no_receiver_shader(&mut self, cx: &mut Cx3d, stock: Option<DrawShaderId>) -> Option<DrawShaderId> {
        let id = stock?;
        if !self.ssr.active() || cx.passes[cx.current_pass_id()].color_textures.len() != 3
            || self.lane_variants.generation != cx.reload_generation {
            return stock;
        }
        match self.no_receiver_variant(cx.cx, id) {
            VariantShader::Built(variant) if cx.draw_shader_ready_here(variant) => Some(variant),
            _ => stock,
        }
    }

    fn no_receiver_variant(&mut self, cx: &mut Cx, stock: DrawShaderId) -> VariantShader {
        if cx.draw_shaders[stock.index].mapping.fragment_outputs != 1 {
            return VariantShader::Unavailable;
        }
        let entry = match self.lane_variants.no_receivers.entry(stock.index) {
            std::collections::hash_map::Entry::Occupied(entry) => return *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => entry,
        };
        if cx.is_script_vm_held() { return VariantShader::Unbuilt; }
        let source = cx.draw_shaders[stock.index].mapping.source.clone();
        let mut built = None;
        if !cx.with_vm_of_heap(source.heap_key(), &mut |vm| {
            built = build_no_receiver(vm, source.as_object());
        }) {
            return VariantShader::Unbuilt;
        }
        let variant = built.filter(|id| same_layout(cx, stock, *id));
        *entry.insert(variant.map_or(VariantShader::Unavailable, VariantShader::Built))
    }

    /// The lane's shaders for this frame's features: the one it draws
    /// through, and the one without `clip` for draws that cut no pixel.
    /// `None` when a variant is not available (the VM was busy: next frame;
    /// or its layout differs): the lane then draws through its stock shader.
    /// `held` is the shader on the lane's draw: a draw that could not take a
    /// live edit (its holder binds the old layout: a failed hot load, the
    /// restart is coming) keeps drawing that until the restart.
    pub(super) fn lane_shaders(&mut self, cx: &mut Cx, lane: ModelLane, held: Option<DrawShaderId>) -> (Option<DrawShaderId>, Option<DrawShaderId>) {
        let fits = |cx: &Cx, id: DrawShaderId| held.is_none_or(|held| same_layout(cx, held, id));
        let (full, opaque) = self.lane_variant_pair(cx, lane, self.lane_features());
        let full = full.or(self.lane_variants.stock.get(&lane).copied()).filter(|id| fits(&*cx, *id));
        (full.or(held), opaque.filter(|id| fits(&*cx, *id)))
    }

    /// Full and no-clip variants for drawing with the GI screen cache on
    /// (terrain tiles and splat-lane surfaces).
    pub(super) fn hoisted_lane_shaders(&mut self, cx: &mut Cx3d, lane: ModelLane, lane_shader: DrawShaderId) -> (Option<DrawShaderId>, Option<DrawShaderId>) {
        let Some(features) = self.lane_features().with_gi_screen(lane, self.gi.receivers()) else { return (None, None) };
        let (full, opaque) = self.lane_variant_pair(cx.cx, lane, features);
        let ready = |id: &DrawShaderId| same_layout(cx.cx, lane_shader, *id) && cx.draw_shader_ready_here(*id);
        (full.filter(&ready), opaque.filter(ready))
    }

    fn lane_variant_pair(&mut self, cx: &mut Cx, lane: ModelLane, features: Features) -> (Option<DrawShaderId>, Option<DrawShaderId>) {
        if std::env::var_os("MAKEPAD_SHADER_VARIANTS").is_some_and(|v| v == "0") {
            return (None, None);
        }
        let full = self.variant(cx, lane, features);
        let opaque = if std::env::var_os("MAKEPAD_OPAQUE_VARIANT").is_some_and(|v| v == "0") {
            None
        } else {
            self.variant(cx, lane, Features { clip: false, ..features })
        };
        (full, opaque)
    }

    /// Keep a drawable shader while the pass builds the new features variant.
    pub(super) fn select_lane_shader(
        &self, cx: &Cx, lane: ModelLane, full: Option<DrawShaderId>, held: Option<DrawShaderId>,
        ready: impl Fn(DrawShaderId) -> bool,
    ) -> Option<DrawShaderId> {
        let fits = |id| held.is_none_or(|held| held == id || same_layout(cx, held, id));
        let full = full.filter(|id| fits(*id)).or(held);
        if full.is_some_and(&ready) {
            return full;
        }
        if held != full && held.is_some_and(&ready) {
            return held;
        }
        let stock = self.lane_variants.stock.get(&lane).copied();
        stock.filter(|id| Some(*id) != full && Some(*id) != held && fits(*id) && ready(*id))
            .or(full)
    }

    /// A level draw's shader (SceneDraws: slabs, blended slabs, terrain)
    /// for this frame's features, when the host draws it through the stock
    /// program or one of its variants; `None` keeps the host's shader.
    pub(super) fn level_shader(&mut self, cx: &mut Cx, lane: ModelLane, current: Option<DrawShaderId>) -> Option<DrawShaderId> {
        let current = current?;
        if std::env::var_os("MAKEPAD_SHADER_VARIANTS").is_some_and(|v| v == "0") {
            return None;
        }
        let full = self.variant(cx, lane, self.lane_features());
        let stock = *self.lane_variants.stock.get(&lane)?;
        let ours = current == stock
            || self.lane_variants.shaders.iter().any(|((l, _), v)| *l == lane && matches!(v, VariantShader::Built(id) if *id == current));
        ours.then(|| full.unwrap_or(stock))
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
/// read and goes too; its debug views and output dither stay as the
/// frame has them.
pub(crate) fn variant_object(vm: &mut ScriptVm, base: ScriptObject, features: Features, lit: bool) -> Option<ScriptObject> {
    scene_variant(vm, base, features, lit, features.reflections && !features.clip && lit)
}

/// Traced surfaces discard misses in the entry point. Reflection outputs are
/// independent of that discard and of the optional host clipping hooks.
pub(crate) fn scene_variant(vm: &mut ScriptVm, base: ScriptObject, features: Features, lit: bool, reflection_outputs: bool) -> Option<ScriptObject> {
    // The stubs, registered once per VM.
    let draw = vm.value(vm.modules(), id!(draw)).as_object()?;
    if vm.value(draw, id!(scene_variant)).as_object().is_none() {
        script_mod(vm);
    }
    let stubs = vm.value(draw, id!(scene_variant)).as_object()?;
    let obj = vm.heap_mut().new_with_proto_no_vec(base.into());
    // The cascade debug view also samples visibility on unlit lanes.
    let sample_csm = features.csm && (lit || features.csm_debug);
    // (when to replace, the method, its replacement)
    let replace: [(bool, LiveId, LiveId); 18] = [
        (!features.clip, id!(clip), id!(clip)),
        (!features.gi || !lit, id!(gi_ambient), id!(gi_ambient)),
        (!features.gi || !lit, id!(gi_ambient_at), id!(gi_ambient_at)),
        (!features.gi_debug, id!(gi_screen_hit), id!(gi_screen_hit)),
        (!features.gi || !lit, id!(gi_specular_occlusion), id!(gi_specular_occlusion)),
        (!features.gi, id!(gi_display), id!(gi_display)),
        (features.gi && !features.gi_debug, id!(gi_display), id!(gi_display_plain)),
        (!sample_csm, id!(csm_vis), id!(csm_vis)),
        (!sample_csm, id!(csm_vis_fast), id!(csm_vis)),
        (sample_csm && features.settled, id!(csm_vis), id!(csm_vis_fast)),
        (!features.csm_debug, id!(csm_debug_view), id!(csm_debug_view)),
        (!features.cluster || !lit, id!(cluster_lights), id!(cluster_lights)),
        (!features.cluster || !lit, id!(cluster_lights_f0), id!(cluster_lights_f0)),
        (!features.local_shadows || !lit, id!(local_shadow_visibility), id!(local_shadow_visibility)),
        (features.cluster, id!(dl_sum), id!(dl_sum)),
        (features.cluster, id!(dl_sum_gated), id!(dl_sum)),
        (features.cluster, id!(sun_filled), id!(sun_filled)),
        // The program's own unlit shading (shaders/pbr.rs).
        (!lit, id!(shade), id!(shade_unlit)),
    ];
    let mut replaced = false;
    for (apply, method, stub) in replace {
        if !apply {
            continue;
        }
        // Only a function the program has (a lane without the feature keeps
        // its own code).
        let own = vm.object_method(base, method);
        if !own.as_object().is_some_and(|f| vm.is_fn(f)) {
            continue;
        }
        // A stub of scene_variant's, or else the program's own.
        let f = match vm.value(stubs, stub).as_object() {
            Some(f) => Some(f),
            None => vm.object_method(base, stub).as_object(),
        };
        let f = f.filter(|f| vm.is_fn(*f))?;
        if !vm.set_value(obj, method, f).is_nil() {
            return None;
        }
        replaced = true;
    }
    if reflection_outputs {
        let reflection = vm.value(draw, id!(pbr_reflections)).as_object()?;
        for field in [id!(fb1), id!(fb2), id!(ssr_surface), id!(ssr_coat), id!(ssr_finish), id!(ssr_gloss_none)] {
            let value = vm.value(reflection, field);
            if !vm.set_value(obj, field, value).is_nil() { return None; }
        }
        replaced = true;
    }
    let has = |vm: &mut ScriptVm, method: LiveId| vm.object_method(base, method).as_object().is_some_and(|f| vm.is_fn(f));
    if features.gi && features.gi_screen && lit && has(vm, id!(gi_receive_early)) && has(vm, id!(gi_receive_hoisted)) {
        for (to, from) in [
            (id!(gi_receive_early), id!(gi_receive_hoisted)),
            (id!(gi_surface_ambient), id!(gi_surface_ambient_pre)),
        ] {
            let f = vm.object_method(base, from).as_object().filter(|f| vm.is_fn(*f))?;
            if !vm.set_value(obj, to, f).is_nil() { return None; }
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

fn build_no_receiver(vm: &mut ScriptVm, base: ScriptObject) -> Option<DrawShaderId> {
    let draw = vm.value(vm.modules(), id!(draw)).as_object()?;
    let reflection = vm.value(draw, id!(pbr_reflections)).as_object()?;
    let obj = vm.heap_mut().new_with_proto_no_vec(base.into());
    let color = vm.object_method(base, id!(fragment)).as_object()?;
    if !vm.set_value(obj, id!(no_receiver_color), color).is_nil() { return None; }
    for (to, from) in [(id!(fb1), id!(fb1)), (id!(fb2), id!(fb2)), (id!(fragment), id!(ssr_held))] {
        let value = vm.value(reflection, from);
        if !vm.set_value(obj, to, value).is_nil() { return None; }
    }
    DrawVars::script_from_value(vm, obj.into()).draw_shader_id
}

/// A draw struct's shader: the type default's (`obj` None) or `obj`'s.
fn shader_of<T: ScriptNew + ScriptApply>(vm: &mut ScriptVm, obj: Option<ScriptObject>, vars: fn(&mut T) -> &mut DrawVars) -> Option<DrawShaderId> {
    match obj {
        None => vars(&mut T::script_new_with_default(vm)).draw_shader_id,
        Some(obj) => {
            // As DrawSceneCustom::from_surface: construction applies the
            // stock defaults (and may compile the stock shader); clear it,
            // then apply the variant object.
            let mut draw = T::script_new(vm);
            vars(&mut draw).draw_shader_id = None;
            draw.script_apply(vm, &Apply::New, &mut Scope::empty(), obj.into());
            vars(&mut draw).draw_shader_id
        }
    }
}

impl ModelLane {
    fn name(self) -> &'static str {
        ["diffuse", "PBR", "city", "foliage", "cube", "alpha", "terrain", "skinned", "impostor", "splat"][self as usize]
    }

    fn type_id(self) -> ScriptTypeId {
        match self {
            ModelLane::Diffuse => DrawSceneSkinned::script_type_id_static(),
            ModelLane::Pbr => DrawScenePbr::script_type_id_static(),
            ModelLane::City => crate::shaders::DrawSceneCity::script_type_id_static(),
            ModelLane::Foliage => crate::shaders::DrawSceneFoliageLit::script_type_id_static(),
            ModelLane::Cube => DrawSceneCube::script_type_id_static(),
            ModelLane::Alpha => DrawSceneAlpha::script_type_id_static(),
            ModelLane::Terrain => DrawSceneTerrain::script_type_id_static(),
            ModelLane::SkinnedGpu => DrawSceneSkinnedGpu::script_type_id_static(),
            ModelLane::Impostor => crate::shaders::DrawSceneImpostor::script_type_id_static(),
            ModelLane::Splat => crate::shaders::DrawSceneSplat::script_type_id_static(),
        }
    }

    /// The lane's shader for `obj` (None: its type default).
    fn shader(self, vm: &mut ScriptVm, obj: Option<ScriptObject>) -> Option<DrawShaderId> {
        match self {
            ModelLane::Pbr => shader_of::<DrawScenePbr>(vm, obj, |d| &mut d.skinned.draw_vars),
            ModelLane::Diffuse => shader_of::<DrawSceneSkinned>(vm, obj, |d| &mut d.draw_vars),
            ModelLane::City => shader_of::<crate::shaders::DrawSceneCity>(vm, obj, |d| &mut d.pbr.skinned.draw_vars),
            ModelLane::Foliage => shader_of::<crate::shaders::DrawSceneFoliageLit>(vm, obj, |d| &mut d.pbr.skinned.draw_vars),
            ModelLane::Cube => shader_of::<DrawSceneCube>(vm, obj, |d| &mut d.cube.draw_vars),
            ModelLane::Alpha => shader_of::<DrawSceneAlpha>(vm, obj, |d| &mut d.cube.cube.draw_vars),
            ModelLane::Terrain => shader_of::<DrawSceneTerrain>(vm, obj, |d| &mut d.draw_vars),
            ModelLane::SkinnedGpu => shader_of::<DrawSceneSkinnedGpu>(vm, obj, |d| &mut d.draw_vars),
            ModelLane::Impostor => shader_of::<crate::shaders::DrawSceneImpostor>(vm, obj, |d| &mut d.pbr.skinned.draw_vars),
            ModelLane::Splat => shader_of::<crate::shaders::DrawSceneSplat>(vm, obj, |d| &mut d.pbr.skinned.draw_vars),
        }
    }
}

/// Build the variant of `lane` with `features`: (stock shader, variant).
fn build(vm: &mut ScriptVm, lane: ModelLane, features: Features) -> Option<(DrawShaderId, DrawShaderId)> {
    // City, foliage and GPU-skinned have independent finishes; they do not
    // call shade_mr_surface and must not declare unwritten attachments.
    let features = Features { reflections: features.reflections && matches!(lane, ModelLane::Pbr | ModelLane::Splat), ..features };
    let base = vm.heap().type_default_for_id(lane.type_id())?;
    let stock = lane.shader(vm, None)?;
    // Every feature on and `clip` kept: the stock shader itself.
    let Some(obj) = variant_object(vm, base, features, true) else { return Some((stock, stock)) };
    let variant = lane.shader(vm, Some(obj))?;
    if !same_layout(vm.cx(), stock, variant) {
        log!("render: the {} lane's variant {features:?} has another layout than the stock shader; drawing through the stock one", lane.name());
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
pub(crate) mod tests {
    use super::*;
    use makepad_draw::makepad_platform::makepad_script::script_eval;

    #[test]
    fn pbr_extra_outputs_exist_only_in_the_reflections_variant() {
        let mut cx = shader_cx();
        let stock = cx.with_vm(|vm| {
            let base = vm.heap().type_default_for_id(ModelLane::Pbr.type_id()).unwrap();
            for (reflections, clip, lit) in [(false, false, true), (true, false, true),
                                             (true, true, true), (true, false, false)] {
                let features = Features { reflections, clip, ..Default::default() };
                let obj = variant_object(vm, base, features, lit).unwrap_or(base);
                let compiled = crate::shaders::compile_scene_webgl2(vm, obj, "PBR reflection outputs");
                let fragment = compiled.view().glsl_fragment;
                let capture = reflections && !clip && lit;
                for location in [1, 2] {
                    assert_eq!(fragment.contains(&format!("layout(location = {location}) out ")), capture,
                        "attachment {location}, {features:?} lit={lit}");
                }
                let finish = crate::shaders::lowered_fn_body(fragment, "ssr_finish").unwrap_or_default();
                assert_eq!(finish.contains("io_scene_fogged("), capture,
                    "only the capture program fogs the removed environment term");
                // It finishes only the pixels the reflection passes read: those over their floor.
                assert_eq!(finish.contains("uni_ssr_pass_floor"), capture,
                    "the capture must skip the pixels at or under the reflection passes' floor");
            }
            crate::volume_fog::script_mod(vm);
            crate::ssr::script_mod(vm);
            DrawSceneViewModel::script_new_with_default(vm).draw_vars.draw_shader_id.unwrap()
        });
        let mut renderer = Renderer::default();
        renderer.set_hdr_output(true);
        renderer.set_ssr(crate::SsrSettings { strength: 1.0, ..Default::default() });
        let scene = DrawPass::new(&mut cx);
        let auxiliary = DrawPass::new(&mut cx);
        let size = dvec2(4.0, 4.0);
        scene.set_size(&mut cx, size);
        auxiliary.set_size(&mut cx, size);
        let targets = [
            TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true },
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
            TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true },
        ].map(|format| Texture::new_with_format(&mut cx, format));
        for target in &targets {
            scene.add_color_texture(&mut cx, target, Default::default());
        }
        auxiliary.set_color_texture(&mut cx, &targets[0], Default::default());
        let depth = Texture::new_with_format(&mut cx, TextureFormat::DepthD32Sampled { size: TextureSize::Auto, initial: true });
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx = Cx3d::new(&mut draw);
        cx.begin_pass(&scene, Some(1.0));
        assert_eq!(renderer.no_receiver_shader(&mut cx, Some(stock)), Some(stock), "SSR inactive");
        assert!(renderer.lane_variants.no_receivers.is_empty(), "inactive SSR built a variant");
        renderer.run_ssr(&mut Cx2d::new(cx.cx), &scene, &depth, size);
        assert!(renderer.ssr.active());
        cx.begin_pass(&auxiliary, Some(1.0));
        assert_eq!(renderer.no_receiver_shader(&mut cx, Some(stock)), Some(stock), "single-target pass");
        assert!(renderer.lane_variants.no_receivers.is_empty(), "single-target pass built a variant");
        cx.end_pass(&auxiliary);

        let selected = renderer.no_receiver_shader(&mut cx, Some(stock));
        let VariantShader::Built(variant) = renderer.lane_variants.no_receivers[&stock.index] else {
            panic!("expected a no-receiver view-model shader");
        };
        assert_eq!(selected, Some(if cx.draw_shader_ready_here(variant) { variant } else { stock }));
        cx.end_pass(&scene);
        cx.with_vm(|vm| {
            let metal = |vm: &mut ScriptVm, id: DrawShaderId| {
                let program = vm.cx().draw_shaders[id.index].mapping.source.as_object();
                let compiled = script_eval!(vm, { mod.shader.test_compile_draw_source(#(program), "metal", false) });
                vm.heap().string_with(compiled, |_, s| s.to_owned()).unwrap()
            };
            let original = metal(vm, stock);
            assert_eq!(vm.cx().draw_shaders[stock.index].mapping.fragment_outputs, 1, "stock only declares fb0");
            assert!(original.contains("frag_fb0") && !original.contains("frag_fb1") && !original.contains("frag_fb2"));
            let body = crate::shaders::lowered_fn_body(&metal(vm, variant), "fragment").expect("Metal fragment entry");
            let (_, stores) = body.split_once("io_no_receiver_color(").expect("original colour function");
            for (target, alpha) in [("frag_fb1", crate::ssr::SSR_HELD_SURFACE), ("frag_fb2", 0.0)] {
                assert!(stores.contains(&format!("{target} = float4(0.0, 0.0, 0.0, {alpha:?})")),
                    "{target} must clear the receiver after the colour write, the surface marked as held: {body}");
            }
        });
        renderer.skip_ssr(cx.cx);
        cx.reload_generation += 1;
        renderer.follow_reload(cx.cx);
        assert!(renderer.lane_variants.no_receivers.is_empty(), "reload kept a derived program");
        while renderer.prepare_lane_variant(cx.cx) == Some(true) {
            if renderer.lane_variants.no_receivers.contains_key(&stock.index) { break; }
        }
        assert!(matches!(renderer.lane_variants.no_receivers.get(&stock.index),
            Some(VariantShader::Built(id)) if *id == variant), "load must stage the current stock program's variant");
    }

    #[test]
    fn traced_scene_variants() {
        shader_cx().with_vm(|vm| {
            let base = script_eval!(vm, { mod.draw.DrawSceneTraced }).as_object().expect("traced surface recipe");
            let sphere = TracedSurfaceRecipe::default();
            let hooks = script_eval!(vm, {
                use mod.prelude.widgets_internal.*
                { traced_base: fn(material: float, p: vec3) -> vec4 {
                    let index = self.trace_volume.sample(p * 0.5 + vec3(0.5, 0.5, 0.5)).x
                    return self.trace_palette.sample(vec2(index, 0.5))
                } }
            }).as_object().unwrap();
            let mut textured = sphere.clone();
            let sample = vm.object_method(hooks, id!(traced_base)).as_object().unwrap();
            textured.set_hook(vm, live_id!(traced_base), sample).unwrap();
            let lane = ModelLane::Pbr;
            let base_id = lane.shader(vm, Some(base)).expect("traced base shader");
            for (clip, reflections) in [(false, false), (false, true), (true, true)] {
                let features = Features {
                    clip, gi: true, gi_screen: false, gi_debug: true, csm: true, settled: false,
                    csm_debug: true, cluster: true, local_shadows: true, reflections,
                };
                let label = format!("outside sphere {features:?}");
                let obj = textured.scene_object(vm, features, false).expect("textured sphere variant");
                // Both recipes use the engine's analytic intersection. Compile the
                // unmodified scene recipe too, including its Metal pick replay.
                let analytic = if clip { sphere.scene_object(vm, features, false).unwrap() } else { obj };
                crate::shaders::inspect_scene_backends(vm, analytic, &label, Some(reflections));
                let id = lane.shader(vm, Some(obj)).expect("traced variant shader");
                assert!(same_layout(vm.cx(), base_id, id), "{features:?}: traced binding layout changed");
                let textures = &vm.cx().draw_shaders[id.index].mapping.textures;
                let names: Vec<_> = textures.iter().map(|t| t.id).collect();
                for map in [live_id!(normal_map), live_id!(occlusion_map), live_id!(orm_map), live_id!(emissive_map)] {
                    assert!(!names.contains(&map), "{features:?}: mesh texture {map} declared in traced layout");
                }
                assert!(names.len() <= makepad_draw::DRAW_CALL_TEXTURE_SLOTS);
                let compiled = crate::shaders::compile_scene_webgl2(vm, obj, &label);
                let fragment = compiled.view().glsl_fragment;
                assert!(fragment.contains("sampler3D tex_trace_volume;"), "{label}: missing volume sampler");
                assert!(fragment.contains("sampler2D tex_trace_palette;"), "{label}: missing palette sampler");
                for location in [1, 2] {
                    assert_eq!(fragment.contains(&format!("layout(location = {location}) out ")), reflections,
                        "{label}: reflection attachment {location}");
                }
                let entry = crate::shaders::lowered_fn_body(fragment, "fragment").unwrap();
                let (front, hit) = entry.split_once("io_traced_hit(").unwrap();
                assert!(front.contains("discard;") && hit.contains("discard;") && fragment.contains("gl_FragDepth ="),
                    "{label}: WebGL2 needs face and miss discard alongside hit depth");
                // GLSL declares sampler uniforms in both stages. Count the named
                // units actually referenced by the emitted fragment functions.
                let read: std::collections::HashSet<&str> = fragment.lines()
                    .filter(|line| !line.trim_start().starts_with("uniform "))
                    .flat_map(|line| line.split(|c: char| c != '_' && !c.is_ascii_alphanumeric()))
                    .filter(|word| word.starts_with("tex_"))
                    .collect();
                for name in ["tex_trace_volume", "tex_trace_palette"] {
                    assert!(read.contains(name), "{label}: {name} missing from fragment work");
                }
                let bound: std::collections::HashSet<_> = names.iter().map(|id| format!("tex_{id}")).collect();
                assert!(read.iter().all(|name| bound.contains(*name)), "{label}: fragment sampler without a binding");
                let vertex_only = names.contains(&live_id!(morph_map)) && !read.contains("tex_morph_map");
                let fragment_slots = names.len() - usize::from(vertex_only);
                assert!(fragment_slots <= 16, "{label}: {fragment_slots} fragment slots, {names:?}");
                assert!(read.len() <= 16, "{label}: {} fragment texture units: {read:?}", read.len());
                println!("{label}: {} fragment units, {} combined bindings", read.len(), names.len());
            }
        });
    }

    #[test]
    fn traced_required_layer() {
        use makepad_draw::makepad_math::{PointUsize, RectUsize, SizeUsize};
        use crate::custom_material::{TracedPass, TracedSurfacePasses, TracedSurfaceState::*};
        use crate::gpu_lightmap::BakeTrigger;
        use crate::model::{StaticDrawLayer, pack_ao_uv};
        use crate::renderer::draw_models::LayerPhase;

        let mut cx = shader_cx();
        let pass = DrawPass::new(&mut cx);
        let projection = Mat4f::perspective(60.0, 1.0, 0.1, 100.0);
        pass.set_keep_camera_matrix(&mut cx, true);
        pass.set_camera(&mut cx, Mat4f::translation(vec3f(0.0, -1.0, -4.0)), projection);
        let mut renderer = Renderer::default();
        renderer.pbr_materials_enabled = false;
        let mut diffuse = cx.with_vm(|vm| DrawSceneSkinned::script_new_with_default(vm));
        let stock = diffuse.draw_vars.draw_shader_id.unwrap();

        // Separate footprints make a proxy observable in the chart, GI and contact inputs.
        let layer = |name: &str, left: f32, color: u32| {
            let uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
            let vertices = uvs.iter().flat_map(|&[x, z]| {
                [left + x, 0.0, z, 0.0, 0.0, f32::from_bits(color), pack_ao_uv(x, z)]
            }).collect();
            StaticDrawLayer {
                name: name.into(), vertices, indices: vec![0, 1, 2], uvs,
                texture_png: None, detail_png: None, detail_scale: [1.0; 2], pbr: Default::default(),
            }
        };
        let layers = vec![layer("proxy", -6.0, u32::MAX), layer("sibling", 2.0, 0xff00_ff00)];
        let model = StaticModel {
            vertices: layers.iter().flat_map(|l| l.vertices.iter().copied()).collect(),
            indices: vec![0, 1, 2, 3, 4, 5], min: vec3f(-6.0, 0.0, 0.0), max: vec3f(3.0, 0.0, 1.0),
            material_name: Default::default(), texture_uri: None, texture_png: None,
            detail_png: None, detail_scale: [1.0; 2], pbr: Default::default(), ground_ao: None,
            parts: vec![], draw_layers: layers, prelit: false, anim_parts: vec![], driven_parts: vec![],
            sky: None, liquids: vec![], liquid_ranges: vec![],
        };
        let ao = Cx::encode_rgba_as_png(2, 2, &[255; 16]).unwrap();
        let mesh = PreparedStaticPreview::prepare_with_sidecars(model, Some(&ao), None).unwrap();
        let mut uploaded = renderer.upload_static_preview(&mut cx, mesh);
        let texture = uploaded.texture.clone();
        let palette = uploaded.detail.clone();
        renderer.set_ssao(Some((texture.clone(), 0.75)));
        let proxy = uploaded.geometry.geometry_id();
        let sibling = uploaded.extra_draws[0].0.geometry_id();
        assert_eq!(uploaded.lm_source.as_ref().unwrap().caster.tri_count(), 2, "fixture needs a merged chart");
        let pair = [ModelInstance {
            model: "pair".into(), transform: Mat4f::identity(), tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0), dynamic: false, depth_order: 0.0,
            custom_material: None, layer_materials: vec![], part_poses: vec![], lamps: Default::default(),
        }];
        let world = World::default();
        let sun = SunLight::default();
        renderer.install_uploaded_static("pair", uploaded.clone());
        renderer.set_models(pair.to_vec());
        let mut review = renderer.fork_scene_for_review();
        let pending = renderer.lightmap_bake_job(&world, &sun, BakeTrigger::FirstBake).unwrap();
        renderer.gpu_baker.schedule(pending);
        renderer.lightmap = Some(texture.clone());
        assert_eq!(uploaded.require_traced_layer(&mut cx, "proxy", "surface", TracedLayerRole::Outside), 1, "mark exactly the selected layer");
        let mut all_required = uploaded.clone();
        assert_eq!(all_required.require_traced_layer(&mut cx, "sibling", "surface", TracedLayerRole::Outside), 1, "mark exactly the selected layer");
        renderer.install_uploaded_static("pair", uploaded);
        assert!(!renderer.gpu_baker.has_state() && renderer.lightmap.is_none(),
            "a queued or delivered atlas containing the newly required proxy must be retired");
        review.mirror_review_assets_from(&renderer, false);
        let mirrored = review.lightmap_bake_job(&world, &sun, BakeTrigger::WorldEdit).unwrap();
        assert_eq!(mirrored.scene.meshes[0].source.caster.tri_count(), 1,
            "the second camera must notice a layer exclusion on the very same geometry handle");
        renderer.install_uploaded_static("only", all_required);
        let only = ModelInstance { model: "only".into(), ..pair[0].clone() };
        renderer.set_models(vec![only.clone()]);
        assert!(renderer.lightmap_bake_job(&world, &sun, BakeTrigger::FirstBake).is_none(),
            "an all-required static model has neither casters nor a ground footprint");
        renderer.set_models(vec![only.clone(), pair[0].clone()]);
        let ordinary_bounds = (vec3f(2.0, 0.0, 0.0), vec3f(3.0, 0.0, 1.0));
        let only_dynamic = ModelInstance { dynamic: true, ..only };
        let mixed_dynamic = ModelInstance { dynamic: true, ..pair[0].clone() };
        let sidecar = SdfMeta { rect: vec4(-1.0, -1.0, 1.0, 1.0), rows: 1, band_world: 0.25, len_per_unit: 1.0 };
        for copy in [&only_dynamic, &mixed_dynamic] {
            renderer.model_sdf_tex.insert(copy.model.clone(), Some((texture.clone(), sidecar)));
        }
        let check_stock_casters = |renderer: &Renderer, cascade: bool, label: &str| {
            let scene = &renderer.static_models[0].1;
            let mut records = Vec::new();
            let mut casters = Vec::new();
            Renderer::push_static_caster_layers(&renderer.custom_draws, scene, &pair[0], (scene.min, scene.max),
                &mut casters, cascade.then_some((0, &mut records)));
            assert_eq!(casters.iter().map(|c| c.geometry).collect::<Vec<_>>(), [sibling],
                "{label}: only the ordinary sibling may supply stock caster geometry");
        };
        check_stock_casters(&renderer, false, "static caster collection");

        let mut record = |renderer: &mut Renderer, cx: &mut Cx, pass: &DrawPass, shader: Option<DrawShaderId>, label: &str| {
            check_stock_casters(renderer, true, label);
            let event = DrawEvent::default();
            let mut list = DrawList::new(cx);
            let mut draw = CxDraw::new(cx, &event);
            draw.begin_pass(pass, Some(1.0));
            list.begin_always(&mut draw);
            let mut cx3d = Cx3d::new(&mut draw);
            let mut stats = RenderStats::default();
            let eye = vec3f(0.0, 1.0, 4.0);
            let fog = (Vec3f::default(), 0.0);
            renderer.draw_models_inner(&mut cx3d, ModelDraw::Diffuse(&mut diffuse), LayerPhase::Opaque,
                eye, 1.0, &pair, WorldModelLane::Attachment, fog, &sun, None, &mut stats);
            let stock_count = cx3d.draw_lists[list.id()].draw_items.len();
            assert_eq!(stock_count, 1, "{label}: exactly one stock submission");
            let calls = &cx3d.draw_lists[list.id()].draw_items;
            assert_eq!(calls[0].draw_call().unwrap().geometry_id, Some(sibling), "{label}: only the ordinary sibling may take the stock route");
            renderer.draw_custom_models(&mut cx3d, eye, 1.0, &pair, WorldModelLane::Attachment, fog, &sun, None, &mut stats);
            list.end(&mut draw);
            draw.end_pass(pass);
            let calls = &draw.draw_lists[list.id()].draw_items;
            assert_eq!(calls.len(), stock_count + usize::from(shader.is_some()), "{label}: traced submissions");
            if let Some(shader) = shader {
                let call = calls[stock_count].draw_call().unwrap();
                assert_eq!(call.geometry_id, Some(proxy), "{label}: the traced submission must use the proxy geometry");
                assert_eq!(call.draw_shader_id, shader, "{label}: submit the selected outside variant");
                assert_ne!(call.draw_shader_id, stock, "{label}: the traced lane must use its own shader");
                let expected_cull = if renderer.traced_programs["surface"].cull_proxy_faces() { CullMode::Back } else { CullMode::None };
                assert!(call.options.cull_mode == expected_cull && call.options.depth_write && !call.options.alpha_blend,
                    "{label}: Outside uses the recipe's culling and draws opaque with depth writes");
                let bindings = &draw.draw_shaders[call.draw_shader_id.index].mapping.textures;
                for (name, expected) in [(live_id!(ssao_map), &texture), (live_id!(trace_palette), &palette)] {
                    let slot = bindings.iter().position(|t| t.id == name).unwrap();
                    assert_eq!(call.texture_slots[slot].as_ref().map(Texture::texture_id), Some(expected.texture_id()),
                        "the traced draw lost its {name} binding");
                }
                if let Some((_, depth, _)) = renderer.gpu_baker.csm_binding().filter(|(frame, _, _)| frame.on) {
                    let slot = bindings.iter().position(|t| t.id == live_id!(csm_map)).expect("Outside must receive cascade shadows");
                    assert_eq!(call.texture_slots[slot].as_ref().map(Texture::texture_id), Some(depth.texture_id()),
                        "{label}: the prewarmed Outside shader must bind the fitted cascade depth");
                }
            }
        };

        // Record actual model draws for each state, including a placement before prewarm.
        let mut traced_shader = None;
        let reason = "lamp pipeline rejected the target format";
        let scene_reason = "Metal rejected the fragment output for this scene layout";
        let web_reason = crate::custom_material::TRACED_WEBGL_FAILURE;
        for (frame, expected) in [Waiting, Waiting, Waiting, Waiting, Waiting, Ready,
                                  Failed(reason.into()), Failed(reason.into()), Failed(scene_reason.into()),
                                  Failed(scene_reason.into()), Failed(web_reason.into()),
                                  Failed(web_reason.into())].into_iter().enumerate() {
            if frame == 1 {
                let mut recipe = TracedSurfaceRecipe::default();
                recipe.textures = vec![(live_id!(trace_palette), palette.clone())].into();
                renderer.install_traced_surface("surface".into(), recipe);
                renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
                let program = renderer.traced_programs.get_mut("surface").unwrap();
                program.passes.set_shader(TracedPass::Lamp, stock);
            }
            if frame == 8 {
                renderer.install_traced_surface("surface".into(), TracedSurfaceRecipe::default());
                renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
            }
            if frame >= 10 {
                let mut recipe = TracedSurfaceRecipe::volume(0.125, 32);
                recipe.host_vm = Some(std::rc::Rc::new(|_, _| panic!("WebGL2 must fail before entering the material VM")));
                renderer.install_traced_program("surface".into(), recipe, TracedSurfacePasses::ALL, frame == 11);
                let scene = renderer.traced_caster_scene(&cx, pass.draw_pass_id());
                renderer.traced_programs.get_mut("surface").unwrap()
                    .prepare_with_volume_support(&mut cx, scene, "surface", |_| Some(Err(web_reason)));
                assert_eq!(renderer.traced_surface_failure("surface"), Some(web_reason), "unsupported volume fails before shader compilation");
                renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
            }
            if let Some(program) = renderer.traced_programs.get_mut("surface") {
                let scene = pass.draw_pass_id();
                program.passes.refresh(|surface_pass, id, target| {
                    if surface_pass == TracedPass::Outside { traced_shader = Some(id); }
                    if frame == 6 && surface_pass == TracedPass::Lamp { return Err(reason.into()); }
                    if frame == 8 && target == Some(scene) { return Err(scene_reason.into()); }
                    Ok((surface_pass as usize) < frame - 1)
                });
            }
            assert_eq!(renderer.traced_surface_state("surface"), expected, "frame {frame}: full program availability");
            let queried = renderer.traced_surface_failure("surface");
            match &expected {
                Failed(reason) => assert_eq!(queried, Some(reason.as_str()), "terminal diagnostic on frame {frame}"),
                _ => assert_eq!(queried, None, "frame {frame}: a live program has no terminal reason"),
            }
            if matches!(frame, 0 | 5 | 6) {
                let atlas = renderer.lightmap_bake_job(&world, &sun, BakeTrigger::WorldEdit).unwrap();
                assert!(atlas.casters_only.is_empty(), "{expected:?}: all-required model cast into the atlas");
                assert_eq!(atlas.mesh_map, [1], "{expected:?}: only the mixed instance owns a chart");
                assert_eq!(atlas.scene.meshes.len(), 1, "only the sibling contributes a bake mesh");
                let chart = &atlas.scene.meshes[0].source;
                assert_eq!(chart.caster.tri_count(), 1, "the chart contains only the sibling triangle");
                assert_eq!(chart.caster.bounds(), ordinary_bounds, "the proxy cannot enlarge the chart");
                let sibling_points = [ordinary_bounds.0, vec3f(3.0, 0.0, 0.0), vec3f(2.0, 0.0, 1.0)];
                assert_eq!(chart.caster.triangle(0), (sibling_points[0], sibling_points[1], sibling_points[2]), "the chart preserves the sibling corners");
                assert_eq!((chart.ao_w, chart.ao_h), (2, 2), "the sibling retains its chart resolution");
                assert_eq!(chart.ao_uv, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], "the sibling retains its chart UVs");
                assert_eq!(chart.albedo, [vec3f(0.0, 1.0, 0.0); 3], "the chart keeps the sibling albedo");
                assert_eq!(atlas.mesh_geometry.len(), 1, "only the sibling uploads chart geometry");
                let gpu = &cx.geometries[atlas.mesh_geometry[0]];
                let vertices = gpu.vertices.as_f32().unwrap();
                let indices = gpu.indices.as_u32().unwrap();
                assert_eq!(indices.len(), 3, "{expected:?}: the GPU chart includes a proxy triangle");
                let (nx, ny) = crate::skin::oct_encode(vec3f(0.0, -1.0, 0.0));
                for (corner, &index) in indices.iter().enumerate() {
                    let v = &vertices[index as usize * crate::model::MODEL_VERTEX_FLOATS..];
                    assert_eq!(vec3f(v[0], v[1], v[2]), sibling_points[corner], "the GPU chart preserves the sibling position");
                    assert_eq!(v[3].to_bits(), makepad_draw::pack_pair_f16(nx, ny).to_bits(), "flat winding normal");
                    assert_eq!(crate::model::unpack_ao_uv(v[6]), chart.ao_uv[corner], "the GPU chart preserves the sibling UV");
                }
                let gi = renderer.gi_snapshot(&mut cx, &world, (Vec3f::all(-10.0), Vec3f::all(10.0)));
                assert_eq!((gi.instances.len(), gi.triangles), (1, 1), "{expected:?}: GI must see only the sibling");
                assert_eq!((gi.instances[0].lo, gi.instances[0].hi), ordinary_bounds, "GI uses the sibling bounds");
                let mut lamp = Vec::new();
                renderer.collect_local_static_casters(&mut cx, &world, &mut lamp);
                assert_eq!(lamp.iter().map(|c| c.geometry).collect::<Vec<_>>(), [sibling], "only the sibling casts into lamp faces");
                assert_eq!((lamp[0].min, lamp[0].max), ordinary_bounds, "the lamp caster keeps the sibling bounds");
                assert!(renderer.dynamic_contact_shadow_input(&mut cx, &only_dynamic, &sun).is_none(), "a required-only copy has no contact shadow");
                let (bounds, sdf) = renderer.dynamic_contact_shadow_input(&mut cx, &mixed_dynamic, &sun).unwrap();
                assert_eq!(bounds, ordinary_bounds, "{expected:?}: blob must sit under the sibling");
                assert!(sdf.is_none(), "{expected:?}: the model-wide SDF contains the proxy");
            }
            if frame == 5 {
                let scene = renderer.traced_caster_scene(&cx, pass.draw_pass_id()).with_features(renderer.traced_outside_features());
                let program = renderer.traced_programs.get_mut("surface").unwrap();
                assert!(program.inside(scene).is_some(), "the complete generic program warmed its real Inside pipeline");
            }
            let shader = (expected == Ready).then(|| traced_shader.expect("a ready full program probed Outside"));
            record(&mut renderer, &mut cx, &pass, shader, &format!("full program frame {frame}"));
        }

        // Record against two scene targets. Only the driver's answer to a
        // probe is supplied here; prewarm selects the shader and the target.
        let ldr = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        pass.set_color_texture(&mut cx, &ldr, Default::default());
        let hdr_pass = DrawPass::new(&mut cx);
        let hdr = Texture::new_with_format(&mut cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
        hdr_pass.set_color_texture(&mut cx, &hdr, Default::default());
        assert_ne!(cx.pass_layout(pass.draw_pass_id()), cx.pass_layout(hdr_pass.draw_pass_id()),
            "the second scene must require a different target layout");

        let complete_outside = |renderer: &mut Renderer, target: &DrawPass| {
            let mut shader = None;
            renderer.traced_programs.get_mut("surface").unwrap().passes.refresh(|pass, id, scene| {
                assert_eq!(pass, TracedPass::Outside, "only the installed outside shader can complete");
                assert_eq!(scene, Some(target.draw_pass_id()), "prewarm must probe the requested scene target");
                shader = Some(id);
                Ok(true)
            });
            shader.expect("outside prewarm must request a real compiled shader")
        };
        let mut recipe = TracedSurfaceRecipe::default();
        recipe.textures = vec![(live_id!(trace_palette), palette.clone())].into();
        let bindings = std::rc::Rc::downgrade(&recipe.textures);
        renderer.gpu_baker.set_mode(crate::gpu_lightmap::GpuLightmapMode::OnChange);
        renderer.install_traced_surface_for_passes("surface".into(), recipe.clone(), TracedSurfacePasses::OUTSIDE);
        renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "outside-only waits for the scene layout");
        record(&mut renderer, &mut cx, &pass, None, "outside pending");
        let first_shader = complete_outside(&mut renderer, &pass);
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "outside alone satisfies the declared set");
        let scene = renderer.traced_caster_scene(&cx, pass.draw_pass_id()).with_features(renderer.traced_outside_features());
        assert!(renderer.traced_programs.get_mut("surface").unwrap().inside(scene).is_none(), "Outside-only omits Inside");
        record(&mut renderer, &mut cx, &pass, Some(first_shader), "outside ready");

        pass.set_color_texture(&mut cx, &hdr, Default::default());
        record(&mut renderer, &mut cx, &pass, None, "replacement attachment on the same pass");
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "a changed attachment invalidates the probed layout");
        renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
        assert_eq!(complete_outside(&mut renderer, &pass), first_shader, "the rebuilt layout uses the original shader");
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "the replacement attachment's layout was admitted");
        record(&mut renderer, &mut cx, &hdr_pass, None, "a different target has not been prewarmed");
        renderer.prewarm_traced_surfaces(&mut cx, hdr_pass.draw_pass_id());
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "the new scene layout needs its own pipeline");
        record(&mut renderer, &mut cx, &hdr_pass, None, "new layout pending");
        assert_eq!(complete_outside(&mut renderer, &hdr_pass), first_shader, "changing layout retains the shader variant");
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "the new target's response admits its draw");
        record(&mut renderer, &mut cx, &hdr_pass, Some(first_shader), "new layout ready");

        renderer.gpu_baker.set_mode(crate::gpu_lightmap::GpuLightmapMode::Off);
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "the new features cannot reuse the old ready state");
        record(&mut renderer, &mut cx, &hdr_pass, None, "new features before prewarm");
        renderer.prewarm_traced_surfaces(&mut cx, hdr_pass.draw_pass_id());
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "a newly compiled variant still needs a pipeline");
        let next_shader = complete_outside(&mut renderer, &hdr_pass);
        assert_ne!(next_shader, first_shader, "disabling cascades selects a different scene shader");
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "the new variant is ready on its own completion");
        record(&mut renderer, &mut cx, &hdr_pass, Some(next_shader), "new variant ready");

        cx.reload_generation += 1;
        renderer.prewarm_traced_surfaces(&mut cx, hdr_pass.draw_pass_id());
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "shader reload revalidates the selected passes");
        let reloaded = complete_outside(&mut renderer, &hdr_pass);
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "shader reload retains the outside-only requirement");
        record(&mut renderer, &mut cx, &hdr_pass, Some(reloaded), "reloaded outside ready");
        renderer.traced_programs.get_mut("surface").unwrap().passes.refresh(|_, _, _| Err(scene_reason.into()));
        record(&mut renderer, &mut cx, &hdr_pass, None, "rejected outside on its previously ready target");
        renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
        assert_eq!(renderer.traced_surface_state("surface"), Failed(scene_reason.into()), "rejection survives a later target probe");
        assert_eq!(renderer.traced_surface_failure("surface"), Some(scene_reason), "the host can query the driver's reason");
        record(&mut renderer, &mut cx, &pass, None, "outside failed");

        let with_sun = TracedSurfacePasses { sun: true, ..TracedSurfacePasses::OUTSIDE };
        let scene_id = pass.draw_pass_id();
        renderer.set_clustered_lighting(true);
        renderer.set_cluster_config(crate::clustered::ClusterConfig {
            tiles_x: 1, tiles_y: 1, slices: 2, lights_per_cluster: 1, max_lights: 1,
        });
        renderer.install_traced_surface_for_passes("surface".into(), recipe.clone(), with_sun);
        assert!(!renderer.traced_programs["surface"].wants_sun(), "a recipe without a tile volume has no Sun draw");
        let volume = Texture::new_with_format(&mut cx, TextureFormat::Unknown);
        let coarse = Texture::new_with_format(&mut cx, TextureFormat::Unknown);
        let volume_recipe = |material: &Texture| {
            let mut recipe = TracedSurfaceRecipe::volume(0.125, 32);
            recipe.textures = [(live_id!(trace_volume), material.clone()), (live_id!(trace_coarse), coarse.clone()),
                (live_id!(trace_palette), palette.clone())].into();
            recipe
        };
        renderer.install_traced_surface_for_passes("surface".into(), volume_recipe(&texture), with_sun);
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(renderer.traced_surface_state("surface"), Failed("trace_volume must be a sampled 3D texture".into()));
        record(&mut renderer, &mut cx, &pass, None, "Off rejects definite 2D volume");

        renderer.install_traced_surface_for_passes("surface".into(), volume_recipe(&volume), with_sun);
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        complete_outside(&mut renderer, &pass);
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "Off admits Outside while the volume has no format");
        renderer.gpu_baker.set_mode(crate::gpu_lightmap::GpuLightmapMode::Realtime);
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "daylight requires Sun before a cascade is fitted");
        record(&mut renderer, &mut cx, &pass, None, "Off to Realtime before prewarm");
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        let shadow = renderer.gpu_baker.realtime_cascade_pass().expect("cold cascade pass");
        assert_eq!(cx.pass_layout(shadow), Some(crate::gpu_lightmap::cascade_layout()), "Off prewarms the actual cascade layout");
        let color = cx.passes[shadow].color_textures[0].texture.clone();
        let depth = cx.passes[shadow].depth_texture.clone().expect("sampled cascade depth");
        assert!(matches!(color.get_format(&mut cx), TextureFormat::RenderRf32 { .. }));
        assert!(matches!(depth.get_format(&mut cx), TextureFormat::DepthD32Sampled { .. }));
        assert!(cx.passes[shadow].main_draw_list_id.is_none(), "prewarm must precede the first caster recording");
        let sun_available = |renderer: &mut Renderer, cx: &Cx| {
            let scene = renderer.traced_caster_scene(cx, scene_id);
            renderer.traced_programs.get_mut("surface").unwrap().sun(scene).and_then(|batch| batch.borrow().draw.as_ref()?.draw_vars.draw_shader_id)
        };
        let exterior = complete_outside(&mut renderer, &pass);
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "an unknown format can still become a volume");
        assert_eq!(renderer.traced_surface_failure("surface"), None);

        let redraws = cx.frame_requests();
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(cx.frame_requests(), redraws, "an unknown format must not drive the redraw loop");
        renderer.gpu_baker.set_cascade_daylight(vec3f(0.0, -1.0, 0.0));
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        complete_outside(&mut renderer, &pass);
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "night admits Outside with an unknown Sun volume");
        record(&mut renderer, &mut cx, &pass, Some(exterior), "night waiting for a volume format");
        renderer.gpu_baker.set_cascade_daylight(vec3f(0.0, 1.0, 0.0));
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "daylight retries the unknown volume");

        // A recording Cx has no device handshake. Supply only the sampled-volume
        // format observation; the recipe and VM must build and bind the actual draw.
        renderer.traced_programs.get_mut("surface").unwrap().prepare_sun(&mut cx, |format| {
            assert!(matches!(format, TextureFormat::Unknown), "the recording supplies the missing device format");
            Ok(true)
        }).unwrap();
        let sun_id = renderer.traced_programs["surface"].sun_batch.as_ref().unwrap().borrow().draw.as_ref()
            .expect("the sampled-volume format builds the tile Sun draw").draw_vars.draw_shader_id.unwrap();
        assert_ne!(sun_id, stock);
        assert_ne!(sun_id, exterior);
        let finish = |renderer: &mut Renderer, response: Result<bool, String>| {
            renderer.traced_programs.get_mut("surface").unwrap().passes.refresh(|kind, id, target| match kind {
                TracedPass::Outside => { assert_eq!(target, Some(scene_id)); Ok(true) }
                TracedPass::Sun => { assert_eq!((id, target), (sun_id, Some(shadow))); response.clone() }
                _ => panic!("unexpected pass {kind:?}"),
            });
        };
        finish(&mut renderer, Ok(false));
        assert_eq!(sun_available(&mut renderer, &cx), None, "a pending caster supplies no draw");
        record(&mut renderer, &mut cx, &pass, None, "Sun waiting");
        finish(&mut renderer, Ok(true));
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "both pipelines admit the bound volume before its first upload");
        assert_eq!(sun_available(&mut renderer, &cx), Some(sun_id), "the admitted caster owns the real tile shader");
        let outside = renderer.traced_caster_scene(&cx, scene_id);
        let outside = renderer.traced_programs.get_mut("surface").unwrap().outside(outside).unwrap();
        for (name, tex) in [(live_id!(trace_volume), &volume), (live_id!(trace_coarse), &coarse)] {
            let slot = cx.draw_shaders[exterior.index].mapping.textures.iter().position(|t| t.id == name).unwrap();
            assert_eq!(outside.skinned.draw_vars.texture_slots[slot].as_ref().unwrap().texture_id(), tex.texture_id(),
                "Outside must use the replacement {name} binding");
        }
        record(&mut renderer, &mut cx, &pass, Some(exterior), "both layouts ready");
        // Use writable CPU texels to exercise a real dirty rect on this recording's
        // bound handle. No device allocation or upload takes place in this fixture.
        for tex in [&volume, &coarse] {
            *tex.get_format(&mut cx) = TextureFormat::VecRu8 {
                width: 2, height: 1, data: Some(vec![0; 2]), unpack_row_length: None, updated: TextureUpdated::Empty,
            };
            tex.set_staged_upload(&mut cx, false);
        }
        let mut texels = volume.take_vec_u8(&mut cx);
        texels[0] = 1;
        let rect = RectUsize { origin: PointUsize { x: 0, y: 0 }, size: SizeUsize { width: 1, height: 1 } };
        volume.put_back_vec_u8(&mut cx, texels, Some(rect));
        assert!(volume.upload_pending(&cx) && volume.allocated_size(&cx).is_none());
        let redraws = cx.frame_requests();
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        finish(&mut renderer, Ok(true));
        assert_eq!(cx.frame_requests(), redraws, "a dent must not request upload frames");
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "a dirty binding keeps the surface admitted");
        assert_eq!(sun_available(&mut renderer, &cx), Some(sun_id), "a dent retains the Sun draw");
        record(&mut renderer, &mut cx, &pass, Some(exterior), "daylight during an in-place edit");

        // The mixed model's traced layer lies wholly outside the ordinary sibling's bounds.
        let scene = renderer.traced_caster_scene(&cx, scene_id);
        let mut placed = pair[0].clone();
        placed.transform.v[13] = 6.0;
        placed.transform.v[14] = 8.0;
        renderer.set_models(vec![placed.clone()]);
        let eye = Vec3f::default();
        let mut movers = Vec::new();
        let mut static_movers = Default::default();
        renderer.collect_lm_movers(&world, eye, 1.0, None, &mut static_movers, &mut movers);
        renderer.admit_traced_casters(scene);
        assert!(movers.is_empty(), "the fixed sibling belongs to the static caster path");
        {
            let batch = renderer.gpu_baker.traced_casters[0].borrow();
            let [group] = batch.groups.as_slice() else { panic!("only the marked layer casts traced depth") };
            assert_eq!(group.geometry, proxy);
            assert_eq!(group.transform.v, placed.transform.v);
            assert_eq!((group.min, group.max), (vec3f(-6.0, 6.0, 8.0), vec3f(-5.0, 6.0, 9.0)));
        }

        #[cfg(all(unix, not(target_arch = "wasm32")))]
        if std::env::var_os("MAKEPAD_CSM_CASTER_LOG").is_none() {
            use crate::shaders::shader_registration_tests::allocations;
            let before = allocations::count();
            renderer.collect_lm_movers(&world, eye, 1.0, None, &mut static_movers, &mut movers);
            renderer.admit_traced_casters(scene);
            assert_eq!(allocations::count(), before, "warm traced group collection and admission allocated");
        }
        let job = renderer.lightmap_bake_job(&world, &sun, BakeTrigger::FirstBake).expect("sibling chart and ground");
        renderer.set_models(Vec::new());
        let mut attachment = ModelInstance { model: "only".into(), ..placed };
        renderer.set_world_attachments(vec![attachment.clone()]);
        renderer.gpu_baker.schedule(job);
        let square = |z| [vec3f(-2., -2., z), vec3f(2., -2., z), vec3f(-2., 2., z), vec3f(2., 2., z)];
        let view = crate::shadow_csm::CsmView { cam: eye, near_corners: square(0.1), far_corners: square(20.), focus_distance: 0.0 };
        // The camera sees x in [-2,2]. From the elevated proxy at x=-6,
        // this sun lands its shadow at x=0 on the floor.
        let dir = vec3f(-1., 1., 0.).normalize();
        let mut recording = None;
        for (frame, change) in ["first", "still", "dent", "uploaded", "pose", "add", "unready", "ready", "remove", "empty"].into_iter().enumerate() {
            match change {
                "first" | "uploaded" => {
                    // The recording has no uploader. The platform volume test covers the upload itself.
                    let TextureFormat::VecRu8 { updated, .. } = volume.get_format(&mut cx) else { unreachable!() };
                    *updated = TextureUpdated::Empty;
                }
                "dent" => {
                    let mut data = volume.take_vec_u8(&mut cx);
                    data[0] ^= 1;
                    volume.put_back_vec_u8(&mut cx, data, Some(rect));
                }
                "pose" => {
                    attachment.transform.v[12] += 0.125;
                    renderer.set_world_attachments(vec![attachment.clone()]);
                }
                "add" => { renderer.set_world_attachments(vec![attachment.clone(), attachment.clone()]); }
                "unready" => finish(&mut renderer, Ok(false)),
                "ready" => finish(&mut renderer, Ok(true)),
                "remove" => { renderer.set_world_attachments(Vec::new()); }
                _ => {}
            }
            renderer.collect_lm_movers(&world, eye, 1.0, None, &mut static_movers, &mut movers);
            assert!(movers.is_empty(), "the attachment has only required layers");
            renderer.admit_traced_casters(scene);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            draw.begin_pass(&pass, Some(1.0));
            if frame == 0 {
                let cold = renderer.lane_features();
                assert!(!cold.csm && !cold.cluster && !cold.local_shadows, "the scene must start before its lighting producers");
            }
            // A fixed fit isolates caster invalidation from changes to camera or scene bounds.
            renderer.gpu_baker.run_frame(&mut draw, dir, None, &[], &movers, Some(&view), eye,
                Some((Vec3f::all(-20.), Vec3f::all(20.))));
            if frame == 0 {
                let lamp = crate::lightmap::LmLight {
                    pos: vec3f(0.0, 3.0, 0.0), radius: 12.0, color: Vec3f::all(1.0),
                    dir: vec3f(0.0, 0.0, -1.0), cone: Some((10.0, 25.0)), shadows: true, ..Default::default()
                };
                renderer.clustered.update(draw.cx, &[lamp], None);
                renderer.clustered.render_shadows(&mut draw, eye, &[], &[]);
                let lit = renderer.lane_features();
                assert!(lit.csm && lit.cluster && lit.local_shadows, "the fit, grid and lamp must become active in this draw");
                assert_eq!(draw.current_pass_id(), scene_id, "the lighting passes must restore the HDR scene");
                assert!(scene == renderer.traced_caster_scene(draw.cx, draw.current_pass_id()),
                    "the caster and Outside need the same scene across the first lighting update");
            }
            draw.end_pass(&pass);
            let list = draw.passes[shadow].main_draw_list_id.unwrap();
            let current = draw.draw_lists[list].recording_gen;
            assert_eq!(recording.replace(current) == Some(current), matches!(change, "still" | "empty"),
                "{change}: only unchanged traced casters may keep the recorded atlas");
            let mut proxy_drawn = false;
            let mut flipped = false;
            for id in draw.passes.id_iter().filter(|&id| id == shadow || frame == 0 && draw.passes[id].depth_texture.is_some()) {
                let Some(list) = draw.passes[id].main_draw_list_id else { continue };
                let items = &draw.draw_lists[list].draw_items;
                for index in 0..items.len() {
                    let item = &items[index];
                    let Some(call) = item.draw_call() else { continue };
                    let inputs = &draw.draw_shaders[call.draw_shader_id.index].mapping.instances;
                    let Some(flip) = inputs.find(live_id!(flip_a)) else { continue };
                    let Some(values) = item.instances.as_ref() else { continue };
                    for instance in values.chunks_exact(inputs.total_slots) {
                        flipped |= instance[flip.offset] == 1.0;
                        if call.draw_shader_id == sun_id {
                            assert_eq!(id, shadow, "a traced draw reached another depth pass");
                            assert_eq!(instance[flip.offset], 0.0, "a flipped pass received a traced layer");
                            assert!(call.options.scissor.is_some(), "traced depth must stay in its tile");
                            let bindings = &draw.draw_shaders[call.draw_shader_id.index].mapping.textures;
                            for (name, tex) in [(live_id!(trace_volume), &volume), (live_id!(trace_coarse), &coarse)] {
                                let slot = bindings.iter().position(|t| t.id == name).unwrap();
                                assert_eq!(call.texture_slots[slot].as_ref().unwrap().texture_id(), tex.texture_id(), "Sun {name}");
                            }
                            assert_eq!(call.geometry_id, Some(proxy), "the other group's shadow lies past the camera");
                            let transform = inputs.find(live_id!(transform)).unwrap().offset;
                            assert_eq!(&instance[transform + 12..transform + 15], &attachment.transform.v[12..15]);
                            proxy_drawn = true;
                        }
                    }
                }
            }
            if frame == 0 {
                assert!(flipped, "the fixture must actually record the flipped ground pass");
                drop(draw);
                record(&mut renderer, &mut cx, &pass, Some(exterior), "Outside on the first fitted frame");
            }
            assert_eq!(proxy_drawn, !matches!(change, "unready" | "remove" | "empty"), "off-camera shadow must follow its attachment: {change}");
        }
        renderer.gpu_baker.traced_casters.clear();

        renderer.gpu_baker.set_cascade_daylight(vec3f(0.0, -1.0, 0.0));
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        finish(&mut renderer, Ok(true));
        record(&mut renderer, &mut cx, &pass, Some(exterior), "night prewarms both layouts while drawing Outside");
        assert_eq!(sun_available(&mut renderer, &cx), None, "night offers no cascade caster");
        renderer.gpu_baker.set_cascade_daylight(vec3f(0.0, 1.0, 0.0));
        assert_eq!(renderer.traced_surface_state("surface"), Ready, "dawn can use both prewarmed layouts immediately");
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        finish(&mut renderer, Ok(true));
        assert_eq!(sun_available(&mut renderer, &cx), Some(sun_id), "dawn revalidates the cached draw");

        let layout = cx.pass_layout(shadow).expect("shadow layout before recording");
        cx.passes[shadow].color_textures[0].texture = hdr.clone();
        assert_ne!(cx.pass_layout(shadow), Some(layout));
        assert_eq!(sun_available(&mut renderer, &cx), None, "a changed layout cannot use the old completion");
        record(&mut renderer, &mut cx, &pass, None, "shadow layout replaced");
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        let rejected = "Sun pipeline rejected the replacement target";
        finish(&mut renderer, Err(rejected.into()));
        assert_eq!(renderer.traced_surface_failure("surface"), Some(rejected));
        renderer.traced_programs.get_mut("surface").unwrap().passes.refresh(|_, _, _| panic!("a terminal failure was probed again"));
        assert_eq!(sun_available(&mut renderer, &cx), None, "rejection cannot select a stock caster");
        record(&mut renderer, &mut cx, &pass, None, "Sun failed");
        cx.passes[shadow].color_textures[0].texture = color;
        renderer.gpu_baker.set_cascade_daylight(vec3f(0.0, -1.0, 0.0));
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(complete_outside(&mut renderer, &pass), exterior);
        assert_eq!(renderer.traced_surface_failure("surface"), None, "night needs only the exterior");
        assert_eq!(sun_available(&mut renderer, &cx), None);
        record(&mut renderer, &mut cx, &pass, Some(exterior), "night after a Sun rejection");
        renderer.gpu_baker.set_cascade_daylight(vec3f(0.0, 1.0, 0.0));
        assert_eq!(renderer.traced_surface_failure("surface"), Some(rejected), "dawn retains this generation's diagnosis");
        record(&mut renderer, &mut cx, &pass, None, "day after a Sun rejection");
        *volume.get_format(&mut cx) = TextureFormat::Unknown;
        *coarse.get_format(&mut cx) = TextureFormat::Unknown;
        cx.reload_generation += 1;
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "reload checks the volume again");
        assert!(renderer.traced_programs["surface"].sun_batch.as_ref().unwrap().borrow().draw.is_none());
        let wrong_format = texture.get_format(&mut cx).clone();
        *volume.get_format(&mut cx) = wrong_format;
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(renderer.traced_surface_state("surface"), Failed("trace_volume must be a sampled 3D texture".into()),
            "a definite 2D format ends the wait");
        renderer.install_traced_surface_for_passes("surface".into(), TracedSurfaceRecipe::volume(0.125, 32), with_sun);
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(renderer.traced_surface_state("surface"), Failed("missing traced texture binding trace_volume".into()),
            "without a native session the missing binding keeps its own diagnostic");
        let mut duplicate = TracedSurfaceRecipe::volume(0.125, 32);
        let volume_slot = live_id!(trace_volume);
        let binding = (volume_slot, Texture::new_with_format(&mut cx, TextureFormat::Unknown));
        duplicate.textures = [binding.clone(), binding, (live_id!(trace_coarse), coarse.clone()),
            (live_id!(trace_palette), palette.clone())].into();
        renderer.install_traced_surface_for_passes("surface".into(), duplicate, with_sun);
        renderer.prewarm_traced_surfaces(&mut cx, scene_id);
        assert_eq!(renderer.traced_surface_state("surface"), Failed(format!("duplicate traced texture binding {volume_slot}")),
            "duplicate bindings keep their own diagnostic without a native session");

        #[cfg(not(target_arch = "wasm32"))]
        {
            use makepad_draw::makepad_platform::makepad_hal::{backend_id, HalInfo};
            let scene = renderer.traced_caster_scene(&cx, scene_id);
            let prepare = |renderer: &mut Renderer, cx: &mut Cx, support| {
                renderer.traced_programs.get_mut("surface").unwrap().prepare_with_volume_support(cx, scene, "surface", |_| Some(support));
            };
            renderer.install_traced_surface_for_passes("surface".into(), TracedSurfaceRecipe::volume(0.125, 32), with_sun);
            prepare(&mut renderer, &mut cx, Ok(()));
            assert_eq!(renderer.traced_surface_state("surface"), Failed("missing traced texture binding trace_volume".into()),
                "a capable device keeps the missing-binding diagnostic");

            let device = HalInfo { backend: backend_id::VULKAN, ..Default::default() };
            let support = device.sampled_volume_support();
            let reason = support.expect_err("the recording advertises no 3D textures");
            for (label, count) in [("missing", 0), ("2D", 1), ("duplicate", 2)] {
                let mut tiles = TracedSurfaceRecipe::volume(0.125, 32);
                tiles.textures = vec![(volume_slot, volume.clone()); count].into();
                renderer.install_traced_surface_for_passes("surface".into(), tiles, with_sun);
                prepare(&mut renderer, &mut cx, support);
                assert_eq!(renderer.traced_surface_state("surface"), Failed(reason.into()), "{label} volume uses the device's reason");
            }

            *volume.get_format(&mut cx) = TextureFormat::Unknown;
            let mut tiles = TracedSurfaceRecipe::volume(0.125, 32);
            tiles.textures = [(volume_slot, volume.clone())].into();
            renderer.install_traced_surface_for_passes("surface".into(), tiles.clone(), with_sun);
            prepare(&mut renderer, &mut cx, support);
            assert_eq!(renderer.traced_surface_state("surface"), Failed(reason.into()), "device rejection precedes format readiness");

            let headless = HalInfo { backend: backend_id::HEADLESS, ..Default::default() };
            renderer.install_traced_surface_for_passes("surface".into(), tiles, with_sun);
            let support = headless.sampled_volume_support();
            let reason = support.expect_err("headless has no sampled volume support");
            prepare(&mut renderer, &mut cx, support);
            assert_eq!(renderer.traced_surface_state("surface"), Failed(reason.into()), "headless reports its device rejection immediately");
        }
        renderer.gpu_baker.set_mode(crate::gpu_lightmap::GpuLightmapMode::Off);

        renderer.install_traced_outside_preview("surface".into(), recipe);
        renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
        let scene = renderer.traced_caster_scene(&cx, pass.draw_pass_id()).with_features(renderer.traced_outside_features());
        let program = renderer.traced_programs.get_mut("surface").unwrap();
        program.passes.refresh(|pass, _, _| Ok(pass == TracedPass::Outside));
        assert!(program.inside(scene).is_none(), "an isolated Outside preview omits Inside");
        let preview = program.outside(scene).unwrap().skinned.draw_vars.draw_shader_id.unwrap();
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "preview still reports full-program availability");
        record(&mut renderer, &mut cx, &pass, Some(preview), "isolated outside preview");
        assert!(renderer.remove_traced_surface("surface"), "removing an installed binding reports its removal");
        assert!(bindings.upgrade().is_none(), "removal releases the program's texture list");
        assert!(!renderer.remove_traced_surface("surface"), "removing an absent program is a no-op");
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "an uninstalled required name has no ready program");
        record(&mut renderer, &mut cx, &pass, None, "required layer after program removal");

        // A fresh generation can fail during prewarm even if nobody draws
        // it. Holding the host VM delays that same request until it is free.
        let busy = std::rc::Rc::new(std::cell::Cell::new(true));
        let mut recipe = TracedSurfaceRecipe::default();
        let held = busy.clone();
        recipe.host_vm = Some(std::rc::Rc::new(move |cx, build| {
            !held.get() && cx.try_with_vm(|vm| build(vm)).is_some()
        }));
        let missing = live_id!(missing_surface_texture);
        recipe.textures = vec![(missing, texture)].into();
        renderer.install_traced_surface("surface".into(), recipe);
        renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
        assert_eq!(renderer.traced_surface_state("surface"), Waiting, "a busy host VM leaves prewarm pending");
        busy.set(false);
        renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
        let Failed(message) = renderer.traced_surface_state("surface") else { panic!("expected texture binding failure") };
        assert!(message.contains(&missing.to_string()), "the host needs the rejected binding: {message}");

        let required = TracedSurfacePasses { inside: true, ..TracedSurfacePasses::OUTSIDE };

        let mut volume = TracedSurfaceRecipe::volume(0.1, 32);
        volume.textures = [live_id!(trace_volume), live_id!(trace_coarse), live_id!(trace_palette)]
            .map(|id| (id, Texture::new_with_format(&mut cx, TextureFormat::Unknown))).into();
        pass.set_size(&mut cx, dvec2(200.0, 100.0));
        let scene = renderer.traced_caster_scene(&cx, pass.draw_pass_id()).with_features(renderer.traced_outside_features());
        use CullMode::{Back, Front, None as NoCull};
        let big = 2.0f32.powi(64);
        let analytic_cases = [(0.5, vec3f(1.0, 1.0, 1.0), false, true, NoCull, NoCull)];
        // Eye Z, copy scale, stage reflection, near-plane overlap, Outside and Inside cull modes.
        let volume_cases = [
            (4.0, vec3f(1.0, 1.0, 1.0), false, false, Back, Front),
            (0.5, vec3f(1.0, 1.0, 1.0), false, true, Back, Front),
            (0.5, vec3f(-1.0, 1.0, 1.0), false, true, Front, Back),
            (0.5, vec3f(1.0, 1.0, 1.0), true, true, Front, Back),
            (0.5, vec3f(-1.0, 1.0, 1.0), true, true, Back, Front),
            (0.5, vec3f(0.0, 1.0, 1.0), false, true, NoCull, NoCull),
            (0.5, vec3f(1.0, big, big), false, true, NoCull, NoCull),
            (0.5, vec3f(1.0, big, big), true, true, NoCull, NoCull),
        ];
        for (recipe, outside_default, inside_default, cases) in [
            (TracedSurfaceRecipe::default(), NoCull, NoCull, analytic_cases.as_slice()),
            (volume, Back, Front, volume_cases.as_slice()),
        ] {
            renderer.install_traced_surface_for_passes("surface".into(), recipe, required);
            renderer.prewarm_traced_surfaces(&mut cx, pass.draw_pass_id());
            let program = renderer.traced_programs.get_mut("surface").unwrap();
            program.passes.refresh(|_, _, _| Ok(true));
            let outside = &program.outside(scene).unwrap().skinned.draw_vars;
            assert_eq!(outside.options.cull_mode, outside_default, "Outside shader default");
            let outside = outside.draw_shader_id.unwrap();
            let inside = &program.inside(scene).unwrap().skinned.draw_vars;
            assert_eq!(inside.options.cull_mode, inside_default, "Inside shader default");
            let mut inv = [0.0; 2];
            inside.get_uniform(&mut cx, live_id!(trace_inv_viewport), &mut inv);
            assert_eq!(inv, [1.0/200.0, 1.0/100.0], "Inside receives the active viewport");
            let inside = inside.draw_shader_id.unwrap();
            assert_ne!(outside, inside);
            for &(eye_z, scale, mirror_stage, overlaps, outside_cull, inside_cull) in cases {
                let eye = vec3f(0.0, 0.0, eye_z);
                pass.set_camera(&mut cx, Mat4f::translation(eye * -1.0), projection);
                let copies = [20.0, 0.0].map(|x| ModelInstance {
                    transform: Mat4f::nonuniform_scaled_translation(scale, vec3f(x, 0.0, 0.0)), ..pair[0].clone()
                });
                let mut list = DrawList::new(&mut cx);
                let event = DrawEvent::default();
                let mut draw = CxDraw::new(&mut cx, &event);
                draw.begin_pass(&pass, Some(1.0));
                list.begin_always(&mut draw);
                if mirror_stage { draw.draw_lists[list.id()].draw_list_uniforms.view_transform.v[0] = -1.0; }
                let mut cx3d = Cx3d::new(&mut draw);
                let mut stats = RenderStats::default();
                renderer.draw_custom_models(&mut cx3d, eye, 1.0, &copies, WorldModelLane::Attachment,
                    (Vec3f::default(), 0.0), &sun, None, &mut stats);
                list.end(&mut draw);
                draw.end_pass(&pass);
                let items = &draw.draw_lists[list.id()].draw_items;
                assert_eq!(stats.traced_inside_draws, u64::from(overlaps), "Inside group count at {eye_z}");
                assert_eq!((stats.world_attachment_draws, stats.world_attachment_instances, stats.world_attachment_triangles),
                    (1, 2, 4), "Inside preserves Outside accounting for two copies of the two-triangle model");
                assert_eq!(items.len(), 1 + usize::from(overlaps), "only submit Inside when one copy overlaps");
                for index in 0..items.len() {
                    let is_inside = overlaps && index == 0;
                    let item = &items[index];
                    let call = item.draw_call().unwrap();
                    assert_eq!(call.geometry_id, Some(proxy));
                    assert_eq!(call.draw_shader_id, if is_inside { inside } else { outside }, "Inside precedes Outside when the near plane overlaps");
                    let expected = if is_inside { inside_cull } else { outside_cull };
                    assert_eq!(call.options.cull_mode, expected, "eye_z={eye_z} scale={scale:?} mirror_stage={mirror_stage} inside={is_inside}");
                    let data = item.instances.as_ref().unwrap().as_slice();
                    assert_eq!(data.len(), call.total_instance_slots * if is_inside { 1 } else { 2 });
                    if is_inside {
                        let at = draw.draw_shaders[inside.index].mapping.instances.find(live_id!(transform)).unwrap().offset;
                        assert_eq!(&data[at..at+16], &copies[1].transform.v, "Inside retains the second copy's original slot");
                    }
                }
            }
        }
        let rejected = "Inside depth output rejected by the scene target";
        renderer.traced_programs.get_mut("surface").unwrap().passes.refresh(|pass, _, _| {
            if pass == TracedPass::Inside { Err(rejected.into()) } else { Ok(true) }
        });
        assert_eq!(renderer.traced_surface_state("surface"), Failed(rejected.into()));
        assert!(renderer.traced_programs.get_mut("surface").unwrap().inside(scene).is_none());

    }

    /// A Cx whose VM has the renderer's shaders.
    pub(crate) fn shader_cx() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(register_test_shaders);
        cx
    }

    fn register_test_shaders(vm: &mut ScriptVm) {
        makepad_draw::script_mod(vm);
        vm.heap_mut().new_module(id!(prelude));
        script_eval!(vm, {
            mod.prelude.widgets_internal = {
                ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw,
            }
        });
        vm.heap_mut().new_module(id!(widgets));
        makepad_render_graph::pass_stdlib(vm);
        crate::local_shadows::sampling::script_mod(vm);
        crate::clustered::script_mod(vm);
        crate::fast_gi::script_mod(vm);
        crate::shaders::script_mod(vm);
        crate::local_shadows::script_mod(vm);
        script_mod(vm);
    }

    /// The painted-terrain, foliage and stand-in lanes are built for a model that draws through
    /// them: with none resident, nothing of theirs reaches the VM or the compiler.
    #[test]
    fn a_lane_without_a_model_builds_nothing() {
        let mut cx = shader_cx();
        let mut renderer = Renderer::default();
        renderer.prepare_splat_lane(&mut cx);
        renderer.prepare_foliage_lane(&mut cx);
        renderer.prepare_impostor_lane(&mut cx);
        assert!(!renderer.splat_ready && !renderer.foliage_ready && renderer.impostor_shader.is_none());
        assert!(renderer.splat_draw.is_none() && renderer.foliage_draw.is_none());
        assert!(renderer.lane_variants.shaders.is_empty());
    }

    /// A live edit of a lane's shader (its default registered again with
    /// another pixel, the reload walk counted) reaches the lane's next draw:
    /// the lane draw held from before takes the edited default, and the
    /// lane's variants are built again from it. A walk that runs the same
    /// code again keeps the shaders, and so the pipelines, the lane had.
    /// The edit's report (`Cx::live_shader_switches`) names exactly the draws
    /// whose shader it changed: the held draw, though its lane draws it
    /// through a variant and a collection ran since it compiled, and a draw
    /// the walk applies; never a draw of a type the edit left alone, nor any
    /// draw when only the same code ran again.
    #[test]
    fn a_live_edit_of_a_lane_shader_reaches_the_lanes_next_draw() {
        let mut cx = shader_cx();
        let mut renderer = Renderer::default();
        let mut held = cx.with_vm(|vm| DrawScenePbr::script_new_with_default(vm));
        // Two draws the reload walk applies, as a widget's `#[live]` fields: one of the edited
        // type, one of a type the edit leaves alone (the sky).
        let (mut tree_pbr, mut tree_sky) = cx.with_vm(|vm| {
            let (pbr, sky) = (script_eval!(vm, { mod.draw.DrawScenePbr{} }), script_eval!(vm, { mod.draw.DrawSceneSky{} }));
            (DrawScenePbr::script_from_value(vm, pbr), DrawSceneSky::script_from_value(vm, sky))
        });
        let walk = |cx: &mut Cx, pbr: &mut DrawScenePbr, sky: &mut DrawSceneSky| cx.with_vm(|vm| {
            let (pbr_value, sky_value) = (script_eval!(vm, { mod.draw.DrawScenePbr{} }), script_eval!(vm, { mod.draw.DrawSceneSky{} }));
            pbr.script_apply(vm, &Apply::Reload, &mut Scope::empty(), pbr_value);
            sky.script_apply(vm, &Apply::Reload, &mut Scope::empty(), sky_value);
        });
        // What a frame's PBR pass does with the lane's draw (draw_models.rs):
        // (the held draw's own shader, the lane's, the lane's without clip).
        let frame = |cx: &mut Cx, renderer: &mut Renderer, held: &mut DrawScenePbr| {
            renderer.follow_reload(cx);
            held.skinned.draw_vars.refresh_after_reload(cx);
            let own = held.skinned.draw_vars.draw_shader_id.expect("the held draw's shader");
            let (full, opaque) = renderer.lane_shaders(cx, ModelLane::Pbr, Some(own));
            held.skinned.draw_vars.draw_shader_id = full;
            (own, full.expect("the lane's shader"), opaque.expect("the lane's shader without clip"))
        };
        let before = frame(&mut cx, &mut renderer, &mut held);
        cx.with_vm(|vm| vm.with_reload(|vm| crate::shaders::script_mod(vm)));
        // the app collects between frames: what was cached by object is gone
        cx.with_vm(|vm| vm.gc());
        cx.reload_generation += 1;
        walk(&mut cx, &mut tree_pbr, &mut tree_sky);
        assert_eq!(frame(&mut cx, &mut renderer, &mut held), before, "the same code run again: the same shaders");
        assert_eq!(cx.live_shader_switches(), &[], "the same code run again changes no draw's shader");
        let sky_before = tree_sky.draw_vars.draw_shader_id;
        cx.with_vm(|vm| {
            script_eval!(vm, {
                use mod.prelude.widgets_internal.*
                mod.draw.DrawScenePbr = mod.std.set_type_default() do mod.draw.DrawScenePbr{
                    pixel: fn() { return vec4(1.0, 0.0, 1.0, 1.0) }
                }
            });
        });
        cx.reload_generation += 1;
        walk(&mut cx, &mut tree_pbr, &mut tree_sky);
        let edited = frame(&mut cx, &mut renderer, &mut held);
        assert_eq!(tree_sky.draw_vars.draw_shader_id, sky_before, "premise: the sky's shader is the one it had");
        assert_eq!(cx.live_shader_switches(), &[("DrawScenePbr".to_string(), 2)],
            "the edit's report names the held draw and the walk's draw of the edited type, and not the sky");
        let features = renderer.lane_features();
        let (stock, variant) = cx.with_vm(|vm| build(vm, ModelLane::Pbr, features)).expect("the edited lane builds");
        assert!(stock != before.0 && variant != before.1, "premise: the edit is other code");
        assert_eq!(edited.0, stock, "the held draw takes the edited default");
        assert_eq!(edited.1, variant, "the lane draws the edited default's variant from the next frame on");
        assert_ne!(edited.2, before.2, "and its variant without clip");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_tier_change_rebuilds_only_the_splat_programs_in_place() {
        use crate::quality_tier::GraphicsTier;
        use std::collections::BTreeSet;

        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert_eq!(GraphicsTier::Medium.apply_sampler_clamp(&mut cx), 0,
            "applying the startup clamp before registration rebuilds no shaders");
        cx.with_vm(register_test_shaders);
        let mut renderer = Renderer::default();
        let sets = renderer.world_frame_features();
        let receivers = renderer.gi.receivers();

        // Use the same feature sets and variant builder as prepare_lane_variant,
        // including stock, clipped and opaque programs for each of these lanes.
        let programs = |renderer: &mut Renderer, cx: &mut Cx, lane: ModelLane| {
            let mut ids = Vec::new();
            for &features in &sets {
                for features in std::iter::once(features).chain(features.with_gi_screen(lane, receivers)) {
                    for clip in [true, false] {
                        let id = renderer.variant(cx, lane, Features { clip, ..features })
                            .expect("the staged lane variant must have the stock layout");
                        ids.push(id);
                    }
                }
            }
            ids.push(renderer.lane_variants.stock[&lane]);
            ids
        };
        let splat = programs(&mut renderer, &mut cx, ModelLane::Splat);
        let pbr = programs(&mut renderer, &mut cx, ModelLane::Pbr);
        let splat_indices: BTreeSet<_> = splat.iter().map(|id| id.index).collect();
        let check_splat = |cx: &Cx, clamp| {
            for &index in &splat_indices {
                let samplers = &cx.draw_shaders[index].mapping.samplers;
                assert!(samplers.iter().any(|s| s.anisotropy == clamp)
                    && samplers.iter().all(|s| s.anisotropy == 1 || s.anisotropy == clamp),
                    "splat program {index} must use {clamp}x for every anisotropic sampler");
            }
        };
        let count = cx.draw_shaders.shaders.len();
        // Replacing a mapping assigns a fresh scope_uniforms_gen, even before
        // the backend creates a pipeline.
        let pbr_generations: Vec<_> = pbr.iter().map(|id| cx.draw_shaders[id.index].mapping.scope_uniforms_gen).collect();
        cx.draw_shaders.compile_set.clear();

        renderer.set_graphics_tier(GraphicsTier::Medium);
        assert_eq!(renderer.apply_pending_sampler_clamp(&mut cx), 0,
            "applying the already staged Medium tier must rebuild no programs");
        assert!(cx.draw_shaders.compile_set.is_empty(),
            "the Medium startup clamp must queue no pipeline a second time");
        check_splat(&cx, 8);
        renderer.set_graphics_tier(GraphicsTier::High);
        assert_eq!(renderer.apply_pending_sampler_clamp(&mut cx), splat_indices.len(),
            "Medium to High must rebuild each distinct splat program exactly once");
        assert_eq!(programs(&mut renderer, &mut cx, ModelLane::Splat), splat,
            "each staged splat shader must retain its ID across the tier change");
        check_splat(&cx, 16);
        assert_eq!(programs(&mut renderer, &mut cx, ModelLane::Pbr), pbr,
            "each staged PBR shader must retain its ID across the tier change");
        for (id, generation) in pbr.iter().zip(pbr_generations) {
            assert_eq!(cx.draw_shaders[id.index].mapping.scope_uniforms_gen, generation,
                "PBR shader {} must retain its original mapping", id.index);
        }
        assert_eq!(cx.draw_shaders.compile_set, splat_indices,
            "only the changed splat pipelines should be queued");
        assert_eq!(cx.draw_shaders.shaders.len(), count,
            "a sampler clamp change must preserve the total program count");
    }

    #[test]
    fn a_view_binds_the_cascades_its_drawings_scene_draw_fitted_and_no_others() {
        let mut cx = shader_cx();
        let mut renderer = Renderer::default();
        renderer.gpu_baker.set_mode(crate::gpu_lightmap::GpuLightmapMode::Realtime);
        let mut world = World::default();
        world.sun.dir = Some(vec3f(0.0, 1.0, 0.0));
        let (scene_pass, view_pass, aux_pass) = (DrawPass::new(&mut cx), DrawPass::new(&mut cx), DrawPass::new(&mut cx));
        let mut scene_list = DrawList::new(&mut cx);
        let mut view_list = DrawList::new(&mut cx);
        let mut aux_list = DrawList::new(&mut cx);
        let mut sky = cx.with_vm(|vm| DrawSceneSky::script_new_with_default(vm));
        let mut draws = SceneDraws {
            sky: &mut sky, sky_analytic: None, cube: None, alpha: None, terrain: None,
            shadow: None, firework: None, flare: None, shadow_sdf: None, water: None,
            screen: None, screen_instances: &[], view_model: None,
        };
        let state = SceneState3D {
            projection: Mat4f::perspective(70.0, 1.5, 0.1, 200.0), ..Default::default()
        };
        let mut reflected = state;
        reflected.camera_pos.x = 5.0;
        reflected.view.v[12] = -reflected.camera_pos.x;
        let lamp = |x| crate::lightmap::LmLight::omni(vec3f(x, 0.0, -8.0), Vec3f::all(1.0), 4.0);
        renderer.set_clustered_lighting(true);
        renderer.set_hdr_output(true);
        renderer.set_cluster_config(crate::clustered::ClusterConfig {
            tiles_x: 2, tiles_y: 1, slices: 2, ..Default::default()
        });
        renderer.add_frame_lights(vec![lamp(0.0)]);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        draw.begin_pass(&scene_pass, Some(1.0));
        renderer.draw_scene(&mut Cx3d::new(&mut draw), &mut scene_list, &mut draws, &world, state);
        assert_eq!(renderer.frame_lights.len(), 1, "the scene picks up the first queued lamp");
        let scene_lamps = format!("{:?}", renderer.frame_lights);
        renderer.add_frame_lights(vec![lamp(5.0)]);
        let (scene, scene_map, _) = renderer.gpu_baker.csm_binding().unwrap();
        assert!(scene.on, "the scene's draw binds its cascades");
        draw.end_pass(&scene_pass);
        draw.begin_pass(&view_pass, Some(1.0));
        assert!(renderer.bind_view_cascades(&mut draw), "a view after its drawing's scene draw binds the scene's cascades");
        let (view, view_map, _) = renderer.gpu_baker.csm_binding().unwrap();
        assert!(view.on && view_map.texture_id() == scene_map.texture_id(), "the view samples the scene's atlas");
        assert_eq!(format!("{view:?}"), format!("{scene:?}"), "the view binds the scene's fit, not one of its own");
        renderer.draw_scene_view(&mut Cx3d::new(&mut draw), &mut view_list, &mut draws, &world, reflected, None);
        assert_eq!(format!("{:?}", renderer.frame_lights), scene_lamps, "the view preserves the scene's light list");
        assert_eq!(renderer.host_lights.len(), 1, "the view leaves the next scene's host lamp queued");
        draw.end_pass(&view_pass);
        draw.begin_pass(&aux_pass, Some(1.0));
        renderer.draw_scene_aux(&mut Cx3d::new(&mut draw), &mut aux_list, &mut draws, &world, reflected, None);
        assert_eq!(format!("{:?}", renderer.frame_lights), scene_lamps, "the aux draw preserves the scene's light list");
        assert_eq!(renderer.host_lights.len(), 1, "the aux draw leaves the next scene's host lamp queued");
        draw.end_pass(&aux_pass);
        drop(draw);

        // The next drawing: a view recorded before any scene draw in it.
        let mut draw = CxDraw::new(&mut cx, &event);
        draw.begin_pass(&view_pass, Some(1.0));
        assert!(!renderer.bind_view_cascades(&mut draw), "a view with no scene draw before it in its drawing binds no cascades");
        assert!(!renderer.gpu_baker.csm_binding().unwrap().0.on, "it takes the off binding, never the last drawing's fit");
        renderer.draw_scene_view(&mut Cx3d::new(&mut draw), &mut view_list, &mut draws, &world, reflected, None);
        assert_eq!(format!("{:?}", renderer.frame_lights), scene_lamps, "an early view keeps the previous scene's lights");
        draw.end_pass(&view_pass);
        draw.begin_pass(&scene_pass, Some(1.0));
        renderer.draw_scene(&mut Cx3d::new(&mut draw), &mut scene_list, &mut draws, &world, state);
        assert!(renderer.host_lights.is_empty(), "the next scene consumes the pending host lamp");
        assert_eq!(renderer.frame_lights.len(), 1, "the queued lamp replaces the previous frame's lamp");
        assert_eq!(renderer.frame_lights[0].pos, lamp(5.0).pos, "the scene receives the lamp that survived both secondary draws");
        draw.end_pass(&scene_pass);
    }

    #[test]
    fn every_lane_builds_its_variants_with_the_stock_layout() {
        let mut cx = shader_cx();
        // Settled stages one cascade variant even across dusk and dawn.
        // The stock draw's uniform follows the sun, including without an atlas.
        use crate::gpu_lightmap::GpuLightmapMode::{Off, OnChange, Realtime};
        let mut renderer = Renderer::default();
        let mut material = cx.with_vm(|vm| DrawScenePbr::script_new_with_default(vm));
        let staged = |r: &Renderer| r.world_frame_features().into_iter().map(|f| (f.csm, f.settled)).collect::<Vec<_>>();
        renderer.gpu_baker.set_mode(OnChange);
        assert_eq!(staged(&renderer), [(true, true)]);
        let settled_features = renderer.lane_features();
        assert!(settled_features.csm && settled_features.settled);
        let parent = DrawPass::new(&mut cx);
        let mut world = World::default();
        for (mode, sun_y, expected, sets) in [
            (OnChange, 1.0, 2.0, &[(true, true)][..]),
            (OnChange, -1.0, 0.0, &[(true, true)][..]),
            (OnChange, 1.0, 2.0, &[(true, true)][..]),
            (Realtime, 1.0, 1.0, &[(true, false), (false, false)][..]),
            (Realtime, -1.0, 0.0, &[(true, false), (false, false)][..]),
            (Off, 1.0, 0.0, &[(false, false)][..]),
        ] {
            renderer.gpu_baker.set_mode(mode);
            world.sun.dir = Some(vec3f(0.0, sun_y, 0.0));
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            draw.begin_pass(&parent, Some(1.0));
            renderer.run_gpu_lightmap(&mut draw, &world, &[], None, Vec3f::default());
            if mode == Realtime && sun_y > 0.0 {
                renderer.set_scene_time(draw.cx, Some(2.0));
                renderer.apply_scene_time(draw.cx, &parent);
                let lamp = crate::lightmap::LmLight {
                    pos: vec3f(0.0, 3.0, 0.0), radius: 12.0, color: Vec3f::all(1.0),
                    dir: vec3f(0.0, 0.0, -1.0), cone: Some((10.0, 25.0)), shadows: true,
                    ..Default::default()
                };
                renderer.clustered.shadows.render(&mut draw, &[lamp], &[true], Vec3f::default(), &[], &[]);
                let recorded: Vec<_> = draw.passes.id_iter().filter(|&id|
                    id == parent.draw_pass_id() || draw.passes[id].main_draw_list_id.is_some()
                ).collect();
                assert!(recorded.iter().any(|&id| draw.passes[id].debug_name == "local_shadows"),
                    "the first shadowed lamp records an atlas after the clock was supplied");
                let check_clocks = |cx: &Cx, time| {
                    for &id in &recorded {
                        assert_eq!(cx.passes[id].own_time, time, "clock on {}", cx.passes[id].debug_name);
                    }
                };
                check_clocks(draw.cx, Some(2.0));
                for time in [Some(3.0), None] {
                    renderer.set_scene_time(draw.cx, time);
                    renderer.apply_scene_time(draw.cx, &parent);
                    check_clocks(draw.cx, time);
                }
            }
            draw.end_pass(&parent);
            assert_eq!(staged(&renderer), sets);
            if mode == OnChange {
                assert_eq!(renderer.lane_features(), settled_features, "dusk and dawn keep the staged variants");
            } else {
                assert_eq!(renderer.lane_features().csm, expected != 0.0);
            }
            let vars = &mut material.skinned.draw_vars;
            renderer.bind_sun_cascades(draw.cx, vars);
            let mut x = [f32::NAN];
            vars.get_uniform(draw.cx, live_id!(csm_p), &mut x);
            assert_eq!(x[0], expected, "{mode:?}, sun y={sun_y}");
        }
        renderer.gpu_baker.set_mode(OnChange);
        assert!(renderer.lane_features().csm, "the settled variant is selected before target allocation");
        assert!(!renderer.gpu_baker.csm_binding().unwrap().0.on, "Off must have released the targets in the no-atlas draw path");
        renderer.gi.install_screen_fill_for_test(&mut cx, Vec3f::default());
        for receivers in [GiReceivers::Fast, GiReceivers::Exact] {
            renderer.set_gi_receivers(receivers);
            let features = renderer.lane_features();
            assert!(features.gi && !features.gi_screen, "{receivers:?}: the lane shader must receive GI in place");
        }
        cx.with_vm(|vm| {
            let mut checked = std::collections::HashSet::new();
            let lit_world = Features { gi: true, gi_screen: true, csm: true, settled: true, cluster: true, local_shadows: true, reflections: true, ..Default::default() };
            // `build` checks the layout against the stock shader itself; the
            // GLSL program must define every function it calls (a stub
            // shared by two names was emitted under one of them only).
            for features in [
                Features::default(),
                Features { clip: true, ..Default::default() },
                Features { gi: true, ..Default::default() },
                Features { csm: true, cluster: true, ..Default::default() },
                Features { csm: true, settled: true, cluster: true, ..Default::default() },
                Features { csm: true, settled: true, cluster: true, reflections: true, ..Default::default() },
                // A lit world's own sets: GI with reflections, the opaque draw and the clipped one.
                lit_world,
                Features { clip: true, ..lit_world },
                Features { settled: false, ..lit_world },
                Features { gi_screen: false, ..lit_world },
                Features { gi: true, gi_screen: true, gi_debug: true, csm: true, settled: false, csm_debug: true, cluster: true, local_shadows: true, clip: false, reflections: false },
            ] {
                for (lane, _) in STAGED_LANES {
                    if features.reflections && !matches!(lane, ModelLane::Pbr | ModelLane::Splat) { continue }
                    let name = lane.name();
                    let (stock_id, variant_id) = build(vm, lane, features)
                        .unwrap_or_else(|| panic!("{name} variant {features:?}"));
                    let base = vm.heap().type_default_for_id(lane.type_id()).unwrap();
                    if checked.insert(stock_id.index) {
                        crate::shaders::check_scene_backends(vm, base, name);
                        crate::shaders::compile_scene_webgl2(vm, base, name);
                    }
                    for lit in [true, false] {
                        let Some(obj) = variant_object(vm, base, features, lit) else { continue };
                        let label = format!("{name} variant {features:?} lit={lit}");
                        let id = if lit { variant_id } else { lane.shader(vm, Some(obj)).expect("unlit variant") };
                        if checked.insert(id.index) {
                            crate::shaders::check_scene_backends(vm, obj, &label);
                        }
                        let compiled = crate::shaders::compile_scene_webgl2(vm, obj, &label);
                        let view = compiled.view();
                        let source = [view.glsl_vertex, view.glsl_fragment].concat();
                        // GLSL names the installed body after the hook. Only the hoisted body fetches GI here.
                        let hoisted = source.split_once("vec4 io_gi_receive_early(")
                            .and_then(|(_, body)| body.split_once("\n}"))
                            .is_some_and(|(body, _)| body.contains("io_gi_screen_fetch("));
                        assert_eq!(hoisted, lane == ModelLane::Splat && lit && features.gi && features.gi_screen,
                            "{name} variant {features:?} lit {lit}: only the lit splat screen-cache variant should fetch GI in the early hook");
                        if features.settled {
                            assert!(!source.contains("io_csm_disk"), "{name}: settled shader includes the disk filter");
                            // Every read of the mode must compare it. Using it directly as
                            // a mix weight extrapolates a settled shadow below zero.
                            for after in source.split("uni_csm_p.x").skip(1) {
                                let after = after.trim_start();
                                assert!(after.starts_with('<') || after.starts_with(">="),
                                    "{name}: settled GLSL uses the raw cascade mode");
                            }
                            if lit && matches!(lane, ModelLane::Pbr | ModelLane::Diffuse | ModelLane::Splat) {
                                assert!(source.contains("uni_csm_p.x >= 1.5"), "{name}: model receiver must distinguish settled copies");
                            }
                            if lit && matches!(lane, ModelLane::Cube | ModelLane::Alpha | ModelLane::Terrain | ModelLane::SkinnedGpu) {
                                assert!(source.contains("min(p_field, p_cascade)"), "{name}: settled ground receiver must keep the darker visibility");
                            }
                        }
                        // GI off compiles neither the probe gather nor the screen cache; on, the
                        // model lanes' receivers read the cache.
                        if !features.gi {
                            assert!(!source.contains("io_gi_gather") && !source.contains("io_gi_screen_fetch"), "{name} variant {features:?} lit {lit}: GI code with GI off");
                        } else if lit && matches!(lane, ModelLane::Pbr | ModelLane::Splat) {
                            assert!(source.contains("io_gi_screen_fetch"), "{name} variant {features:?}: the receiver must read the screen cache");
                            // Its taps read level zero: implicit gradients are undefined in its loop.
                            let fetch: Vec<&str> = source.lines()
                                .skip_while(|l| !(l.trim_end().ends_with('{') && l.split_whitespace().nth(1).is_some_and(|w| w.starts_with("io_gi_screen_fetch("))))
                                .skip(1).take_while(|l| l.trim_end() != "}").collect();
                            assert!(fetch.iter().any(|l| l.contains("sample2d_lod(")) && !fetch.iter().any(|l| l.contains("sample2d(")),
                                "{name} variant {features:?}: the screen cache fetch must read level zero");
                        }
                        // The splat's reflection outputs carry the layer gate: without it, dry ground
                        // traces wherever the normal map lifts a texel over the floor.
                        if lane == ModelLane::Splat && lit && features.reflections {
                            assert!(source.contains("uni_splat_ssr_floor"), "{name} variant {features:?}: the reflection outputs must take the layer's gloss gate");
                        }
                        if lane == ModelLane::Pbr && lit && features.csm {
                            assert!(source.contains("io_csm_tap1"), "lit PBR must sample cascade depth");
                            assert_eq!(source.contains("io_csm_disk"), !features.settled, "only Realtime PBR uses the disk filter");
                        }
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
