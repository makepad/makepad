//! The fast-GI hook of the scene pass: scene snapshot keying, the probe
//! volume's centre, and the per-frame GI run.

use super::*;
use crate::GiReceivers;

/// Screen AO for one view. The host supplies its frame/sample index and
/// resets history on camera cuts; radius is in world units.
#[derive(Clone, Copy, Debug)]
pub struct ScreenAoParams {
    pub strength: f32,
    pub radius: f32,
    pub tier: crate::cortao::CortaoTier,
    /// The AO chain's internal size (`Auto`: half size for 4K-class frames).
    pub resolution: crate::cortao::CortaoResolution,
    pub history: bool,
    pub frame: u64,
    pub reset: bool,
}

impl Renderer {
    /// Apply a world's camera-following probe lighting without invalidating
    /// an unchanged field. Soft local shadows stay enabled after GI is disabled.
    pub fn set_gi_settings(&mut self, settings: Option<makepad_scene::GiSettings>) {
        if let Some(settings) = settings {
            let config = crate::GiConfig { gpu_budget_ms: self.gi_budget_ms, ..crate::GiConfig::from(settings) };
            if self.gi_config() != config {
                self.set_gi_config(config);
            }
            self.set_gi_mode(crate::GiMode::Fast);
            self.set_soft_local_shadows(true);
        } else {
            self.set_gi_mode(crate::GiMode::Off);
        }
    }

    /// The GPU device was replaced (`Event::GpuReset`): the GI field and cascades, and the
    /// baked lightmaps, lived only on the GPU. GI starts over; the lightmap bakes again into
    /// its regions.
    pub fn gpu_reset(&mut self) {
        self.gi.release();
        self.volume_fog.reset();
        self.ssr.reset();
        self.gpu_baker.debug_redirty();
    }

    /// Hosts that light in linear space (HDR output) tell GI their albedo
    /// inputs are sRGB-encoded, so bounce light is computed on decoded
    /// albedo. Changing the decode wakes the probe field.
    pub fn set_gi_linear_albedo(&mut self, on: bool) { self.gi.set_linear_albedo(on); }

    /// Screen-space ambient occlusion for a host WITHOUT a depth prepass:
    /// call once per frame after `make_child_pass(scene_pass)` and before
    /// `draw_scene_full`. `depth` is the scene pass's depth target
    /// (`TextureFormat::DepthD32Sampled`); it still holds the previous
    /// frame, so the occlusion trails the image by one frame. It is bound
    /// through `set_ssao`: the model lanes apply it to their ambient term
    /// only (never sun or lamps), as dark as `params.strength` (0 turns
    /// it off) over `params.radius` metres. CORTAO returns full-resolution
    /// visibility, with optional temporal history owned by this view.
    pub fn run_screen_ao(&mut self, cx: &mut Cx2d, scene_pass: &DrawPass, depth: &Texture, size: DVec2, mut params: ScreenAoParams) {
        let strength = params.strength;
        // Re-enabling AO must not blend a history from before it was off.
        params.reset |= self.ssao.is_none();
        let out = if strength > 0.0 { self.gi.run_screen_ao(cx, scene_pass.draw_pass_id(), depth, size, params) } else { None };
        self.set_ssao(out.map(|t| (t, strength.min(1.0))));
    }

    /// Choose how the scene's receivers obtain their GI ambient.
    pub fn set_gi_receivers(&mut self, receivers: GiReceivers) { self.gi.set_receivers(receivers); }

    /// The scene's current GI receiver setting.
    pub fn gi_receivers(&self) -> GiReceivers { self.gi.receivers() }

    /// Gather GI into a quarter-resolution cache. Call once per frame beside
    /// `run_screen_ao`, after `make_child_pass(scene_pass)` and before
    /// `draw_scene_full`. The sampled scene depth (`DepthD32Sampled`), pass
    /// camera and render origin are from the previous scene draw. Returns
    /// true when the fill is recorded. A new target waits one frame for depth.
    /// In [`GiReceivers::Exact`] (the default), it records nothing and every receiver gathers.
    pub fn run_gi_screen(&mut self, cx: &mut Cx2d, scene_pass: &DrawPass, depth: &Texture, size: DVec2) -> bool {
        self.gi.run_gi_screen(cx, scene_pass.draw_pass_id(), depth, size, self.gi.last_scene_origin)
    }

    /// Call instead of [`Self::run_gi_screen`] when the frame cannot use a
    /// single camera's cache, as in a split view, XR or an auxiliary scope.
    pub fn skip_gi_screen(&mut self, cx: &mut Cx) { self.gi.skip_gi_screen(cx); }

    /// The last `run_screen_ao` bound nothing only because the AO's pipelines
    /// are still being made (native compiles them in the background, and
    /// `Cx::draw_shaders_pending` sees only WebGL's): a host settling a still
    /// frame records again rather than taking one without its AO.
    pub fn screen_ao_pending(&self) -> bool { self.gi.screen_ao_pending() }

    /// A host's own GI scene, replacing what the renderer would take from the
    /// world and its models: its solids (meshes or distance fields, in the
    /// pass's world frame) under the key that identifies them. A changed key
    /// is the next snapshot; the voxel worker re-voxelizes only the bricks of
    /// the instances that differ. `None`: the renderer's own scene.
    pub fn set_gi_scene(&mut self, scene: Option<(u64, Vec<crate::fast_gi::GiInstance>)>) {
        self.gi_host_scene = scene;
    }

    /// The GI's work is not done: draw again (a settling host stops drawing
    /// when this goes false).
    pub fn gi_wants_frames(&self) -> bool { self.gi.wants_frames() }

    /// Binds what a host's own shader needs to sample the GI
    /// (`mod.draw.FastGiSampling`); `gi_shading` says whether it reads anything.
    pub fn bind_gi(&self, cx: &Cx, draw: &mut DrawVars) {
        self.gi.bind(cx, draw);
        self.gi.bind_host_screen(cx, draw);
    }

    /// The host fills this cache with `gi_screen_value` over its own G-buffer at
    /// `view`, in its receivers' coordinate frame. Its receivers spread
    /// `mod.draw.FastGiScreen`, bind through `bind_gi` and call `gi_ambient_at`.
    /// Pass `None` to gather directly on every pixel.
    pub fn set_gi_screen(&mut self, screen: Option<crate::GiScreenCache>) { self.gi.set_screen(screen); }

    /// Whether the GI has something to sample this frame.
    pub fn gi_shading(&self) -> bool { self.gi.shader_features().0 }

    /// Queue GPU-confirmed valid/invalid/untraced probe counts. Call after
    /// drawing the scene, while its parent pass is active. This adds one tiny
    /// reduction pass and an asynchronous readback; it never waits for the GPU.
    /// Returns false when a request is already in flight; a deferred reduction
    /// is reattached on this draw. Poll before retrying on a later draw/report.
    /// Requests waiting for a ticket are retained and retried on later draws.
    pub fn request_gi_probe_counts(&mut self, cx: &mut CxDraw) -> Result<bool, makepad_platform::ReadbackError> {
        self.gi.request_probe_counts(cx)
    }

    /// Consume only GI's readback ticket, on Event::Signal or the report
    /// cadence. None means no completed sample. On success `gi_stats()` also
    /// holds the counts and their request frame; the empty-field guard reports
    /// once if that sample followed a full sweep in a scene with geometry.
    /// A reset discards old-generation results. No redraw or GPU wait is needed.
    pub fn poll_gi_probe_counts(&mut self, cx: &mut Cx) -> Result<Option<crate::GiStats>, makepad_platform::ReadbackError> {
        self.gi.poll_probe_counts(cx)
    }

    /// World and placed-model revisions used to request a GI snapshot.
    pub(crate) fn gi_scene_key(&self, world: &World) -> crate::fast_gi::SceneKey {
        let terrain_rev = world.terrain.as_deref().map_or(0, |t| t.revision);
        let voxel_rev = world.voxel.as_ref().map_or(0, |v| v.meshes.iter().fold(0xcbf29ce484222325u64, |h, (key, mesh)| {
            [key.x as u64, key.y as u64, key.z as u64, mesh.rev].into_iter().fold(h, |h, value| h.wrapping_mul(1099511628211) ^ value)
        }));
        [world.render_rev, world.paint_rev, terrain_rev, voxel_rev, self.models_rev, self.models_gi_rev]
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_fast_gi(
        &mut self,
        cx: &mut Cx3d,
        world: &World,
        camera_pos: Vec3f,
        scene_state: &SceneState3D,
        _skins: Option<&[SkinnedDraw]>,
        sun: &SunLight,
        stats: &mut RenderStats,
    ) {
        // Orbit cameras shade their subject, not empty space around a
        // distant lens. Player views use the followed body; XR falls
        // back to its world-space eye when no game camera is active.
        let center=if let Some(focus)=self.csm_focus {
            camera_pos-vec3f(scene_state.view.v[2],scene_state.view.v[6],scene_state.view.v[10])*focus
        }else if let Some(e)=world.entity(world.camera.third).or_else(||world.entity(world.camera.follow)) {e.pos+vec3f(0.0,1.0,0.0)}
        else if self.stage.mode==StageMode::Flat {world.camera.target}else{camera_pos};
        // A paint revision takes a new snapshot. Its diff re-voxelizes the
        // changed bricks and nearby probes get the first re-traces.
        let key=self.gi_scene_key(world);
        let center=self.gi.config().anchor.unwrap_or(center);
        // A host's scene has its own key: nothing of the world's enters it.
        let key=self.gi_host_scene.as_ref().map_or(key,|(host,_)|[*host,0,0,0,0,0]);
        if self.gi.wants_scene(key,center) {
            let start=Cx::monotonic_now();
            let snapshot=match &self.gi_host_scene {
                Some((_,instances))=>{let triangles=instances.iter().map(|i|i.triangles()).sum();crate::fast_gi::Snapshot{instances:instances.clone(),triangles,region:None}},
                None=>self.gi_snapshot(cx.cx,world,self.gi.region(center)),
            };
            self.gi.submit_scene(key,center,snapshot);
            self.gi.stats.snapshot_us=((Cx::monotonic_now()-start)*1e6)as u64;
        }
        self.gi.set_scene_stale(!self.gi.scene_is(key));
        self.gi.set_emission_gain(self.lin_ctl()[3]);
        self.gi.run(cx.cx,center,sun,&self.clustered);
        if self.gi.stats.batch>0 {if let Some(parent)=self.gi.relight_pass(){self.clustered.parent_shadows(cx.cx,parent);}}
        stats.gi=self.gi.stats;
        if crate::fast_gi::gi_stats_enabled() && self.clustered_frames%120==0 {
            if let Err(error)=self.poll_gi_probe_counts(cx.cx.cx) {
                error!("fast GI: probe count readback failed: {error}");
            }
            if self.gi.mode()==crate::GiMode::Fast {
                if let Err(error)=self.request_gi_probe_counts(cx.cx) {
                    error!("fast GI: probe count request failed: {error}");
                }
            }
            stats.gi=self.gi.stats;
            let g=&stats.gi;
            log!("fast GI: {} instances / {} triangles ({} outside region), {} probes ({} pending), {} valid / {} invalid / {} untraced (count request frame {:?}), batch {} of budget {}, {} lights, {}us snapshot, {}us encode ({} static bytes), voxels {} jobs / {} bricks / {:.1}ms, GPU [trace, relight, gather, scatter] mean {:?}ms max120 {:?}ms, sum mean {:.3}ms max120 {:.3}ms max600 {:.3}ms aim {:.3}ms, AO {:.3}ms, {} bytes resident, {} uploaded; building={} unsupported={}",
                g.instances,g.triangles,g.omitted_instances,g.probes,g.pending_probes,g.valid_probes,g.invalid_probes,g.untraced_probes,g.probe_counts_frame,g.batch,g.budget,g.selected_lights,g.snapshot_us,g.encode_us,g.static_encode_bytes,g.voxel_jobs,g.voxel_bricks,g.voxel_ms,g.gpu_ms.map(|v|(v*1000.0).round()/1000.0),g.gpu_peak_ms.map(|v|(v*1000.0).round()/1000.0),g.frame_sum_mean,g.frame_sum_max120,g.frame_sum_max,g.gpu_aim_ms,g.ao_gpu_ms,g.resident_bytes,g.uploads_bytes,g.building,g.rejected_scene);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_scene::GiSettings;

    #[test]
    fn settings_enable_and_disable_without_resetting_an_unchanged_field() {
        let mut renderer = Renderer::default();
        assert_eq!(renderer.gi_mode(), crate::GiMode::Off);
        let settings = GiSettings { spacing: 0.75, ..Default::default() };
        renderer.set_gi_settings(Some(settings));
        let config = renderer.gi_config();
        assert_eq!(config, crate::GiConfig::from(settings));
        assert_eq!(renderer.gi_mode(), crate::GiMode::Fast);
        assert!(renderer.clustered.shadows.soft_filter);
        // release() clears statistics along with the field. Preserve a
        // resident probe count across repeated frames and lighting edits.
        renderer.gi.stats.probes = config.probe_count() * config.cascades;
        let probes = renderer.gi_stats().probes;
        renderer.set_gi_settings(Some(settings));
        assert_eq!(renderer.gi_config(), config, "unchanged config");
        assert_eq!(renderer.gi_stats().probes, probes, "unchanged settings keep the field");
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut issue = |r: &mut Renderer| r.gi.test_transport_issue(&mut cx, &r.clustered);
        for _ in 0..39 { assert!(issue(&mut renderer).is_some(), "the initial cold field must converge"); }
        assert_eq!(issue(&mut renderer), None, "the unchanged field sleeps");
        let dim = GiSettings { strength: 0.4, ..settings };
        renderer.set_gi_settings(Some(dim));
        assert_eq!(issue(&mut renderer), None, "receiver strength alone must issue nothing");
        renderer.set_gi_settings(Some(GiSettings { bounce: 0.0, ..dim }));
        for _ in 0..19 {
            assert_eq!(issue(&mut renderer), Some(0.85), "a feedback change must restart warm transport");
        }
        assert_eq!(issue(&mut renderer), None, "the field sleeps again after its warm issues");
        renderer.set_gi_config(crate::GiConfig { gpu_budget_ms: 3.0, ..renderer.gi_config() });
        assert_eq!(issue(&mut renderer), None, "the GPU budget alone must issue nothing");
        assert_eq!(renderer.gi_stats().probes, probes, "lighting changes keep the field");
        let medium = crate::quality_tier::GraphicsTier::Medium;
        renderer.set_graphics_tier(medium);
        assert_eq!(issue(&mut renderer), None, "the tier's GPU budget alone must issue nothing");
        renderer.set_gi_settings(Some(settings));
        assert_eq!(renderer.gi_config().gpu_budget_ms, medium.budget().gi_budget_ms, "the host's GI settings must keep the tier's budget");
        assert_eq!(renderer.gi_stats().probes, probes, "the tier's budget keeps the field");
        renderer.set_gi_settings(Some(GiSettings { spacing: 1.5, ..settings }));
        assert_eq!(renderer.gi_stats().probes, 0, "spacing rebuilds the field");
        renderer.set_gi_settings(None);
        assert_eq!(renderer.gi_mode(), crate::GiMode::Off);
        assert!(renderer.clustered.shadows.soft_filter, "disabling GI keeps shadow quality");
    }
}
