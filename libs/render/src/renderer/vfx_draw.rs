//! Particles and VFX decals: the host API and both draw paths (the scene's
//! late layer, or the separate soft pass) — see vfx/mod.rs.

use super::*;
use crate::particles::{ParticleSystem, PARTICLE_SHEET};
use crate::vfx::{record_radius, DrawSceneVfx, DrawSceneVfxDecal, VfxFrame, VfxLight};

impl Renderer {
    /// Hand this frame's particles to the renderer: explicit records (a
    /// host's markers) or burst records. Replaces the previous frame's.
    pub fn set_particles(&mut self, instances: Vec<ParticleInstance>) {
        self.vfx.records = instances;
    }

    /// Add a particle system's frame: its bursts (or their CPU expansion),
    /// its light flashes (as this frame's transient lights) and its decals.
    pub fn set_vfx(&mut self, fx: &ParticleSystem) {
        if self.vfx.cpu_sim {
            self.vfx.records.extend(fx.cpu_instances());
        } else {
            self.vfx.records.extend(fx.instances());
        }
        self.host_lights.extend(fx.frame_lights());
        self.vfx.decals.clear();
        self.vfx.decals.extend_from_slice(fx.decals());
    }

    /// Draw particles in their own pass over the finished scene, soft
    /// against its depth (the host then calls [`Self::run_vfx`] after its
    /// scene pass). Off: they draw inside the scene pass, hard-edged.
    pub fn set_vfx_pass(&mut self, on: bool) {
        self.vfx.pass_enabled = on;
    }

    /// Evaluate particles on the CPU (one explicit record each) instead of
    /// in the vertex shader — for backends where the closed form should not
    /// run per vertex.
    pub fn set_vfx_cpu_sim(&mut self, on: bool) {
        self.vfx.cpu_sim = on;
    }

    /// GPU ms of the VFX pass (a frame or two behind; 0 without the pass).
    pub fn vfx_gpu_ms(&self) -> f64 {
        self.vfx.gpu_ms
    }

    /// The scene pass's VFX layer: decals always; particles here unless the
    /// host runs the soft pass, in which case this frame's view is kept for
    /// [`Self::run_vfx`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_vfx_layer(
        &mut self,
        cx: &mut Cx3d,
        scene: &SceneState3D,
        stage: Mat4f,
        frustum: Option<&Frustum>,
        light: VfxLight,
        scale: f32,
        stats: &mut RenderStats,
    ) {
        self.draw_vfx_decals(cx, frustum, &light);
        let frame = VfxFrame { scene: *scene, stage, light, scale };
        if self.vfx.pass_enabled && self.stage.mode == StageMode::Flat {
            self.vfx.pending = Some(frame);
            return;
        }
        let mut draw = self.vfx.draw_scene.take();
        if draw.is_none() {
            draw = cx.cx.try_with_vm(|vm| Box::new(DrawSceneVfx::script_new_with_default(vm)));
        }
        if let Some(mut d) = draw {
            stats.particles += self.emit_particles(cx, &mut d, &frame, frustum, None);
            self.vfx.draw_scene = Some(d);
        }
        self.vfx.records.clear();
    }

    fn draw_vfx_decals(&mut self, cx: &mut Cx3d, frustum: Option<&Frustum>, light: &VfxLight) {
        if self.vfx.decals.is_empty() {
            return;
        }
        if self.vfx.draw_decal.is_none() {
            self.vfx.draw_decal = cx.cx.try_with_vm(|vm| Box::new(DrawSceneVfxDecal::script_new_with_default(vm)));
        }
        let Some(mut d) = self.vfx.draw_decal.take() else { return };
        let geometry_id = self.ensure_flare_geometry(cx.cx);
        d.draw_vars.geometry_id = Some(geometry_id);
        d.depth_clip = 1.0;
        // Ambient plus the sun on an up-facing surface: what a mark's
        // albedo is lit by.
        let lit = light.sky + light.sun * (light.dir.y.max(0.0) * 0.8);
        d.draw_vars.set_uniform(cx.cx, live_id!(decal_light), &[lit.x, lit.y, lit.z]);
        d.draw_vars.set_uniform(cx.cx, live_id!(lin_ctl), &light.lin);
        for m in &self.vfx.decals {
            if let Some(f) = frustum {
                if !f.intersects_sphere(m.pos, m.half_len.max(m.half_width)) {
                    continue;
                }
            }
            d.d_pos = vec4(m.pos.x, m.pos.y, m.pos.z, m.half_len);
            d.d_normal = vec4(m.normal.x, m.normal.y, m.normal.z, m.half_width);
            d.d_dir = vec4(m.dir.x, m.dir.y, m.dir.z, m.kind.id());
            d.d_color = vec4(m.color.x, m.color.y, m.color.z, m.color.w * m.fade());
            d.d_extra = vec4(m.seed, 0.0, 0.0, 0.0);
            if d.draw_vars.can_instance() {
                let new_area = cx.add_instance(&d.draw_vars);
                d.draw_vars.area = cx.update_area_refs(d.draw_vars.area, new_area);
            }
        }
        self.vfx.draw_decal = Some(d);
    }

    /// Sort, cull, thin and split this frame's records, then issue them:
    /// burst chunks on the particle sheet, explicit particles on a single
    /// quad. Returns the particles submitted.
    fn emit_particles(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut DrawSceneVfx,
        frame: &VfxFrame,
        frustum: Option<&Frustum>,
        depth: Option<(&Texture, [f32; 4])>,
    ) -> u64 {
        if self.vfx.records.is_empty() {
            return 0;
        }
        let cam = frame.scene.camera_pos;
        let mut chunks = std::mem::take(&mut self.vfx.chunks);
        chunks.clear();
        let mut explicit: Vec<(f32, ParticleInstance)> = Vec::new();
        let mut submitted = 0u64;
        for r in &self.vfx.records {
            let o = vec3f(r.origin.x, r.origin.y, r.origin.z);
            if let Some(f) = frustum {
                if !f.intersects_sphere(o, record_radius(r)) {
                    continue;
                }
            }
            let key = (o - cam).length();
            if r.is_explicit() {
                explicit.push((key, *r));
                submitted += 1;
                continue;
            }
            // Thermal thinning keeps a prefix of every burst: fewer
            // particles in each, never whole effects missing.
            let n = ((r.count() as f32) * frame.scale).ceil() as usize;
            if n == 0 {
                continue;
            }
            let mut c = *r;
            c.extra2.x = n as f32;
            submitted += n as u64;
            for first in (0..n).step_by(PARTICLE_SHEET) {
                c.extra.w = first as f32;
                chunks.push((key, c));
            }
        }
        // Back to front, so alpha smoke composites in order. The instances
        // are ~200 bytes each: sort (distance, index) pairs, not the
        // instances (the tie-break keeps the stable order the draw had).
        let back_to_front = |batch: &[(f32, ParticleInstance)]| {
            let mut order: Vec<(f32, u32)> = batch.iter().enumerate().map(|(i, c)| (c.0, i as u32)).collect();
            order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            order
        };
        let chunk_order = back_to_front(&chunks);
        let explicit_order = back_to_front(&explicit);

        let (atlas, atlas_on) = self.vfx.atlas_texture(cx.cx);
        let l = &frame.light;
        let dv = &mut draw.draw_vars;
        dv.set_texture(0, &atlas);
        dv.set_texture(1, depth.map(|d| d.0).unwrap_or(&atlas));
        dv.set_uniform(cx.cx, live_id!(vfx_ctl), &[if atlas_on { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);
        dv.set_uniform(cx.cx, live_id!(vfx_depth), &depth.map(|d| d.1).unwrap_or([0.0; 4]));
        dv.set_uniform(cx.cx, live_id!(lin_ctl), &l.lin);
        dv.set_uniform(cx.cx, live_id!(light_dir), &[l.dir.x, l.dir.y, l.dir.z]);
        dv.set_uniform(cx.cx, live_id!(sun_color), &[l.sun.x, l.sun.y, l.sun.z]);
        dv.set_uniform(cx.cx, live_id!(sun_sky), &[l.sky.x, l.sky.y, l.sky.z]);
        dv.set_uniform(cx.cx, live_id!(fog_color), &[l.fog.x, l.fog.y, l.fog.z]);
        dv.set_uniform(cx.cx, live_id!(fog_density), &[l.fog_density]);
        draw.depth_clip = 1.0;
        for (batch, order, geometry) in [(&chunks, &chunk_order, self.vfx.sheet_geometry(cx.cx, PARTICLE_SHEET)), (&explicit, &explicit_order, self.vfx.sheet_geometry(cx.cx, 1))] {
            if batch.is_empty() {
                continue;
            }
            draw.draw_vars.geometry_id = Some(geometry);
            for &(_, i) in order.iter() {
                let c = &batch[i as usize].1;
                draw.set_record(c);
                if draw.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draw.draw_vars);
                    draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
                }
            }
        }
        self.vfx.chunks = chunks;
        submitted
    }

    /// The soft VFX pass: draws this frame's particles over the finished
    /// scene target `color`, fading each against the scene's hardware
    /// `depth`. Call right after the scene pass (and after the post chain
    /// has re-parented it): the pass slots itself between the scene and
    /// whatever consumed it. None when nothing was left for it.
    /// The soft-particle pass draws into the scene target, so it must
    /// resolve to exactly the scene pass's size. The host calls this after
    /// it has placed its scene pass (`set_pass_area`), every frame: a pass
    /// sized on its own (`set_size`) differed from an area-placed scene pass
    /// whenever the render scale was below 1, and the shared target was
    /// re-allocated between the two passes — the scene was lost and the
    /// view drew black wherever particles were alive.
    pub fn follow_scene_pass_rect(&self, cx: &mut Cx, scene_pass: &DrawPass) {
        if let Some((pass, _)) = &self.vfx.pass {
            let rect = cx.passes[scene_pass.draw_pass_id()].pass_rect.clone();
            cx.passes[pass.draw_pass_id()].pass_rect = rect;
        }
    }

    pub fn run_vfx(&mut self, cx: &mut Cx2d, size: DVec2, color: &Texture, depth: &Texture, scene_pass: &DrawPass) -> Option<DrawPassId> {
        let frame = self.vfx.pending.take();
        if frame.is_none() || self.vfx.records.is_empty() {
            // No particles this frame: the pass must not stay attached to
            // the scene target. A detached pass still repaints on its own,
            // at the size it last had — after a resize (a split pane, an
            // adaptive render scale) that re-allocated the scene target at
            // that stale size every frame and the scene drew black.
            if let Some((pass, _)) = &self.vfx.pass {
                pass.clear_color_textures(cx.cx);
            }
            return None;
        }
        let frame = frame?;
        if self.vfx.draw_pass.is_none() {
            self.vfx.draw_pass = cx.cx.try_with_vm(|vm| Box::new(DrawSceneVfx::script_new_with_default(vm)));
        }
        let mut draw = self.vfx.draw_pass.take()?;
        let (pass, mut list) = self.vfx.pass.take().unwrap_or_else(|| {
            let pass = DrawPass::new_with_name(cx.cx, "vfx");
            pass.set_gpu_timing_enabled(cx.cx, true);
            (pass, DrawList::new(cx.cx))
        });
        let id = pass.draw_pass_id();
        let scene_id = scene_pass.draw_pass_id();
        let parent = cx.cx.passes[scene_id].parent.clone();
        pass.set_size(cx, size);
        pass.clear_color_textures(cx.cx);
        // Load, never clear: the scene is already in the target.
        pass.set_color_texture(cx.cx, color, DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 0.0)));
        cx.cx.passes[id].depth_texture = None;
        cx.cx.passes[id].keep_camera_matrix = true;
        cx.cx.passes[id].parent = parent;
        cx.cx.passes[scene_id].parent = CxDrawPassParent::DrawPass(id);
        crate::scene::set_pass_camera(cx.cx, &pass, &frame.scene);
        if let Some(ms) = pass.take_gpu_times_ms(cx.cx).last() {
            self.vfx.note_gpu_ms(*ms);
        }
        let p = frame.scene.projection.v;
        // Metal, Vulkan and D3D keep clip z/w as the stored depth; only GL
        // maps it to 0..1. Read as GL, a Vulkan depth put the scene a few
        // metres from the eye, so every particle with geometry behind it was
        // discarded (a hearth's flames; only sparks against the sky showed).
        let unmapped = match cx.cx.gpu_backend() {
            GpuBackend::OpenGl | GpuBackend::WebGl => 0.0,
            _ => 1.0,
        };
        let depth_ctl = [1.0, p[10], p[14], unmapped];
        // The scene target's own density: a pass at any other dpi would
        // re-allocate the shared target and lose the scene in it.
        let dpi = cx.current_dpi_factor();
        cx.begin_pass(&pass, Some(dpi));
        list.begin_always(cx);
        {
            let cx3d = &mut Cx3d::new(cx.cx);
            cx3d.begin_scene_3d(frame.scene);
            let previous = cx3d.set_scene_world_transform_3d(frame.stage);
            cx3d.cx.draw_lists[list.id()].draw_list_uniforms.view_transform = frame.stage;
            self.emit_particles(cx3d, &mut draw, &frame, None, Some((depth, depth_ctl)));
            if let Some(previous) = previous {
                let _ = cx3d.set_scene_world_transform_3d(previous);
            }
            cx3d.end_scene_3d();
        }
        list.end(cx);
        cx.end_pass(&pass);
        self.vfx.records.clear();
        self.vfx.draw_pass = Some(draw);
        self.vfx.pass = Some((pass, list));
        Some(id)
    }
}
