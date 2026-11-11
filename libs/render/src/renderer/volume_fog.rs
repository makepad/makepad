//! Host scheduling of the shared fog chain. Like soft VFX, inputs are
//! prepared while the scene is recorded and the passes run after the scene.
use super::*;
use crate::volume_fog::{VolumeFogCamera, VolumeFogFrame, VolumeFogPass};

pub(super) struct PendingFog {
    scene: DrawPassId,
    depth: Texture,
    size: DVec2,
}
#[derive(Default)]
pub(super) struct VolumeFogState {
    pub params: makepad_scene::VolumeFog,
    pub pass: VolumeFogPass,
    pending: Option<PendingFog>,
    sun: Option<SunLight>,
    origin: Vec3f,
    warned_legacy: bool,
}
impl VolumeFogState {
    pub fn reset(&mut self) {
        self.pending = None;
        self.sun = None;
        self.pass.reset_history();
    }
}

impl Renderer {
    /// Enable a general height medium. Density zero allocates no targets and
    /// binds no fog input. Hosts call `run_volume_fog` and `run_post` per frame.
    pub fn set_volume_fog(&mut self, fog: makepad_scene::VolumeFog) {
        let fog = fog.sanitized();
        if self.volume_fog.params != fog { self.volume_fog.reset(); }
        self.volume_fog.params = fog;
    }

    /// Prepare fog beside `run_screen_ao`, before drawing the scene. Recording
    /// is completed by `run_post`, so the march reads THIS frame's depth,
    /// sun and cascades. `depth` is the same sampled hardware depth as SSAO.
    /// Non-HDR output runs nothing and logs once; XR hosts must skip this call.
    pub fn run_volume_fog(&mut self, cx: &mut Cx2d, scene_pass: &DrawPass, depth: &Texture, size: DVec2) {
        self.volume_fog.pending = None;
        self.volume_fog.sun = None;
        self.volume_fog.pass.begin_frame();
        if self.volume_fog.params.active() && !self.hdr_output && !self.volume_fog.warned_legacy {
            log!("volume fog requires HDR output; fog passes disabled");
            self.volume_fog.warned_legacy = true;
        }
        if !self.volume_fog.params.active() || !self.hdr_output || !size.x.is_finite() || !size.y.is_finite() || size.x < 2.0 || size.y < 2.0 {
            self.volume_fog.pass.disable(cx.cx);
            return;
        }
        if self.volume_fog.pass.ensure(cx.cx) {
            self.volume_fog.pending = Some(PendingFog { scene: scene_pass.draw_pass_id(), depth: depth.clone(), size });
        }
    }

    /// Withdraw this view's fog passes when the host skips post processing,
    /// for example while presenting XR. Keeps the authored setting.
    pub fn skip_volume_fog(&mut self, cx: &mut Cx) {
        self.volume_fog.reset();
        self.volume_fog.pass.disable(cx);
    }

    pub(super) fn volume_fog_active_for(&self, scene: DrawPassId) -> bool {
        self.hdr_output && self.volume_fog.params.active() && self.volume_fog.pending.as_ref().is_some_and(|p| p.scene == scene)
    }

    pub(super) fn prepare_volume_fog_lighting(&mut self, scene: DrawPassId, sun: SunLight, eye: Vec3f) {
        if !self.volume_fog_active_for(scene) { return; }
        self.volume_fog.origin = self.render_origin_for(eye);
        self.volume_fog.sun = Some(sun);
        self.volume_fog.pass.set_lights(&self.frame_lights, eye);
    }

    pub(super) fn record_volume_fog(&mut self, cx: &mut Cx2d, scene: &Texture, parent: DrawPassId) -> Option<DrawPassId> {
        let pending = self.volume_fog.pending.take();
        let sun = self.volume_fog.sun.take();
        let (Some(pending), Some(sun)) = (pending, sun) else {
            self.volume_fog.pass.disable(cx.cx);
            return None;
        };
        if !self.hdr_output { self.volume_fog.pass.disable(cx.cx); return None; }
        // Camera uniforms are the ones used by the actual depth producer.
        // Its view may be relative to a snapped render origin; CSM and
        // authored medium height are absolute world quantities.
        let pass = &cx.passes[pending.scene].pass_uniforms;
        let mut shift = Mat4f::identity();
        shift.v[12] = -self.volume_fog.origin.x;
        shift.v[13] = -self.volume_fog.origin.y;
        shift.v[14] = -self.volume_fog.origin.z;
        let relative_clip = Mat4f::mul(&pass.camera_projection, &pass.camera_view);
        let mut camera = VolumeFogCamera::from_clip(relative_clip);
        let origin = self.volume_fog.origin;
        let mut unshift = Mat4f::identity();
        unshift.v[12] = origin.x;
        unshift.v[13] = origin.y;
        unshift.v[14] = origin.z;
        // Derive the eye before adding the origin: a distant eye must not
        // look orthographic to the homogeneous-coordinate tolerance.
        camera.general.clip_from_world = Mat4f::mul(&relative_clip, &shift).v;
        camera.general.world_from_clip = Mat4f::mul(&unshift, &Mat4f { v: camera.general.world_from_clip }).v;
        if camera.eye[3] > 0.5 {
            camera.eye[0] += origin.x;
            camera.eye[1] += origin.y;
            camera.eye[2] += origin.z;
        }
        if camera.eye[3] == 0.0 {
            let near = match cx.cx.gpu_backend() { GpuBackend::OpenGl | GpuBackend::WebGl => -1.0, _ => 0.0 };
            let h = Mat4f { v: camera.general.world_from_clip }.transform_vec4(vec4(0.0, 0.0, near, 1.0));
            if h.w.abs() > 1e-7 { camera.reference = [h.x / h.w, h.y / h.w, h.z / h.w, 0.0]; }
        }
        let binding = self.gpu_baker.csm_binding();
        if let Some(vars) = self.volume_fog.pass.march_draw_vars() {
            // Scene depth is a valid depth-kind fallback when CSM is off;
            // the shader's csm_p early-out ensures it is never sampled.
            Self::write_csm_uniforms(cx.cx, vars, &binding, &pending.depth, 1, false);
        }
        self.volume_fog.pass.run(cx, VolumeFogFrame {
            fog: self.volume_fog.params, camera, sun, depth: &pending.depth,
            scene, size: pending.size, parent,
        })
    }
}
