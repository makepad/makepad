//! Per-view scheduling; hosts own the two optional scene attachments.
use super::*;
use crate::ssr::{SsrFrame, SsrPass, SsrSettings};
use crate::volume_fog::VolumeFogCamera;

struct PendingSsr {
    scene: DrawPassId, depth: Texture, surface: Texture, envspec: Texture, size: DVec2,
}
#[derive(Default)]
pub(super) struct SsrState {
    pub settings: SsrSettings,
    pub pass: SsrPass,
    pending: Option<PendingSsr>,
    origin: Vec3f,
    warned_capability: bool,
}
impl SsrState {
    pub fn reset(&mut self) { self.pending = None; self.pass.reset_history(); }
    pub fn active(&self) -> bool { self.settings.active() && self.pending.is_some() }
}
impl Renderer {
    pub fn set_ssr(&mut self, settings: SsrSettings) {
        let settings = settings.sanitized();
        if self.ssr.settings != settings { self.ssr.reset(); }
        self.ssr.settings = settings;
    }

    /// Hosts use the same gate before allocating/attaching SURFACE and ENVSPEC.
    /// Off, non-HDR, and unsupported devices bind no extra shader outputs.
    pub fn ssr_available(&mut self, cx: &Cx) -> bool {
        if self.ssr.settings.active() && !cx.gpu_info().mixed_output_passes && !self.ssr.warned_capability {
            log!("reflections unavailable: this GPU cannot mix draws with different outputs in one pass");
            self.ssr.warned_capability = true;
        }
        self.hdr_output && self.ssr.settings.active() && cx.gpu_info().mixed_output_passes
    }

    /// Schedule beside volume fog before the scene draws. Attachments 1/2
    /// are the host's zero-cleared SURFACE (eight-bit RGBA) and ENVSPEC (16F).
    /// `run_post` records the chain after the scene camera has been supplied.
    /// XR hosts call `skip_ssr` instead.
    pub fn run_ssr(&mut self, cx: &mut Cx2d, scene_pass: &DrawPass, depth: &Texture, size: DVec2) {
        self.ssr.pending = None;
        self.ssr.pass.begin_frame();
        if !self.ssr_available(cx.cx) || !size.x.is_finite() || !size.y.is_finite() || size.x < 2.0 || size.y < 2.0 {
            self.ssr.pass.disable(cx.cx);
            return;
        }
        let scene = scene_pass.draw_pass_id();
        let attachments = &cx.passes[scene].color_textures;
        let (Some(surface), Some(envspec)) = (attachments.get(1), attachments.get(2)) else {
            self.ssr.pass.disable(cx.cx);
            return;
        };
        let (surface, envspec) = (surface.texture.clone(), envspec.texture.clone());
        if self.ssr.pass.ensure(cx.cx) {
            self.ssr.pending = Some(PendingSsr { scene, depth: depth.clone(), surface, envspec, size });
        }
    }

    pub fn skip_ssr(&mut self, cx: &mut Cx) {
        self.ssr.reset();
        self.ssr.pass.disable(cx);
    }

    pub(super) fn prepare_ssr_camera(&mut self, scene: DrawPassId, eye: Vec3f) {
        if self.ssr.pending.as_ref().is_some_and(|p| p.scene == scene) {
            self.ssr.origin = self.render_origin_for(eye);
        }
    }

    pub(super) fn record_ssr(&mut self, cx: &mut Cx2d, scene: &Texture, parent: DrawPassId) -> Option<DrawPassId> {
        let pending = self.ssr.pending.take();
        let Some(pending) = pending.filter(|_| self.ssr_available(cx.cx)) else {
            self.ssr.pass.disable(cx.cx);
            return None;
        };
        let pass = &cx.passes[pending.scene].pass_uniforms;
        let relative_clip = Mat4f::mul(&pass.camera_projection, &pass.camera_view);
        let mut camera = VolumeFogCamera::from_clip(relative_clip);
        let origin = self.ssr.origin;
        let mut shift = Mat4f::identity();
        shift.v[12] = -origin.x;
        shift.v[13] = -origin.y;
        shift.v[14] = -origin.z;
        let mut unshift = Mat4f::identity();
        unshift.v[12] = origin.x;
        unshift.v[13] = origin.y;
        unshift.v[14] = origin.z;
        camera.general.clip_from_world = Mat4f::mul(&relative_clip, &shift).v;
        camera.general.world_from_clip = Mat4f::mul(&unshift, &Mat4f { v: camera.general.world_from_clip }).v;
        if camera.eye[3] > 0.5 {
            camera.eye[0] += origin.x;
            camera.eye[1] += origin.y;
            camera.eye[2] += origin.z;
        } else {
            let near = match cx.cx.gpu_backend() { GpuBackend::OpenGl | GpuBackend::WebGl => -1.0, _ => 0.0 };
            let h = Mat4f { v: camera.general.world_from_clip }.transform_vec4(vec4(0.0, 0.0, near, 1.0));
            if h.w.abs() > 1e-7 { camera.reference = [h.x / h.w, h.y / h.w, h.z / h.w, 0.0]; }
        }
        self.ssr.pass.run(cx, SsrFrame {
            settings: self.ssr.settings, camera, depth: &pending.depth, scene,
            surface: &pending.surface, envspec: &pending.envspec, size: pending.size, parent,
        })
    }
}
