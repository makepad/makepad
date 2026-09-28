//! The fast-GI hook of the scene pass: scene snapshot keying, the probe
//! volume's centre, and the per-frame GI run.

use super::*;

impl Renderer {
    /// Hosts that light in linear space (HDR output) tell GI their albedo
    /// inputs are sRGB-encoded, so bounce light is computed on decoded
    /// albedo. Relights the field; nothing is re-traced.
    pub fn set_gi_linear_albedo(&mut self, on: bool) { self.gi.set_linear_albedo(on); }

    /// Screen-space ambient occlusion for a host WITHOUT a depth prepass:
    /// call once per frame after `make_child_pass(scene_pass)` and before
    /// `draw_scene_full`. `depth` is the scene pass's depth target
    /// (`TextureFormat::DepthD32Sampled`); it still holds the previous
    /// frame, so the occlusion trails the image by one frame. It is bound
    /// through `set_ssao`: the model lanes apply it to their ambient term
    /// only (never sun or lamps). `strength` 0 turns it off. Cost: three
    /// half-resolution passes.
    pub fn run_screen_ao(&mut self, cx: &mut Cx2d, scene_pass: &DrawPass, depth: &Texture, size: DVec2, strength: f32) {
        let out = if strength > 0.0 { self.gi.run_screen_ao(cx, scene_pass.draw_pass_id(), depth, size) } else { None };
        self.set_ssao(out.map(|t| (t, strength.min(1.0))));
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
        let terrain_rev=world.terrain.as_deref().map_or(0,|t|t.revision);
        let voxel_rev=world.voxel.as_ref().map_or(0,|v|v.meshes.iter().fold(0xcbf29ce484222325u64, |h, (key, mesh)| {
            [key.x as u64, key.y as u64, key.z as u64, mesh.rev].into_iter().fold(h, |h, value| h.wrapping_mul(1099511628211) ^ value)
        }));
        // Orbit cameras shade their subject, not empty space around a
        // distant lens. Player views use the followed body; XR falls
        // back to its world-space eye when no game camera is active.
        let center=if let Some(focus)=self.csm_focus {
            camera_pos-vec3f(scene_state.view.v[2],scene_state.view.v[6],scene_state.view.v[10])*focus
        }else if let Some(e)=world.entity(world.camera.third).or_else(||world.entity(world.camera.follow)) {e.pos+vec3f(0.0,1.0,0.0)}
        else if self.stage.mode==StageMode::Flat {world.camera.target}else{camera_pos};
        // Paint (traffic lights, recolours) is part of the key: the voxel
        // worker's diff re-voxelizes only the recoloured bricks and the
        // probes near them relight; nothing else is re-traced.
        let key=[world.render_rev,world.paint_rev,terrain_rev,voxel_rev,self.models_rev];
        let center=self.gi.config().anchor.unwrap_or(center);
        if self.gi.wants_scene(key,center) {
            let start=Cx::monotonic_now();
            let snapshot=self.gi_snapshot(cx.cx,world,self.gi.region(center));
            self.gi.submit_scene(key,center,snapshot);
            self.gi.stats.snapshot_us=((Cx::monotonic_now()-start)*1e6)as u64;
        }
        self.gi.set_emission_gain(self.lin_ctl()[3]);
        self.gi.run(cx.cx,center,sun,&self.clustered);
        if self.gi.stats.batch>0 {if let Some(parent)=self.gi.relight_pass(){self.clustered.parent_shadows(cx.cx,parent);}}
        stats.gi=self.gi.stats;
        if self.clustered_frames%120==0 && std::env::var_os("MAKEPAD_GI_STATS").is_some(){
            let g=&stats.gi;
            log!("fast GI: {} instances / {} triangles ({} outside region), {} probes ({} pending), batch {} of budget {}, {} lights, {}us snapshot, {}us encode, voxels {} jobs / {} bricks / {:.1}ms, GPU [trace, relight, gather, scatter] {:?}ms peak {:?}ms, AO {:.3}ms, {} bytes resident, {} uploaded; building={} unsupported={}",
                g.instances,g.triangles,g.omitted_instances,g.probes,g.pending_probes,g.batch,g.budget,g.selected_lights,g.snapshot_us,g.encode_us,g.voxel_jobs,g.voxel_bricks,g.voxel_ms,g.gpu_ms.map(|v|(v*1000.0).round()/1000.0),g.gpu_peak_ms.map(|v|(v*1000.0).round()/1000.0),g.ao_gpu_ms,g.resident_bytes,g.uploads_bytes,g.building,g.rejected_scene);
        }
    }
}
