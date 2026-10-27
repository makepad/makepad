//! Stock PBR props under image-based lighting (phase 3 of the HDRI plan).
//!
//! A shiny placed model with no material of its own (none named, or one
//! that is not installed: `has_own_material`) draws on the PBR lane
//! (`ModelDraw::Pbr`) until a world names an environment. Then it draws
//! through `__stock_ibl`: an engine-owned Splash material program that is
//! the PBR lane with `Builtin::Ibl` installed (custom_material.rs) and
//! nothing else — no hooks, the stock shadow caster, nothing that cuts a
//! pixel (its opaque draws take the variant without `clip`, as the PBR
//! lane's do). `ModelDraw::Custom` walks the same instance loop as the PBR
//! lane, so the model keeps its textures (bound by name), its morph
//! targets and its generated skin surface; slot 5 receives the atlas as it
//! does for every IBL material (draw_models.rs).
//!
//! The program is built on first use, once, and stays installed until the
//! host drops its materials (`retain_custom_materials`, `enter_realm`): the
//! next frame under an environment builds it again, and a build that
//! failed is made again only then (`stock_ibl_forget_failure`). Whether
//! this frame routes through it is decided after the environment is
//! resolved (`prepare_stock_ibl`, in `draw_scene_scoped` after
//! `resolve_ibl_for`), from the IBL texture (the bound preparation's; a
//! fork's is the live renderer's, mirrored) and the shader on the
//! program's draw, and decided again in each model list's diffuse pass,
//! before any lane walks that list (`confirm_stock_ibl`, beside
//! `pbr_ready`): only there are this frame's features known, and the lanes
//! draw through the program's variant for them, a shader of its own, so the
//! models leave the PBR lane only once that variant's pipeline is ready.
//! Without an environment nothing is built and both lanes take exactly the
//! instances they took before.
//!
//! The lanes that take the IBL lookups (this program, the skin lane's
//! surface path, the IBL item materials and a host's IBL material) read
//! one control, `ibl_ctl`, resolved per draw where the frame makes its rig
//! (`resolve_ibl_lane`) and written when a lane binds (`ibl_lane_ctl`):
//! - y, the lane's scale on both lookups: 1 under HDR output, the map's own
//!   exposure in the legacy lane (`sun::env_lane_scale`, the factor the
//!   rig's fill is made with), so everything in a lane carries one
//!   exposure. The shared `mat_ibl_*` functions stay at the map's own scale:
//!   the dome applies its own exposure, a legacy fork draws the live HDR
//!   renderer's texture (the meta row cannot carry a lane's value), and a
//!   preview reads them as they are.
//! - z, the fill from the map. Cleared on the stock lanes (this program and
//!   the skin lane) while the world authors its fill (`SunConfig.ambient`,
//!   a `Light::Sky`: `sun::fill_is_authored`) or fast GI gathers one for
//!   the draw: they keep the fill they had before they took the lookups
//!   and take the reflection alone from the map. An item's or a host's IBL
//!   material has no such fill: it always fills from the map.
use super::*;
use crate::custom_material::DrawSceneCustom;
use makepad_render_material::{HookMask, HookSet, MaterialDesc};

/// The custom-material name the stock IBL program is installed under. An
/// engine name (the `__item_*` convention): a host never installs it.
pub const STOCK_IBL_MATERIAL: &str = "__stock_ibl";

pub(super) struct StockIblState {
    /// The program did not build (logged once). It is not built again
    /// until the host drops its materials (`stock_ibl_forget_failure`).
    failed: bool,
    /// This frame routes shiny stock models through the program.
    active: bool,
    /// The scale this draw's lane puts on the IBL lookups
    /// (`sun::env_lane_scale`; `resolve_ibl_lane`).
    scale: f32,
    /// This draw's world authors its fill (`sun::fill_is_authored`): the
    /// stock lanes keep it.
    authored_fill: bool,
}

impl Default for StockIblState {
    fn default() -> Self {
        // No draw yet: the lookups at the map's own scale, the fill the map's.
        Self { failed: false, active: false, scale: 1.0, authored_fill: false }
    }
}

impl Renderer {
    /// Install `__stock_ibl` when it is missing (and its build did not fail
    /// since the host last dropped its materials). True when it is
    /// installed after the call. A held VM (a script-driven draw mid-apply)
    /// means "next frame", not failure.
    pub(super) fn stock_ibl_install(&mut self, cx: &mut Cx) -> bool {
        if self.custom_material_shader(STOCK_IBL_MATERIAL).is_some() {
            return true;
        }
        if self.stock_ibl.failed {
            return false;
        }
        // The PBR lane with IBL and nothing else: the stock bodies of every
        // hook, so a recoloured car keeps its clear coat and its maps.
        let desc = MaterialDesc { ibl: true, ..Default::default() };
        match cx.try_with_vm(|vm| DrawSceneCustom::build(vm, &desc, &HookSet::new(), HookMask::ALL, Vec4f::default())) {
            Some(Ok(material)) => {
                // A program the lane cannot instance is a failure too:
                // retried, it would be built again on every frame.
                let installed = self.install_custom_material(STOCK_IBL_MATERIAL.to_string(), material);
                if !installed {
                    log!("render: the stock IBL material has no shader, shiny models keep the analytic sky");
                    self.stock_ibl.failed = true;
                }
                installed
            }
            Some(Err(e)) => {
                log!("render: the stock IBL material did not build, shiny models keep the analytic sky: {e}");
                self.stock_ibl.failed = true;
                false
            }
            None => false,
        }
    }

    /// Forget a build that failed, where the host drops its materials
    /// (`retain_custom_materials`: a script reload; `enter_realm`: another
    /// world): what did not build may build now, so the next frame under
    /// an environment tries once more. Until then the failure stays
    /// remembered and no frame builds (and logs) it again.
    pub(super) fn stock_ibl_forget_failure(&mut self) {
        self.stock_ibl.failed = false;
    }

    /// Decide, once per frame after the environment is resolved, whether
    /// shiny stock models draw through the program this frame: the PBR lane
    /// is enabled, an IBL texture is bound (the bound preparation's, which
    /// stays while the next prepares; a fork's, mirrored from the live
    /// renderer) and the shader on the program's draw is ready. Without an
    /// environment the program is never built: the short-circuit is the
    /// no-change guarantee. The shader on the draw is the variant the last
    /// list decided from (the stock shader before the first), not
    /// necessarily the one this frame's lanes draw through: this frame's
    /// features are known only once the lights are clustered, later in the
    /// frame. So each list's diffuse pass decides again (`confirm_stock_ibl`)
    /// before any lane walks that list.
    pub(super) fn prepare_stock_ibl(&mut self, cx: &mut Cx) {
        self.stock_ibl.active = self.pbr_materials_enabled
            && self.ibl_texture().is_some()
            && self.stock_ibl_install(cx)
            && self.stock_ibl_shader_ready(cx);
    }

    /// Decide again in a model list's diffuse pass, beside `pbr_ready`
    /// (draw_models.rs) and before any lane walks the list, now that the
    /// frame's features are known: the PBR lane is enabled, an IBL texture
    /// is bound, the program is installed and its variant for this frame's
    /// features (`CustomMaterial::shaders`, built on first use: a shader of
    /// its own, whose pipeline the HAL compiles apart) is ready. Otherwise the
    /// models stay on the PBR lane, as `pbr_ready` keeps them on the
    /// diffuse lane while the PBR lane's variant compiles. The variant goes
    /// on the program's draw, where the program's own pass puts it too, so
    /// `stock_ibl_ready` (`items_ready`) reads the shader the lane draws
    /// through. Installs nothing: a program `prepare_stock_ibl` did not
    /// install is no route.
    pub(super) fn confirm_stock_ibl(&mut self, cx: &mut Cx) {
        if !self.pbr_materials_enabled || self.ibl_texture().is_none() {
            self.stock_ibl.active = false;
            return;
        }
        let (features, hdr) = (self.lane_features(), self.hdr_output);
        self.stock_ibl.active = self.custom_draws.get_mut(STOCK_IBL_MATERIAL).is_some_and(|m| {
            let full = m.shaders(cx, features).0;
            m.draw.draw_vars.draw_shader_id = full;
            full.is_some_and(|id| pipeline_ready(cx, id, hdr))
        });
    }

    /// This frame's decision (`prepare_stock_ibl`, then the list's diffuse
    /// pass: `confirm_stock_ibl`), read by the lanes.
    pub(super) fn stock_ibl_active(&self) -> bool {
        self.stock_ibl.active
    }

    /// Take this draw's control of the lanes that read the IBL lookups, at
    /// the frame's rig site (frame.rs), from the lighting the rig takes
    /// from the environment: the lane's scale and whether the world authors
    /// its fill. Every draw resolves its own (an aux draw's world can name
    /// no environment; a fork draws in its own lane).
    pub(super) fn resolve_ibl_lane(&mut self, world: &World, env: Option<&crate::sun::EnvLighting>) {
        self.stock_ibl.scale = crate::sun::env_lane_scale(env, self.hdr_output);
        self.stock_ibl.authored_fill = crate::sun::fill_is_authored(world);
    }

    /// `ibl_ctl` for a draw that takes the IBL lookups (the lane functions
    /// of render-material read it): x = 1, the lookups are on (the skin
    /// lane's switch; a PBR-family IBL program is one by construction);
    /// y = the lane's scale on both lookups (`sun::env_lane_scale`: the
    /// exposure the rig's fill carries); z = 1 while the fill comes from the
    /// map, 0 while a stock lane keeps the fill it had: the world authors
    /// one (`sun::fill_is_authored`), or fast GI gathers one for this draw
    /// (`gi_on`, as `gi.bind` just wrote it on `vars`: bind the GI first).
    /// Only the stock lanes (`stock_lane`: the stock IBL program, the skin
    /// lane) have a fill of their own to keep; an item's or a host's IBL
    /// material always fills from the map.
    pub(super) fn ibl_lane_ctl(&self, cx: &Cx, vars: &DrawVars, stock_lane: bool) -> [f32; 4] {
        // `gi_ambient` returns the gathered field exactly while gi_on is
        // above 0 (fast_gi/shaders.rs).
        let gathered = vars.uniform_range(cx, live_id!(gi_on)).is_some_and(|(at, _)| vars.dyn_uniforms.get(at).is_some_and(|on| *on > 0.0));
        let from_map = !(stock_lane && (self.stock_ibl.authored_fill || gathered));
        [1.0, self.stock_ibl.scale, if from_map { 1.0 } else { 0.0 }, 0.0]
    }

    /// Write `ibl_ctl` on the draw of the PBR-family IBL program `name`
    /// (`bind_model_lane`, after the GI is bound).
    pub(super) fn bind_ibl_lane(&self, cx: &Cx, name: &str, vars: &mut DrawVars) {
        let ctl = self.ibl_lane_ctl(cx, vars, name == STOCK_IBL_MATERIAL);
        vars.set_uniform(cx, live_id!(ibl_ctl), &ctl);
    }

    /// Whether `inst` has a material of its own for the lanes' routing: it
    /// names one and that one is installed. `lane` is the program walking
    /// the list, whose draw is out of `custom_draws` for its pass
    /// (`draw_custom_models`). A name that is not installed (a replica that
    /// installs no materials, one dropped by `retain_custom_materials`)
    /// draws through the stock lanes, so it routes as a stock model.
    pub(super) fn has_own_material(&self, inst: &ModelInstance, lane: Option<&str>) -> bool {
        inst.custom_material.as_ref().is_some_and(|m| lane == Some(m.name.as_str()) || self.custom_draws.contains_key(&m.name))
    }

    /// Whether `instances` lists a model the stock IBL program draws while
    /// the frame routes: a shiny one with no material of its own
    /// (`draw_custom_models` draws the program only then). The per-instance
    /// routing (`takes_stock_ibl`) reads the same `has_own_material`, so a
    /// model the PBR lane leaves is always one this lists.
    pub(super) fn lists_a_stock_ibl_model(&self, instances: &[ModelInstance]) -> bool {
        let shiny: std::collections::HashSet<&str> = self.static_models.iter()
            .filter(|(_, m)| m.wants_pbr).map(|(k, _)| k.as_str()).collect();
        !shiny.is_empty() && instances.iter().any(|inst| !self.has_own_material(inst, None) && shiny.contains(inst.model.as_str()))
    }

    /// Whether a locked-time host has nothing to wait for on this path: no
    /// environment, the PBR lane off, no program installed, or the pipeline
    /// of the shader on the program's draw ready, the variant the lanes
    /// last drew through (`items_ready`). Not installed is nothing
    /// to wait for, as for the items' materials and the PBR lane there: the
    /// scene draw that binds an environment installs the program before any
    /// lane walks the models, so after a draw it is missing only when it did
    /// not build (those models stay on the PBR lane until the host drops its
    /// materials: `stock_ibl_forget_failure`) or the VM was held (as a held
    /// VM leaves the PBR lane unmade).
    pub(super) fn stock_ibl_ready(&self, cx: &Cx) -> bool {
        self.ibl_texture().is_none()
            || !self.pbr_materials_enabled
            || !self.custom_draws.contains_key(STOCK_IBL_MATERIAL)
            || self.stock_ibl_shader_ready(cx)
    }

    /// Whether the shader on the program's draw can draw: the variant the
    /// last diffuse pass decided from (`confirm_stock_ibl` puts it there,
    /// the program's own pass the same one), so within a list the shader
    /// the lanes draw through, and the stock shader before the first
    /// diffuse pass. Read before the walk it is the last list's variant, a
    /// first estimate only (`prepare_stock_ibl`).
    fn stock_ibl_shader_ready(&self, cx: &Cx) -> bool {
        self.custom_draws
            .get(STOCK_IBL_MATERIAL)
            .and_then(|m| m.draw.draw_vars.draw_shader_id)
            .is_some_and(|id| pipeline_ready(cx, id, self.hdr_output))
    }
}

/// Whether the pipeline of `id` can draw (`Cx::draw_shader_ready`): what
/// both decisions and `stock_ibl_ready` ask of the HAL. A test runs no
/// beat, so there the HAL makes no pipeline and every decision would only
/// wait; it names the pipelines it treats as made (`tests::Made`), so the
/// route it decides is seen taken as well as waited for.
fn pipeline_ready(cx: &Cx, id: DrawShaderId, hdr: bool) -> bool {
    #[cfg(test)]
    if tests::MADE.with(|made| made.borrow().contains(&id.index)) {
        return true;
    }
    cx.draw_shader_ready(id, hdr)
}

/// Whether one instance draws through the stock IBL program this frame:
/// the frame routes (`active`), the model wants the PBR lane, it has no
/// material of its own (`Renderer::has_own_material`: none named, or the
/// named one not installed) and it is not swaying foliage (the foliage lane
/// keeps those). Read by every lane, so exactly one of them draws the
/// instance.
pub(super) fn takes_stock_ibl(active: bool, uses_pbr_lane: bool, own_material: bool, sways: bool) -> bool {
    active && uses_pbr_lane && !own_material && !sways
}

/// Whether a stock lane (diffuse, PBR or foliage) skips one instance. With
/// `takes_stock_ibl` false this is the filter the lanes had before.
pub(super) fn stock_lane_skips(wanted_custom: bool, takes_stock_ibl: bool, sways: bool, foliage_lane: bool, uses_pbr_lane: bool, pbr_lane: bool) -> bool {
    wanted_custom || takes_stock_ibl || sways != foliage_lane || (!foliage_lane && uses_pbr_lane != pbr_lane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_draw::makepad_platform::makepad_script::script_eval;
    use makepad_render_material::{Builtin, ShadowVariant};

    thread_local! {
        /// The shaders (by index) whose pipelines this thread's test treats
        /// as made ([`Made`], read by `pipeline_ready`).
        pub(super) static MADE: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The pipelines a test treats as made. A test runs no beat, so the HAL
    /// makes none and every decision would only ever wait: a test names
    /// the pipelines the HAL has made to see the route taken too. None to
    /// begin with, and forgotten when the test ends (the thread may run
    /// another).
    struct Made;

    impl Made {
        fn none() -> Made {
            MADE.with(|made| made.borrow_mut().clear());
            Made
        }

        /// The HAL has made the pipeline of `id`.
        fn pipeline(&self, id: DrawShaderId) {
            MADE.with(|made| made.borrow_mut().push(id.index));
        }
    }

    impl Drop for Made {
        fn drop(&mut self) {
            MADE.with(|made| made.borrow_mut().clear());
        }
    }

    /// A headless Cx whose VM holds the scene shaders and the material
    /// built-ins (the custom_material.rs test rig).
    fn headless() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            vm.capture_errors();
            makepad_draw::script_mod(vm);
            vm.heap_mut().new_module(id!(prelude));
            script_eval!(vm, { mod.prelude.widgets_internal = { ..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw } });
            vm.heap_mut().new_module(id!(widgets));
            crate::custom_material::register(vm);
        });
        cx
    }

    /// Where the installed program lives. Its Box keeps this address for as
    /// long as it stays installed (a lane's pass takes it out and puts the
    /// same Box back), and an install puts another Box in its place before
    /// the old one is freed: the address is the same exactly when no
    /// program was built and installed in between. The shader id cannot
    /// tell: the same program text gives the same shader.
    fn program_at(renderer: &Renderer) -> Option<*const CustomMaterial> {
        renderer.custom_material(STOCK_IBL_MATERIAL).map(|m| m as *const CustomMaterial)
    }

    #[test]
    fn only_a_shiny_stock_model_under_an_environment_takes_the_program() {
        // (active, uses_pbr_lane, own_material, sways)
        assert!(takes_stock_ibl(true, true, false, false));
        assert!(!takes_stock_ibl(false, true, false, false), "no environment: the PBR lane");
        assert!(!takes_stock_ibl(true, false, false, false), "a matte model: the diffuse lane");
        assert!(!takes_stock_ibl(true, true, true, false), "its own material: that program");
        assert!(!takes_stock_ibl(true, true, false, true), "swaying: the foliage lane");
    }

    /// The composed lane filter of draw_models_inner, as a truth table: a
    /// stock instance (no material of its own) is drawn by exactly one of
    /// the diffuse, PBR and foliage lanes or the stock IBL program, and
    /// with no environment the filter is the expression the lanes had.
    #[test]
    fn exactly_one_lane_draws_a_stock_instance_and_no_environment_is_the_old_filter() {
        // (foliage_lane, pbr_lane) of the diffuse, PBR and foliage lanes.
        let lanes = [(false, false), (false, true), (true, true)];
        for bits in 0..16u32 {
            let (active, uses_pbr, own, sways) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let takes = takes_stock_ibl(active, uses_pbr, own, sways);
            let stock = lanes.iter().filter(|(foliage, pbr)| !stock_lane_skips(false, takes, sways, *foliage, uses_pbr, *pbr)).count();
            assert_eq!(stock + takes as usize, 1, "active {active} pbr {uses_pbr} own {own} sways {sways}");
            if !active {
                for (foliage, pbr) in lanes {
                    let before = sways != foliage || (!foliage && uses_pbr != pbr);
                    assert_eq!(stock_lane_skips(false, takes, sways, foliage, uses_pbr, pbr), before, "no environment: the lanes' old filter");
                }
            }
            // An instance whose own material is ready leaves every stock lane.
            for (foliage, pbr) in lanes {
                assert!(stock_lane_skips(true, takes, sways, foliage, uses_pbr, pbr), "a ready material of its own: no stock lane");
            }
        }
    }

    #[test]
    fn the_program_is_the_pbr_lane_with_ibl_and_nothing_else() {
        let mut cx = headless();
        let mut renderer = Renderer::default();
        assert!(renderer.stock_ibl_install(&mut cx), "the program builds and installs");
        let built = program_at(&renderer);
        assert!(renderer.stock_ibl_install(&mut cx), "the second call finds it");
        assert_eq!(program_at(&renderer), built, "installed once: the second call builds no other program");
        let m = renderer.custom_material(STOCK_IBL_MATERIAL).expect("installed under its name");
        assert!(m.ibl, "binds the environment on detail_map");
        assert_eq!(m.plan.builtins, vec![Builtin::Ibl], "no hooks: no hooked composition");
        assert_eq!(m.plan.shadow, ShadowVariant::Stock);
        assert!(m.shadow.is_none(), "the stock caster");
        assert!(m.cuts_no_pixel(&cx), "nothing cuts a pixel: its opaque draws take the variant without `clip`");
        assert_eq!(m.bounds_pad, 0.0);
        let id = m.stock_shader().expect("compiled");
        let textures = &cx.draw_shaders[id.index].mapping.textures;
        assert_eq!(textures[0].id, live_id!(tex));
        assert_eq!(textures[5].id, live_id!(detail_map), "the PBR lane's free detail slot");
        assert!(textures.iter().any(|t| t.id == live_id!(orm_map)), "a stock model's ORM binds as on the PBR lane");
        assert!(textures.iter().any(|t| t.id == live_id!(morph_map)), "its morph targets too");
    }

    #[test]
    fn without_an_environment_nothing_is_built_and_the_frame_routes_as_before() {
        let mut cx = headless();
        let mut renderer = Renderer::default();
        renderer.prepare_stock_ibl(&mut cx);
        // The decision off: the lanes' filter is the one they had (the
        // truth table above).
        assert!(!renderer.stock_ibl_active());
        assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_none(), "no environment, no program");
        assert!(renderer.custom_draws.is_empty());
    }

    /// Under a bound environment: before a scene draw has installed the
    /// program there is nothing to wait for (plan 2's environment tests bind
    /// a texture with no scene draw and expect `items_ready`), and with the
    /// PBR lane off the frame neither builds the program nor routes.
    #[test]
    fn under_an_environment_the_pbr_lane_gates_the_program_and_no_draw_yet_waits_for_nothing() {
        use makepad_draw::makepad_platform::thread::ShutdownMode;
        use makepad_render_material::ibl::EnvMap;
        use makepad_scene::{Environment, Ibl, IblSource, TextureRef};
        let made = Made::none();
        let mut cx = headless();
        // A closed pool prepares on the spot: the call binds the texture.
        cx.task_pool().close(ShutdownMode::CancelPending);
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(1), std::sync::Arc::new(EnvMap::constant(16, [0.25; 3])));
        let env = Environment { ibl: Some(Ibl { source: IblSource::Hdri(TextureRef(1)), intensity: 1.0, rotation_deg: 0.0 }), ..Default::default() };
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.ibl_texture().is_some(), "premise: the environment is bound");
        assert!(renderer.stock_ibl_ready(&cx) && renderer.items_ready(&cx), "no draw has installed the program: nothing to wait for");

        renderer.pbr_materials_enabled = false;
        renderer.prepare_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active(), "the PBR lane off: no routing");
        assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_none(), "and no program");

        // The control: the same frame with the lane on builds the program
        // and routes once its pipeline can draw.
        renderer.pbr_materials_enabled = true;
        renderer.prepare_stock_ibl(&mut cx);
        let id = renderer.custom_material_shader(STOCK_IBL_MATERIAL).expect("the lane on: the program is built");
        let active = renderer.stock_ibl_active();
        assert_eq!(active, pipeline_ready(&cx, id, renderer.hdr_output), "the decision is the program's pipeline");
        // Under the HAL a new shader's pipeline is made at the next beat and
        // compiled off the thread on every backend, so in a test, which runs
        // no beat, it cannot draw yet: the frame waits for it.
        assert!(!active, "the lane on and the program's pipeline not made yet: the frame waits for it");
        // Once the HAL has made it, the frame routes.
        made.pipeline(id);
        renderer.prepare_stock_ibl(&mut cx);
        assert!(renderer.stock_ibl_active(), "the lane on and the program's pipeline made: the frame routes");
        renderer.pbr_materials_enabled = false;
        renderer.prepare_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active(), "the PBR lane off: no routing, the pipeline made or not");
    }

    /// The lanes draw the program through its variant for this frame's
    /// features, a shader of its own (`CustomMaterial::shaders`), not the
    /// one `prepare_stock_ibl` reads off the draw before the walk (the
    /// stock shader, or last walk's variant). A list's diffuse pass decides
    /// again from that variant and puts it on the draw, so the lanes,
    /// `stock_ibl_ready` and `items_ready` read one shader. Where a
    /// pipeline compiles apart (every backend under the HAL, Metal before
    /// it) a decision read off the other shader routed the models while
    /// their variant compiled: the PBR lane had left them and the variant's
    /// draw was left out of the frame.
    #[test]
    fn each_list_decides_from_this_frames_variant_and_puts_it_on_the_draw() {
        use makepad_draw::makepad_platform::thread::ShutdownMode;
        use makepad_render_material::ibl::EnvMap;
        use makepad_scene::{Environment, Ibl, IblSource, TextureRef};
        let made = Made::none();
        let mut cx = headless();
        cx.task_pool().close(ShutdownMode::CancelPending);
        let mut renderer = Renderer::default();
        let on_draw = |r: &Renderer| r.custom_material(STOCK_IBL_MATERIAL).and_then(|m| m.draw.draw_vars.draw_shader_id);

        // No environment: the diffuse pass builds nothing and routes nothing.
        renderer.confirm_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active());
        assert!(renderer.custom_draws.is_empty(), "no environment, no program");

        renderer.register_environment(TextureRef(1), std::sync::Arc::new(EnvMap::constant(16, [0.25; 3])));
        let env = Environment { ibl: Some(Ibl { source: IblSource::Hdri(TextureRef(1)), intensity: 1.0, rotation_deg: 0.0 }), ..Default::default() };
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.ibl_texture().is_some(), "premise: the environment is bound");
        renderer.prepare_stock_ibl(&mut cx);
        let stock = renderer.custom_material_shader(STOCK_IBL_MATERIAL).expect("an environment builds the program");
        assert_eq!(on_draw(&renderer), Some(stock), "before its first walk the draw holds the stock shader");
        // A test runs no beat, so the HAL has made no pipeline: the first
        // estimate waits. Once the stock shader's is made it is on.
        assert!(!renderer.stock_ibl_active(), "no pipeline made: the first estimate waits");
        made.pipeline(stock);
        renderer.prepare_stock_ibl(&mut cx);
        assert!(renderer.stock_ibl_active(), "the stock shader on the draw can draw: the first estimate is on");

        renderer.confirm_stock_ibl(&mut cx);
        let features = renderer.lane_features();
        let variant = renderer.custom_draws.get_mut(STOCK_IBL_MATERIAL).unwrap().shaders(&mut cx, features).0.expect("a variant for this frame's features");
        assert_ne!(variant, stock, "premise: this frame's variant is a shader of its own");
        assert_eq!(on_draw(&renderer), Some(variant), "the variant the lanes draw through is on the draw");
        assert_eq!(renderer.stock_ibl_active(), pipeline_ready(&cx, variant, renderer.hdr_output), "the decision is that variant's pipeline");
        // The stock shader's pipeline is made, the variant's not: a decision
        // read off the stock shader would route the models while their
        // variant compiles.
        assert!(!renderer.stock_ibl_active(), "the stock shader made, this frame's variant not: the list waits for the variant");
        assert!(!renderer.stock_ibl_ready(&cx), "the program is waited for on the shader the lanes draw through");
        assert!(!renderer.items_ready(&cx), "and items_ready waits for it");
        // The next frame's first decision reads the variant off the draw.
        renderer.prepare_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active(), "the next frame's first estimate reads the variant on the draw, not the stock shader");

        // The HAL makes the variant's pipeline: the list routes, and nothing
        // is waited for.
        made.pipeline(variant);
        renderer.confirm_stock_ibl(&mut cx);
        assert!(renderer.stock_ibl_active(), "the variant's pipeline made: the list routes");
        assert!(renderer.stock_ibl_ready(&cx) && renderer.items_ready(&cx), "the variant can draw: nothing is waited for");

        // The next frame's first decision reads the variant off the draw;
        // another list's diffuse pass finds it built and decides the same.
        let built = program_at(&renderer);
        renderer.prepare_stock_ibl(&mut cx);
        assert!(renderer.stock_ibl_active(), "the next frame's first estimate: the variant can draw");
        renderer.confirm_stock_ibl(&mut cx);
        assert_eq!(program_at(&renderer), built, "the next frame builds no other program");
        assert_eq!(on_draw(&renderer), Some(variant), "one variant per feature set");
        assert!(renderer.stock_ibl_active());

        // The PBR lane off, or the environment gone: the diffuse pass does
        // not route either.
        renderer.pbr_materials_enabled = false;
        renderer.confirm_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active(), "the PBR lane off: no routing");
        renderer.pbr_materials_enabled = true;
        renderer.resolve_ibl(&mut cx, &Environment::default());
        assert!(renderer.ibl_texture().is_none());
        renderer.confirm_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active(), "no environment: no routing");
    }

    use crate::model::PbrMaterial;

    /// A resident stock model in the fixture form of
    /// prepared_static_preview_tests.rs: one triangle, seven floats a vertex.
    fn triangle(pbr: PbrMaterial) -> StaticModel {
        let mut vertices = Vec::new();
        for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
            vertices.extend_from_slice(&[p[0], p[1], p[2], 0.0, 0.0, f32::from_bits(0xffff_ffff), 0.0]);
        }
        StaticModel {
            vertices, indices: vec![0, 1, 2], texture_uri: None, texture_png: None,
            min: vec3f(0.0, 0.0, 0.0), max: vec3f(1.0, 0.0, 1.0),
            parts: vec![(vec3f(0.0, 0.0, 0.0), vec3f(1.0, 0.0, 1.0))],
            ground_ao: None, draw_layers: Vec::new(), detail_png: None, detail_scale: [1.0, 1.0],
            prelit: false, anim_parts: Vec::new(), driven_parts: Vec::new(), sky: None, liquids: Vec::new(), liquid_ranges: Vec::new(),
            pbr,
        }
    }

    /// Whether the resident model `name` asks for the PBR lane.
    fn wants_pbr(renderer: &Renderer, name: &str) -> bool {
        renderer.static_models.iter().find(|(k, _)| k == name).map(|(_, m)| m.wants_pbr).unwrap()
    }

    /// A renderer with a chrome and a matte stock model resident and placed.
    fn renderer_with_a_chrome_and_a_matte_model(cx: &mut Cx) -> Renderer {
        let mut renderer = Renderer::default();
        let chrome = PbrMaterial { metallic: 1.0, roughness: 0.15, ..Default::default() };
        renderer.load_model_parsed(cx, "test/chrome", triangle(chrome), None, None).expect("the chrome model uploads");
        renderer.load_model_parsed(cx, "test/matte", triangle(PbrMaterial::default()), None, None).expect("the matte model uploads");
        assert!(wants_pbr(&renderer, "test/chrome") && !wants_pbr(&renderer, "test/matte"), "a narrowed roughness is the PBR lane, the default the diffuse lane");
        let instance = |model: &str| ModelInstance { model: model.into(), custom_material: None, transform: Mat4f::identity(), tint: vec4(1.0, 1.0, 1.0, 1.0), color_adjust: vec4(0.0, 1.0, 1.0, 0.0), dynamic: true, depth_order: 0.0, part_poses: Vec::new(), lamps: Default::default() };
        renderer.set_models(vec![instance("test/chrome"), instance("test/matte")]);
        renderer
    }

    /// No environment, a shiny and a matte model resident and placed: the
    /// frame's first estimate (`prepare_stock_ibl`) stays off, which leaves
    /// the lanes' filter the one they had (the truth table above), no
    /// program is built, and `items_ready` has nothing to wait for. The
    /// walk of the placed list through the lanes is not driven here: no
    /// headless test walks an instance list.
    #[test]
    fn without_an_environment_resident_stock_models_build_no_program_and_wait_for_nothing() {
        let mut cx = headless();
        let mut renderer = renderer_with_a_chrome_and_a_matte_model(&mut cx);
        renderer.prepare_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active(), "no environment: the estimate is off");
        assert!(renderer.custom_draws.is_empty(), "no environment: not even the program is built");
        assert!(renderer.items_ready(&cx), "and nothing is waited for");
    }

    /// Under a bound environment, with the same two models resident:
    /// `prepare_stock_ibl`, the frame's first estimate (each list's diffuse
    /// pass decides again, `confirm_stock_ibl`: the test above), is on
    /// exactly while the shader on the program's draw can draw; the program
    /// is built once; `stock_ibl_ready` and `items_ready` wait for that
    /// pipeline; and without the environment the estimate is off again
    /// while the program stays installed. The route predicate is asked
    /// with each model's `wants_pbr` flag, which stands in for the walk's
    /// `uses_pbr_lane`: no headless test walks the instance list, so the
    /// composed route (the lanes' filter in draw_models.rs, the program's
    /// pass in realm.rs) is the lab's pixel test (`--scene=ibl`, columns 4
    /// and 5), run before a merge.
    #[test]
    fn an_environment_turns_the_first_estimate_on_and_the_route_predicate_takes_only_the_shiny_models_flag() {
        use makepad_render_material::ibl::EnvMap;
        use makepad_scene::{Environment, Ibl, IblSource, TextureRef};
        let made = Made::none();
        let mut cx = headless();
        let mut renderer = renderer_with_a_chrome_and_a_matte_model(&mut cx);
        // A closed pool prepares on the spot: the call binds the texture.
        cx.task_pool().close(ShutdownMode::CancelPending);
        renderer.register_environment(TextureRef(1), std::sync::Arc::new(EnvMap::constant(32, [0.5, 0.4, 0.3])));
        let env = Environment { ibl: Some(Ibl { source: IblSource::Hdri(TextureRef(1)), intensity: 1.0, rotation_deg: 0.0 }), ..Default::default() };
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.ibl_texture().is_some(), "premise: the environment is bound");
        renderer.prepare_stock_ibl(&mut cx);
        let id = renderer.custom_material_shader(STOCK_IBL_MATERIAL).expect("an environment builds the program");
        // No beat has made the program's pipeline (the HAL makes it at the
        // next one): the estimate waits for it, and so does items_ready.
        assert!(!renderer.stock_ibl_active(), "an environment and a program whose pipeline is not made yet: the estimate is off");
        assert!(!renderer.stock_ibl_ready(&cx), "the program is waited for while its pipeline cannot draw");
        assert!(!renderer.items_ready(&cx), "and items_ready waits for it");
        // The HAL makes it: the next frame's estimate is on.
        made.pipeline(id);
        renderer.prepare_stock_ibl(&mut cx);
        let active = renderer.stock_ibl_active();
        assert_eq!(active, pipeline_ready(&cx, id, renderer.hdr_output), "the first estimate is the texture plus the pipeline of the shader on the program's draw");
        assert!(active, "an environment and the program's pipeline made: the estimate is on");
        assert!(renderer.stock_ibl_ready(&cx) && renderer.items_ready(&cx), "and nothing is waited for");
        let (chrome, matte) = (wants_pbr(&renderer, "test/chrome"), wants_pbr(&renderer, "test/matte"));
        assert!(takes_stock_ibl(active, chrome, false, false), "the predicate takes the chrome model's flag");
        assert!(!takes_stock_ibl(active, matte, false, false), "and leaves the matte model's to the diffuse lane");
        // Exactly one drawer for each flag: the program or one stock lane.
        for uses_pbr in [chrome, matte] {
            let takes = takes_stock_ibl(active, uses_pbr, false, false);
            let stock = [(false, false), (false, true), (true, true)].iter().filter(|(foliage, pbr)| !stock_lane_skips(false, takes, false, *foliage, uses_pbr, *pbr)).count();
            assert_eq!(stock + takes as usize, 1);
        }
        // Another frame finds the program installed and builds no other.
        let built = program_at(&renderer);
        renderer.prepare_stock_ibl(&mut cx);
        assert_eq!(program_at(&renderer), built, "one program, built once");
        assert_eq!(renderer.custom_material_shader(STOCK_IBL_MATERIAL), Some(id));
        assert_eq!(renderer.custom_draws.len(), 1);
        // No environment again: the estimate is off; the program stays
        // installed for the next one.
        renderer.resolve_ibl(&mut cx, &Environment::default());
        assert!(renderer.ibl_texture().is_none());
        renderer.prepare_stock_ibl(&mut cx);
        assert!(!renderer.stock_ibl_active());
        assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_some());
    }

    /// Decision 9: a build that failed is remembered, so the frames after
    /// it build (and log) nothing, and it is forgotten where the host drops
    /// its materials: `retain_custom_materials` (a script reload) and
    /// `enter_realm` (another world). The next frame under the environment
    /// then builds the program. The failure here is a treeline of
    /// `Builtin::Ibl` whose body does not compile, mended before the next
    /// frame: without the memory that frame would build it.
    #[test]
    fn a_failed_build_is_not_made_again_until_the_host_reloads_its_materials() {
        use makepad_scene::Environment;
        let reloads: [(&str, fn(&mut Renderer)); 2] = [
            ("retain_custom_materials", |renderer| renderer.retain_custom_materials(&[])),
            ("enter_realm", |renderer| renderer.enter_realm()),
        ];
        for (reload, run) in reloads {
            let mut cx = headless();
            cx.task_pool().close(ShutdownMode::CancelPending);
            let mut renderer = Renderer::default();
            renderer.register_environment(TextureRef(1), std::sync::Arc::new(EnvMap::constant(16, [0.25; 3])));
            let env = Environment { ibl: Some(Ibl { source: IblSource::Hdri(TextureRef(1)), intensity: 1.0, rotation_deg: 0.0 }), ..Default::default() };
            renderer.resolve_ibl(&mut cx, &env);
            assert!(renderer.ibl_texture().is_some(), "premise: the environment is bound");

            cx.with_vm(|vm| {
                script_eval!(vm, {
                    use mod.prelude.widgets_internal.*
                    mod.draw.mat_ibl_coat_treeline_kept = mod.draw.mat_ibl_coat_treeline
                    mod.draw.mat_ibl_coat_treeline = fn(env: vec3, r: vec3) -> vec3 {
                        return no_such_function(env)
                    }
                });
            });
            renderer.prepare_stock_ibl(&mut cx);
            assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_none(), "{reload}: premise: the program does not build");
            assert!(!renderer.stock_ibl_active() && renderer.items_ready(&cx), "{reload}: no program: nothing routes and nothing is waited for");
            cx.with_vm(|vm| {
                script_eval!(vm, { mod.draw.mat_ibl_coat_treeline = mod.draw.mat_ibl_coat_treeline_kept });
            });
            renderer.prepare_stock_ibl(&mut cx);
            assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_none(), "{reload}: a failed build is remembered: the next frame builds nothing");

            run(&mut renderer);
            assert!(renderer.ibl_texture().is_some(), "{reload}: premise: the environment stays bound");
            renderer.prepare_stock_ibl(&mut cx);
            assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_some(), "{reload}: the next frame under the environment builds the program");
        }
    }

    // ---- the lanes' control value (`ibl_ctl`) ----

    use makepad_draw::makepad_platform::thread::ShutdownMode;
    use makepad_render_material::ibl::EnvMap;
    use makepad_scene::{GeometryId, GeometryRef, Ibl, IblSource, Item, ItemKind, Light, MaterialFrame, MaterialId, MaterialKind, PbrParams, TextureRef};

    /// A sky over a darker ground, `level` the sky's green: 2.9 meters like
    /// the clear noon preset (a mean near 1.8, exposure about 0.4), 0.006
    /// like a night (the band's ceiling, 3.2).
    fn sky_over_ground(level: f32) -> EnvMap {
        EnvMap::from_fn(64, |d| if d[1] > 0.0 { [0.8 * level, level, 1.25 * level] } else { [0.3 * level, 0.27 * level, 0.2 * level] })
    }

    /// The built-in material a lit, opaque item draws through under an
    /// environment (renderer/items.rs `item_custom`).
    const ITEM_IBL: &str = "__item_ibl";

    /// Bind `map` on `renderer` (a closed pool prepares on the spot), let a
    /// scene draw's first decision install the stock IBL program, and give
    /// the world that names the map, with one IBL item in it (a chrome
    /// triangle, drawn through `__item_ibl`).
    fn bind_map(cx: &mut Cx, renderer: &mut Renderer, map: EnvMap) -> World {
        cx.task_pool().close(ShutdownMode::CancelPending);
        renderer.register_environment(TextureRef(1), std::sync::Arc::new(map));
        let mut world = World::new();
        world.environment.ibl = Some(Ibl { source: IblSource::Hdri(TextureRef(1)), intensity: 1.0, rotation_deg: 0.0 });
        renderer.resolve_ibl(cx, &world.environment);
        assert!(renderer.ibl_texture().is_some(), "premise: the environment is bound");
        renderer.prepare_stock_ibl(cx);
        assert!(renderer.custom_material(STOCK_IBL_MATERIAL).is_some(), "premise: the stock IBL program is installed");
        let triangle = GeometryData {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![],
            colors: vec![],
            indices: vec![0, 1, 2],
        };
        renderer.register_geometry(GeometryId(1), triangle).expect("the item's geometry");
        world.set_material(MaterialFrame { id: MaterialId(1), kind: MaterialKind::Pbr(PbrParams { metallic: 1.0, roughness: 0.15, ..Default::default() }), ..Default::default() });
        world.items.push(Item::new(ItemKind::Mesh { geometry: GeometryRef::Resident(GeometryId(1)), material: MaterialId(1), transform: Mat4f::identity() }));
        // The frame's item pass picks (and builds) the item's material.
        renderer.push_item_instances(cx, &world);
        let named = renderer.placed_models.last().and_then(|inst| inst.custom_material.as_ref()).map(|m| m.name.clone());
        renderer.pop_item_instances();
        assert_eq!(named.as_deref(), Some(ITEM_IBL), "premise: under an environment a lit item draws through the item IBL material");
        assert!(renderer.custom_material(ITEM_IBL).is_some_and(|m| m.ibl), "premise: it is an IBL program");
        world
    }

    /// The frame's rig for `world` and, as `draw_scene_inner` does at the
    /// same site, the lanes' control for this draw.
    fn frame_rig(renderer: &mut Renderer, world: &World) -> SunLight {
        let env = renderer.env_lighting(world);
        renderer.resolve_ibl_lane(world, env.as_ref());
        crate::world_lights::apply_world_sun(world, renderer.lane_rig(world, crate::sun::resolve_sun(&world.sun)))
    }

    /// `ibl_ctl` as it stands on a draw.
    fn ctl_on(cx: &Cx, vars: &DrawVars) -> [f32; 4] {
        let (at, slots) = vars.uniform_range(cx, live_id!(ibl_ctl)).expect("the ibl_ctl uniform");
        assert_eq!(slots, 4);
        [vars.dyn_uniforms[at], vars.dyn_uniforms[at + 1], vars.dyn_uniforms[at + 2], vars.dyn_uniforms[at + 3]]
    }

    const STOCK_LANE: &str = "the stock IBL program";
    const ITEM_LANE: &str = "an IBL item";
    const SKIN_LANE: &str = "the skin lane";

    /// What the three lanes that take the IBL lookups write on their draws
    /// this frame, each bound as its pass binds it: the GI first (`gi_on`:
    /// what `gi.bind` writes, the field's strength while it samples), then
    /// the lane's control. A program's draw is out of `custom_draws` for
    /// its pass, as in `draw_custom_models`.
    fn lane_ctls(cx: &Cx, renderer: &mut Renderer, skin: &mut DrawSceneSkinnedGpu, gi_on: f32) -> [(&'static str, [f32; 4]); 3] {
        let program = |renderer: &mut Renderer, name: &str| {
            let mut m = renderer.custom_draws.remove(name).expect("installed");
            m.draw.draw_vars.set_uniform(cx, live_id!(gi_on), &[gi_on]);
            renderer.bind_ibl_lane(cx, name, &mut m.draw.draw_vars);
            let ctl = ctl_on(cx, &m.draw.draw_vars);
            renderer.custom_draws.insert(name.to_string(), m);
            ctl
        };
        let stock = program(renderer, STOCK_IBL_MATERIAL);
        let item = program(renderer, ITEM_IBL);
        skin.draw_vars.set_uniform(cx, live_id!(gi_on), &[gi_on]);
        renderer.bind_skin_ibl(cx, &mut skin.draw_vars);
        [(STOCK_LANE, stock), (ITEM_LANE, item), (SKIN_LANE, ctl_on(cx, &skin.draw_vars))]
    }

    /// What `mat_ibl_ambient` returns for the normal `n` (render-material's
    /// builtin.rs): the SH9 irradiance, clamped, x `Ibl.intensity` / pi.
    /// The rotation is about +Y and leaves the two poles where they are.
    fn mat_ibl_ambient(lighting: &crate::sun::EnvLighting, n: [f32; 3]) -> Vec3f {
        let e = makepad_render_material::ibl::sh9_irradiance(&lighting.sh, n);
        vec3f(e[0].max(0.0), e[1].max(0.0), e[2].max(0.0)) * (lighting.gain / std::f32::consts::PI)
    }

    /// Decision 1: one exposure for everything in a lane. For a map that
    /// meters far from 1, the frame's rig fill at +Y and -Y is what each
    /// lane's IBL fill returns for the same normal (`mat_ibl_ambient` x the
    /// scale on the lane's draw), under HDR output and in the legacy lane.
    /// Before the lanes carried the scale the legacy lane's lookups lit at
    /// the map's raw scale beside a rig exposed by 0.4 (noon) or 3.2 (night).
    #[test]
    fn in_each_lane_the_rigs_fill_is_the_ibl_fill_of_the_lanes_that_take_the_lookups() {
        for (label, level) in [("a clear noon", 2.9), ("a night", 0.006)] {
            let mut cx = headless();
            let mut renderer = Renderer::default();
            let world = bind_map(&mut cx, &mut renderer, sky_over_ground(level));
            let mut skin = cx.with_vm(|vm| DrawSceneSkinnedGpu::script_new_with_default(vm));
            let lighting = renderer.env_lighting(&world).expect("the bound map lights the world");
            let exposure = crate::sun::env_exposure(&lighting);
            assert!((exposure - 1.0).abs() > 0.5, "premise: {label} meters far from 1 ({exposure})");
            for hdr in [false, true] {
                renderer.hdr_output = hdr;
                let rig = frame_rig(&mut renderer, &world);
                assert!(rig.sky.y > rig.ground.y && rig.ground.y > 0.0, "premise: {label}, hdr {hdr}: the map fills both hemispheres, {:?} over {:?}", rig.sky, rig.ground);
                for (lane, ctl) in lane_ctls(&cx, &mut renderer, &mut skin, 0.0) {
                    for (n, fill) in [([0.0, 1.0, 0.0], rig.sky), ([0.0, -1.0, 0.0], rig.ground)] {
                        let lookup = mat_ibl_ambient(&lighting, n) * ctl[1];
                        let off = fill - lookup;
                        // The rig's values sit on its colour grid (1/4096).
                        assert!(off.x.abs().max(off.y.abs()).max(off.z.abs()) <= 0.6 / 4096.0, "{label}, hdr {hdr}, {lane}, n {n:?}: the rig fills {fill:?}, the lane's IBL fill is {lookup:?}");
                    }
                    assert_eq!(ctl, [1.0, if hdr { 1.0 } else { exposure }, 1.0, 0.0], "{label}, hdr {hdr}, {lane}: the lookups on, the lane's scale, the fill the map's");
                }
            }
        }
    }

    /// The built-in item material carries the lane's scale too (it has lit
    /// at the map's raw scale in the legacy lane since the rig took the
    /// map's exposure), and always fills from the map: an item has no fill
    /// of its own to keep.
    #[test]
    fn the_item_ibl_material_carries_the_lanes_scale() {
        let mut cx = headless();
        let mut renderer = Renderer::default();
        let world = bind_map(&mut cx, &mut renderer, sky_over_ground(2.9));
        let mut skin = cx.with_vm(|vm| DrawSceneSkinnedGpu::script_new_with_default(vm));
        let lighting = renderer.env_lighting(&world).expect("the bound map lights the world");
        let exposure = crate::sun::env_exposure(&lighting);
        let item = |cx: &Cx, renderer: &mut Renderer, skin: &mut DrawSceneSkinnedGpu, gi_on: f32| lane_ctls(cx, renderer, skin, gi_on)[1].1;
        frame_rig(&mut renderer, &world);
        assert_eq!(item(&cx, &mut renderer, &mut skin, 0.0), [1.0, exposure, 1.0, 0.0], "the legacy lane: the map's exposure on both lookups");
        renderer.hdr_output = true;
        frame_rig(&mut renderer, &world);
        assert_eq!(item(&cx, &mut renderer, &mut skin, 0.0), [1.0, 1.0, 1.0, 0.0], "HDR output: the composite exposes");
        // An authored ambient and gathered GI are the stock lanes' to keep.
        renderer.hdr_output = false;
        let mut authored = world.clone();
        authored.sun.ambient = Some(vec3f(0.55, 0.55, 0.55));
        frame_rig(&mut renderer, &authored);
        assert_eq!(item(&cx, &mut renderer, &mut skin, 0.6), [1.0, exposure, 1.0, 0.0], "an item fills from the map whatever the world authors");
    }

    /// Decisions 2 and 3. While the world authors its fill (a script's
    /// `SunConfig.ambient`, a `Light::Sky`) or fast GI gathers one for the
    /// draw, the stock IBL program and the skin lane keep that fill
    /// (`ibl_ctl.z` 0) and still take the reflection from the map, at the
    /// lane's scale (x and y stay). An IBL item is not a stock lane.
    #[test]
    fn an_authored_fill_or_gathered_gi_keeps_the_stock_lanes_fill_and_the_maps_reflection() {
        let mut cx = headless();
        let mut renderer = Renderer::default();
        let world = bind_map(&mut cx, &mut renderer, sky_over_ground(2.9));
        let mut skin = cx.with_vm(|vm| DrawSceneSkinnedGpu::script_new_with_default(vm));
        let lighting = renderer.env_lighting(&world).expect("the bound map lights the world");
        // The legacy lane (the default): its scale is the map's exposure.
        let scale = crate::sun::env_exposure(&lighting);
        let want = |lanes: [(&'static str, [f32; 4]); 3], stock_fill: f32, what: &str| {
            for (lane, ctl) in lanes {
                let fill = if lane == ITEM_LANE { 1.0 } else { stock_fill };
                assert_eq!(ctl, [1.0, scale, fill, 0.0], "{what}: {lane}");
            }
        };
        let from_map = frame_rig(&mut renderer, &world);
        want(lane_ctls(&cx, &mut renderer, &mut skin, 0.0), 1.0, "nothing authored, no GI: the fill is the map's");

        let mut ambient = world.clone();
        ambient.sun.ambient = Some(vec3f(0.55, 0.55, 0.55));
        let rig = frame_rig(&mut renderer, &ambient);
        assert_eq!((rig.sky, rig.ground), (vec3f(0.55, 0.55, 0.55), vec3f(0.55, 0.55, 0.55)), "premise: the script's ambient is the rig's fill");
        want(lane_ctls(&cx, &mut renderer, &mut skin, 0.0), 0.0, "SunConfig.ambient: the script's fill stays, the reflection is the map's");

        let mut sky = world.clone();
        sky.lights.push(Light::Sky { top: vec3f(0.2, 0.3, 0.4), ground: vec3f(0.1, 0.1, 0.1), intensity: 1.0 });
        let rig = frame_rig(&mut renderer, &sky);
        assert_eq!((rig.sky, rig.ground), (vec3f(0.2, 0.3, 0.4), vec3f(0.1, 0.1, 0.1)), "premise: the world's Sky is the rig's fill");
        want(lane_ctls(&cx, &mut renderer, &mut skin, 0.0), 0.0, "a Light::Sky: the world's fill stays, the reflection is the map's");

        // The next draw's world authors nothing: the map's fill again.
        assert_eq!(frame_rig(&mut renderer, &world).sky, from_map.sky);
        want(lane_ctls(&cx, &mut renderer, &mut skin, 0.0), 1.0, "the authored fill gone");

        // Fast GI gathers the ambient for a draw (gi_on above 0, what
        // gi.bind writes while the field samples): the draw keeps it.
        want(lane_ctls(&cx, &mut renderer, &mut skin, 0.6), 0.0, "fast GI on: the gathered fill stays, the reflection is the map's");
        want(lane_ctls(&cx, &mut renderer, &mut skin, 0.0), 1.0, "fast GI off again");
    }

    /// No environment: no draw's control differs from the uniform's
    /// default, which is the lookups as they were before the control (the
    /// map's own scale, the fill from the map), and the skin lane's switch
    /// stays off with every component zero.
    #[test]
    fn without_an_environment_the_lanes_control_is_neutral() {
        let mut cx = headless();
        let mut renderer = Renderer::default();
        let mut skin = cx.with_vm(|vm| DrawSceneSkinnedGpu::script_new_with_default(vm));
        // An IBL program can be installed with no environment bound (a
        // host's; here the engine's own, built directly).
        assert!(renderer.stock_ibl_install(&mut cx));
        let default = ctl_on(&cx, &renderer.custom_material(STOCK_IBL_MATERIAL).unwrap().draw.draw_vars);
        assert_eq!(default, [1.0, 1.0, 1.0, 0.0], "an IBL program whose control was never written takes the raw lookups");
        let mut authored = World::new();
        authored.sun.ambient = Some(vec3f(0.55, 0.55, 0.55));
        for world in [World::new(), authored] {
            for hdr in [false, true] {
                renderer.hdr_output = hdr;
                frame_rig(&mut renderer, &world);
                let mut m = renderer.custom_draws.remove(STOCK_IBL_MATERIAL).unwrap();
                renderer.bind_ibl_lane(&cx, "a host's IBL material", &mut m.draw.draw_vars);
                assert_eq!(ctl_on(&cx, &m.draw.draw_vars), default, "hdr {hdr}: no environment, nothing to scale");
                renderer.custom_draws.insert(STOCK_IBL_MATERIAL.to_string(), m);
                renderer.bind_skin_ibl(&cx, &mut skin.draw_vars);
                assert_eq!(ctl_on(&cx, &skin.draw_vars), [0.0; 4], "hdr {hdr}: no environment, the skin lane's switch off");
            }
        }
    }

    /// Decision 8: a model whose named material is not installed is a stock
    /// model. A shiny one takes the stock IBL route like every other shiny
    /// stock prop; one whose material is installed keeps its own program,
    /// in every lane's walk of the list (the walking program's draw is out
    /// of `custom_draws` for its pass).
    #[test]
    fn a_shiny_model_naming_an_uninstalled_material_takes_the_stock_ibl_route() {
        let made = Made::none();
        let mut cx = headless();
        let mut renderer = renderer_with_a_chrome_and_a_matte_model(&mut cx);
        bind_map(&mut cx, &mut renderer, EnvMap::constant(16, [0.25; 3]));
        let paint = cx.with_vm(|vm| DrawSceneCustom::unlit(vm, &HookSet::new(), HookMask::ALL, 0.0)).unwrap_or_else(|e| panic!("{e}"));
        assert!(renderer.install_custom_material("paint".to_string(), paint), "a host material, installed");
        let instance = |model: &str, material: Option<&str>| ModelInstance {
            model: model.into(),
            custom_material: material.map(|name| CustomMaterialInstance { name: name.into(), params: Vec4f::default() }),
            transform: Mat4f::identity(), tint: vec4(1.0, 1.0, 1.0, 1.0), color_adjust: vec4(0.0, 1.0, 1.0, 0.0), dynamic: true, depth_order: 0.0, part_poses: Vec::new(), lamps: Default::default(),
        };
        let (stock, missing, own) = (instance("test/chrome", None), instance("test/chrome", Some("replica/paint")), instance("test/chrome", Some("paint")));
        let program = renderer.custom_material_shader(STOCK_IBL_MATERIAL).expect("premise: the program is built");
        assert!(!renderer.stock_ibl_active(), "premise: a test runs no beat, so the HAL has made no pipeline: the frame waits for the program");
        made.pipeline(program);
        renderer.prepare_stock_ibl(&mut cx);
        assert!(renderer.stock_ibl_active(), "premise: the frame routes once the program can draw");
        let chrome = wants_pbr(&renderer, "test/chrome");
        // Each lane's walk: the stock lanes (no program), the stock IBL
        // program's own pass and the host material's own pass, in a frame
        // that routes (this one) and in one that waits for the program's
        // pipeline.
        for (active, lane) in [true, false].into_iter().flat_map(|active| [None, Some(STOCK_IBL_MATERIAL), Some("paint")].map(|lane| (active, lane))) {
            let walking = lane.and_then(|name| renderer.custom_draws.remove(name).map(|m| (name, m)));
            assert!(!renderer.has_own_material(&stock, lane), "lane {lane:?}: no name, no material of its own");
            assert!(!renderer.has_own_material(&missing, lane), "lane {lane:?}: a name that is not installed is no material of its own");
            assert!(renderer.has_own_material(&own, lane), "lane {lane:?}: an installed material is the model's own");
            assert_eq!(takes_stock_ibl(active, chrome, renderer.has_own_material(&missing, lane), false), active, "lane {lane:?}, routing {active}: the shiny model takes the stock IBL route, as the shiny stock prop beside it");
            assert_eq!(takes_stock_ibl(active, chrome, renderer.has_own_material(&stock, lane), false), active, "lane {lane:?}, routing {active}");
            assert!(!takes_stock_ibl(active, chrome, renderer.has_own_material(&own, lane), false), "lane {lane:?}, routing {active}: the model with its own program keeps it");
            if let Some((name, m)) = walking { renderer.custom_draws.insert(name.to_string(), m); }
        }
        // The program's pass draws when such a model is listed, alone too
        // (the PBR lane has left it: nothing else would draw it).
        assert!(renderer.lists_a_stock_ibl_model(&[instance("test/matte", None), instance("test/chrome", Some("replica/paint"))]), "an uninstalled name on a shiny model is listed for the program");
        assert!(renderer.lists_a_stock_ibl_model(&[instance("test/chrome", None)]));
        assert!(!renderer.lists_a_stock_ibl_model(&[instance("test/chrome", Some("paint")), instance("test/matte", None), instance("test/matte", Some("replica/paint"))]), "its own program, or a matte model: nothing for the stock IBL program");
        // A material dropped by a reload is not installed any more.
        renderer.retain_custom_materials(&[STOCK_IBL_MATERIAL.to_string()]);
        assert!(!renderer.has_own_material(&own, None), "dropped by retain_custom_materials: a stock model again");
        assert!(renderer.lists_a_stock_ibl_model(&[own]));
    }
}
