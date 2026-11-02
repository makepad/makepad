//! Star map, lighting-tier, GI, cluster, CSM and material settings.

use super::*;

/// Emission gain in the HDR lane: a glowing window or beacon is a light
/// source, several times brighter than a lit wall once night exposure opens.
const HDR_GLOW_GAIN: f32 = 3.0;
/// Auto-exposure: the mean scene luminance maps to this after exposure (its
/// band around the metered exposure is the grade's, see [`Renderer::set_grade`]).
const AUTO_EXPOSURE_KEY: f32 = 0.2;

/// Find the star panorama by the standard search order:
/// the `MAKEPAD_STAR_MAP` env override, then — walking up from the
/// working directory — the repo-local cache `local/sky/` that
/// `tools/download_stars.sh` fills (NASA/GSFC SVS "Deep Star Maps
/// 2020", public domain, credit NASA/GSFC SVS; the ATTRIBUTION.txt
/// sits beside it), then the sandbox's bundled copy. Without a hit the
/// analytic point stars stay and one hint is logged. Call from a worker.
pub fn star_map_path() -> Option<std::path::PathBuf> {
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
                .push(d.join("apps/commercial/sandbox/resources/sky/starmap_2020_4k.png"));
            dir = d.parent();
        }
    }
    for path in candidates {
        if path.is_file() {
            log!("star map: {}", path.display());
            return Some(path);
        }
    }
    log!(
        "star map: none found — analytic stars only (run tools/download_stars.sh, or set MAKEPAD_STAR_MAP)"
    );
    None
}

impl Renderer {
    /// Opt into the linear HDR lighting convention. The HOST must then
    /// render the scene pass into an RGBA16F colour target
    /// (`TextureFormat::RenderRGBAf16`, see `GpuInfo::float16_blend_targets`)
    /// and composite it with `DrawSceneTexture::post = composite_post()`.
    /// On: texels and vertex colours are decoded sRGB -> linear, the sun,
    /// fill, fog and lights are scene-referred (unclamped), no lane tone
    /// maps, and the composite applies exposure, AgX, FXAA and dither. Off
    /// (the default) is the legacy display-space lane, unchanged for every
    /// host that did not ask.
    pub fn set_hdr_output(&mut self, on: bool) {
        self.hdr_output = on;
        // GI traces the same scene: its albedo decodes with the lanes'.
        self.set_gi_linear_albedo(on);
    }

    pub fn hdr_output(&self) -> bool {
        self.hdr_output
    }

    /// Display dither on 8-bit targets (on by default).
    pub fn set_output_dither(&mut self, on: bool) {
        self.output_dither = on;
    }

    /// Bind display dither for a lane or a host's own receiver, inside its colour pass.
    pub fn bind_output_dither(&self, cx: &mut CxDraw, draw: &mut DrawVars) {
        let pass_id = cx.current_pass_id();
        let target = cx.passes[pass_id].color_textures.first().map(|t| t.texture.clone());
        let size = match target.as_ref().map(|t| t.get_format(cx.cx).clone()) {
            Some(TextureFormat::RenderBGRAu8 { size, .. }) | Some(TextureFormat::RenderCubeBGRAu8 { size, .. }) => match size {
                TextureSize::Fixed { width, height } => Some((width, height)),
                TextureSize::Auto => {
                    // Raster density can differ from the density used for layout.
                    let dpi = cx.passes[pass_id].dpi_factor.unwrap_or_else(|| cx.current_dpi_factor());
                    cx.cx.get_pass_rect(pass_id, dpi).map(|rect| Cx::pass_pixel_size(rect.size, dpi))
                }
            },
            _ => None,
        };
        let mut ctl = [1.0, 1.0, 0.0];
        if let Some((width, height)) = size {
            ctl[0] = width as f32;
            ctl[1] = height as f32;
            if self.output_dither && width > 0 && height > 0 && !self.hdr_output && self.lm_debug <= 0.5
                && !self.shadow_debug && matches!(self.gi.debug(), crate::fast_gi::GiDebug::Off)
            {
                ctl[2] = 1.0;
            }
        }
        draw.set_uniform(cx.cx, live_id!(output_dither_ctl), &ctl);
    }

    /// FXAA in the HDR composite (on by default).
    pub fn set_fxaa(&mut self, on: bool) {
        self.fxaa = on;
    }

    /// Last frame's metered exposure (1 when HDR output is off).
    pub fn exposure(&self) -> f32 {
        if self.hdr_output { self.hdr_exposure } else { 1.0 }
    }

    /// The composite's post-chain control for `DrawSceneTexture::post`.
    pub fn composite_post(&self) -> Vec4f {
        if self.hdr_output {
            vec4(1.0, self.hdr_exposure, if self.fxaa { 1.0 } else { 0.0 }, if self.auto_exposure && self.grade.auto && self.post.exposure().1 { 1.0 } else { 0.0 })
        } else {
            vec4(0.0, 1.0, 0.0, 0.0)
        }
    }

    /// Bloom share mixed into the HDR composite (0 = off).
    pub fn set_bloom(&mut self, share: f32) {
        self.bloom = share.max(0.0);
    }

    /// Adapt exposure to the rendered scene (HDR output; on by default),
    /// within a band around the metered exposure.
    pub fn set_auto_exposure(&mut self, on: bool) {
        self.auto_exposure = on;
    }

    /// The game's colour grade (exposure bias, contrast, saturation, fixed
    /// or auto exposure and its band); applied in the HDR composite.
    pub fn set_grade(&mut self, grade: makepad_scene::ColorGrade) {
        self.grade = grade;
    }

    /// Record the HDR post chain (bloom + auto-exposure) for this frame.
    /// `scene` is the host's RGBA16F scene target of logical `size`,
    /// `parent` the pass that composites it. Returns the pass the host must
    /// parent its scene pass under, so the scene renders before the chain.
    pub fn run_post(&mut self, cx: &mut Cx2d, size: DVec2, scene: &Texture, parent: DrawPassId) -> Option<DrawPassId> {
        if !self.hdr_output || (self.bloom <= 0.0 && !self.auto_exposure) {
            return None;
        }
        self.post.run(cx, size, scene, parent);
        self.post.first_pass_id()
    }

    /// GPU ms of the post chain (a frame or two behind).
    pub fn post_gpu_ms(&self) -> f64 {
        self.post.gpu_ms
    }

    /// Everything the composite needs: post controls, texel, and the bloom
    /// and exposure textures. `texels` is the scene target in pixels.
    pub fn bind_composite(&self, draw: &mut makepad_render_graph::DrawSceneTexture, texels: Vec2f) {
        draw.post = self.composite_post();
        // The HDR composite writes alpha 1 everywhere (tone-mapped colour,
        // dithered): premultiplied-over then IS a plain write. Drawn without
        // blending it is an opaque draw, so a tile GPU's hidden-surface
        // removal skips whatever the window drew under it (its ground).
        draw.draw_vars.options.alpha_blend = !self.hdr_output;
        draw.texel = vec2(1.0 / texels.x.max(1.0), 1.0 / texels.y.max(1.0));
        let bloom = self.post.bloom().filter(|_| self.hdr_output && self.bloom > 0.0);
        let g = &self.grade;
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        draw.post2 = vec4(
            if bloom.is_some() { self.bloom } else { 0.0 },
            AUTO_EXPOSURE_KEY,
            2.0f32.powf(finite(g.auto_min_ev, -0.415).clamp(-8.0, 0.0)),
            1.0 / self.post.levels().max(1) as f32,
        );
        draw.grade = vec4(
            finite(g.contrast, 1.0).clamp(0.25, 4.0),
            finite(g.saturation, 1.0).clamp(0.0, 4.0),
            2.0f32.powf(finite(g.auto_max_ev, 0.678).clamp(0.0, 8.0)),
            2.0f32.powf(finite(g.exposure_ev, 0.0).clamp(-8.0, 8.0)),
        );
        draw.tilt = vec4(
            finite(g.tilt, 0.0).clamp(0.0, 1.0),
            finite(g.tilt_center, 0.6).clamp(0.0, 1.0),
            finite(g.tilt_width, 0.2).clamp(0.0, 1.0),
            0.0,
        );
        if let Some(bloom) = bloom {
            draw.draw_vars.set_texture(1, bloom);
        }
        if let (Some(exposure), true) = self.post.exposure() {
            draw.draw_vars.set_texture(2, exposure);
        }
    }

    /// What each stateful subsystem of this renderer is doing
    /// (KERNELS.md §3.4.3), for `makepad_render_graph::locked::check`
    /// before a locked-time frame: a subsystem that carries history refuses
    /// the frame with a diagnostic naming it.
    pub fn locked_time_usage(&self) -> Vec<(makepad_render_graph::locked::Subsystem, makepad_render_graph::locked::Usage)> {
        use makepad_render_graph::locked::{Subsystem, Usage};
        let exposure = if !self.hdr_output {
            Usage::Off
        } else if self.auto_exposure && self.grade.auto {
            Usage::History
        } else {
            Usage::Analytic
        };
        let bake = if !self.lightmap_enabled {
            Usage::Off
        } else if self.gpu_baker.is_idle() {
            Usage::Synchronous
        } else {
            Usage::Pending
        };
        vec![
            (Subsystem::AutoExposure, exposure),
            (Subsystem::RenderScale, if self.thermometer.level() == 0 { Usage::Analytic } else { Usage::History }),
            (Subsystem::LightmapBake, bake),
            (Subsystem::Ssao, if self.ssao.is_some() { Usage::History } else { Usage::Off }),
            (Subsystem::FastGi, if self.gi.mode() == crate::fast_gi::GiMode::Off { Usage::Off } else { Usage::History }),
            (Subsystem::VfxParticles, if self.vfx.records.is_empty() { Usage::Off } else { Usage::History }),
            (Subsystem::OccluderDither, Usage::Stateless),
            (Subsystem::Fxaa, if self.hdr_output && self.fxaa { Usage::Stateless } else { Usage::Off }),
        ]
    }

    /// Whether this renderer can draw a locked-time frame now (see
    /// [`Renderer::locked_time_usage`]).
    pub fn check_locked_time(&self) -> Result<(), makepad_render_graph::locked::Refusal> {
        makepad_render_graph::locked::check(&self.locked_time_usage())
    }

    /// The lanes' `lin_ctl` uniform: (linear on, exposure, 1/exposure,
    /// emission gain).
    pub(super) fn lin_ctl(&self) -> [f32; 4] {
        if self.hdr_output {
            [1.0, self.hdr_exposure, 1.0 / self.hdr_exposure, HDR_GLOW_GAIN]
        } else {
            [0.0, 1.0, 1.0, 1.0]
        }
    }

    /// Install a worker-prepared panorama with its complete mip chain.
    pub fn set_star_map_texture(&mut self, texture: Texture) {
        self.star_map_loaded = true;
        self.star_texture = Some(texture);
    }

    /// The bound star texture + gain: the uploaded panorama, or a 1x1 black
    /// stand-in (gain 0) so the sky shader samples unconditionally.
    pub(super) fn star_binding(&mut self, cx: &mut Cx) -> (Texture, f32) {
        if let Some(tex) = &self.star_texture {
            return (tex.clone(), if self.star_map_loaded { 1.0 } else { 0.0 });
        }
        let mut black = ImageBuffer::default();
        black.width = 1;
        black.height = 1;
        black.data = vec![0xFF00_0000];
        let tex = black.into_new_texture(cx);
        self.star_texture = Some(tex.clone());
        (tex, 0.0)
    }

    /// Live GPU-lightmap scheduling policy switch (OnChange <-> Realtime).
    /// Takes effect immediately: Realtime -> OnChange re-dirties every
    /// region so mover shadows stamped into the tiles are baked away.
    pub fn set_gpu_lightmap_mode(&mut self, mode: crate::gpu_lightmap::GpuLightmapMode) {
        if self.gpu_baker.mode() != mode {
            self.shadow_gate = ShadowRebuildGate::default();
            self.lm_kick_sun = None;
            if self.clustered_enabled { self.clear_world_atlas(); }
        }
        self.gpu_baker.set_mode(mode);
        self.rebuild_csm_static_casters();
    }

    /// Runtime A/B switch; clustering is on by default. The old eight-light
    /// path and lamp atlas remain available with MAKEPAD_CLUSTERED=off.
    pub fn set_clustered_lighting(&mut self, enabled: bool) {
        if self.clustered_enabled == enabled { return; }
        self.clustered_enabled = enabled;
        self.gi.reset();
        self.clear_world_atlas();
        self.lamp_cache_rev = None;
        self.clustered_frames = 0;
        self.rebuild_csm_static_casters();
    }

    pub fn clustered_lighting(&self) -> bool { self.clustered_enabled }
    pub fn gi_mode(&self)->crate::GiMode{self.gi.mode()}
    pub fn gi_debug(&self)->crate::GiDebug{self.gi.debug()}
    pub fn set_gi_debug(&mut self,debug:crate::GiDebug){self.gi.set_debug(debug);}
    pub fn gi_config(&self)->crate::GiConfig{self.gi.config()}
    pub fn set_gi_mode(&mut self,mode:crate::GiMode){self.gi.set_mode(mode);}
    pub fn set_gi_config(&mut self,config:crate::GiConfig){self.gi.set_config(config);}
    pub fn gi_stats(&self)->crate::GiStats{self.gi.stats}
    pub fn set_cluster_config(&mut self, config: crate::clustered::ClusterConfig) {
        self.clustered.set_config(config);
    }
    pub fn cluster_stats(&self) -> crate::clustered::ClusterStats {
        if self.clustered_enabled { self.clustered.stats() } else { Default::default() }
    }

    pub fn set_local_shadow_config(&mut self, config: crate::local_shadows::LocalShadowConfig) {
        self.clustered.shadows.set_config(config);
    }
    /// Optional wider PCF. Kept independent of GI for fair lighting A/Bs.
    pub fn set_soft_local_shadows(&mut self,enabled:bool){self.clustered.shadows.soft_filter=enabled;}
    /// Apparent emitter radius in world units. Only the optional soft filter
    /// uses it; individual light energy/range and the Quest baseline are unchanged.
    pub fn set_local_shadow_source_radius(&mut self,radius:f32){
        self.clustered.shadows.source_radius=if radius.is_finite(){radius.clamp(0.0,2.0)}else{0.4};
    }

    pub(super) fn world_atlas_required(&self) -> bool {
        !self.clustered_enabled || self.gpu_baker.mode() == crate::gpu_lightmap::GpuLightmapMode::OnChange
    }

    pub(super) fn clear_world_atlas(&mut self) {
        self.gpu_baker.enter_realm();
        // The old CPU sun/probe colours must not survive a mode switch:
        // clustered realtime uses CSM for visibility, not a second sun bake.
        self.bake.enter_realm();
        self.lightmap = None;
        self.lm_remaps.clear();
        self.lm_ground = None;
        self.lm_top = None;
        self.lm_kick_key = None;
        self.lm_kick_sun = None;
        self.shadow_gate = ShadowRebuildGate::default();
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
    /// Camera-relative rendering (see [`crate::scene::render_origin`]):
    /// the scene draw list shifts world geometry by the render origin. The
    /// host must set its pass camera with [`Self::set_pass_camera`] (which
    /// applies the same origin). Opt-in per host; off, nothing changes.
    pub fn set_camera_relative(&mut self, on: bool) { self.camera_relative = on; }
    pub fn camera_relative(&self) -> bool { self.camera_relative }
    /// The render origin this renderer uses for a camera (zero when
    /// camera-relative rendering is off or the stage is not flat).
    pub fn render_origin_for(&self, camera: Vec3f) -> Vec3f {
        if self.camera_relative && self.stage.mode == StageMode::Flat { crate::scene::render_origin(camera) } else { Vec3f::default() }
    }
    /// The pass camera for this renderer's scene draw: absolute, or rebuilt
    /// around the render origin when camera-relative.
    pub fn set_pass_camera(&self, cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
        crate::scene::set_pass_camera_origin(cx, pass, scene, self.render_origin_for(scene.camera_pos));
    }

    /// The noise seed of the recordings that follow: 0 (the default) keeps
    /// every stochastic filter's stable pattern; a host averaging several
    /// recordings of a still view ([`crate::StillAa`]) passes each one's
    /// sample index, so the patterns differ and average out (the sun
    /// cascades' penumbra disk).
    pub fn set_noise_seed(&mut self, seed: u32) {
        self.gpu_baker.set_csm_seed(seed as f32);
    }

    /// Stretch the far sun cascade to `metres` (0 = default); see
    /// [`crate::shadow_csm::fit_cascades_reach`]. A streamed city sets this
    /// so distant towers keep their shadows.
    pub fn set_csm_far_reach(&mut self, metres: f32) {
        self.gpu_baker.set_csm_far_reach(metres);
    }

    /// The region the near sun cascade holds when that is tighter than its own slice (or, in XR,
    /// its ring around the eye): a work surface whose millimetre parts need fine shadows. World
    /// bounds; `None`: the ladder alone. See [`crate::shadow_csm::fit_near`].
    pub fn set_csm_near_bounds(&mut self, bounds: Option<(Vec3f, Vec3f)>) {
        self.gpu_baker.set_csm_near_bounds(bounds.filter(|(lo, hi)| lo.x.is_finite() && lo.y.is_finite() && lo.z.is_finite() && hi.x.is_finite() && hi.y.is_finite() && hi.z.is_finite() && lo.x <= hi.x && lo.y <= hi.y && lo.z <= hi.z));
    }

    /// Spend what `tier` allows (quality_tier.rs): impostor distances,
    /// grass reach and density, the sun cascades. Screen-space occlusion and
    /// the render scale are the host's passes: it reads them from the same
    /// [`crate::quality_tier::TierBudget`].
    pub fn set_graphics_tier(&mut self, tier: crate::quality_tier::GraphicsTier) {
        let b = tier.budget();
        self.graphics_tier = tier;
        self.impostor_scale = b.impostor_scale;
        self.grass_radius_scale = b.grass_radius;
        self.grass_density = b.grass_density;
        self.terrain_blend_reach = b.terrain_blend_reach;
        self.set_csm_config(b.csm_resolution, b.csm_range);
    }

    pub fn graphics_tier(&self) -> crate::quality_tier::GraphicsTier {
        self.graphics_tier
    }

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

    /// Fit cascades to this projection instead of the pass's. A host averaging
    /// jittered recordings supplies the unjittered projection; `None` uses the pass's.
    pub fn set_csm_fit_projection(&mut self, projection: Option<Mat4f>) {
        self.csm_fit_projection = projection;
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

    /// The per-element lookup the static draw takes (`elem_map`, slot 6, and
    /// `elem_ctl` = (width, height, texels per element: 1 or 4, enable)), applied to the PBR lane too
    /// and the sun cascades' casters so hidden, offset and moved elements agree across lanes and
    /// shadows. `None` turns it off.
    ///
    /// `casters` is the host's key of what the cascades' casters read in it (each element's
    /// shown or hidden, and its pose), changed whenever that changes. The cascades are drawn
    /// again only when it, or anything else they draw, changes: a change of the lookup the
    /// casters do not see (a selection, an element drawn by other means moving) keeps them.
    pub fn set_element_lookup(&mut self, lookup: Option<(Texture, [f32; 4])>, casters: u64) {
        self.gpu_baker.set_element_lookup(lookup.clone(), casters);
        self.element_lookup = lookup;
    }

    /// World bounds the lookup's moved elements reach past their model's own bounds, so a
    /// cascade does not cull a model whose moved element still casts into it. `None`: at rest.
    pub fn set_element_reach(&mut self, reach: Option<(Vec3f, Vec3f)>) {
        if self.element_reach != reach {
            self.element_reach = reach;
            self.rebuild_csm_static_casters();
        }
    }

    /// Binds the sun cascades to a host's own draw that spreads `mod.draw.SunCascades` and
    /// declares `csm_map` (a surface the renderer does not draw, receiving its shadows). Valid
    /// after this frame's `draw_scene_full`.
    pub fn bind_sun_cascades(&self, cx: &mut Cx, vars: &mut DrawVars) {
        let slot = vars.draw_shader_id.and_then(|s| cx.draw_shaders[s.index].mapping.textures.iter().position(|t| t.id == live_id!(csm_map)));
        let binding = self.gpu_baker.csm_binding();
        if let (Some(slot), Some(fallback)) = (slot, binding.as_ref().map(|b| b.1.clone())) {
            Self::write_csm_uniforms(cx, vars, &binding, &fallback, slot, false);
        }
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
}
