//! XR on the HAL (`info_flags::XR`): `xr_start_presenting` makes a `window_kind::XR` window (the
//! headset's stereo view, both eyes side by side). Each `XrFrame` the HAL sends becomes an
//! `Event::XrUpdate` and the eyes the next frame of that window is drawn with; the app's XR
//! pass (`DrawPass::set_as_xr_pass`) paints into it once per eye, the camera matrices of each
//! pass being that eye's view and projection -- or, on a backend with `info_flags::MULTIVIEW`,
//! once for both eyes (one multiview pass, both cameras in its uniforms).
use super::super::super::{
    cx::Cx,
    draw_pass::{CxDrawPassParent, DrawPassUniforms},
    event::{xr::{normalize_xr_controller_stick, XrController, XrHand, XrState, XrUpdateEvent}, Event, hal_intake::hal_window_id},
    makepad_math::{CameraFov, Mat4f, Pose, Quat, Vec2f, Vec3f},
    window::{WindowHandle, WindowId},
};
use makepad_hal::{buffers::{read_items, Wire}, window_kind, Id, XrDepth, XrFrame, XrPose};
use std::rc::Rc;

/// Near and far planes of the eye projections (metres in the room).
const EYE_NEAR: f32 = 0.05;
const EYE_FAR: f32 = 2000.0;

pub(crate) struct HalXr {
    /// the Cx window standing for the HAL's XR window
    pub(crate) window: WindowId,
    /// the newest `XrFrame`: the eyes of the next frame
    pub(crate) frame: Option<XrFrame>,
    /// its predicted display time (HAL clock, ns)
    pub(crate) display_ns: u64,
    pub(crate) last: Rc<XrState>,
    /// the pass uniforms of the frame being built, one per eye (alive until the batch is sent);
    /// a multiview pass uses the first, with both eyes in it
    pub(crate) eye_uniforms: [DrawPassUniforms; 2],
    /// the TSDF depth writer the headset's environment depth goes to (`cx.xr_tsdf()`), made
    /// at the first depth frame
    pub(crate) depth_writer: Option<makepad_tsdf::DepthWriter>,
    /// The newest environment depth image as a texture (its depth values 0..1) and its header
    /// (`Cx::xr_environment_depth`).
    pub(crate) env_depth: Option<(crate::texture::Texture, XrDepth)>,
}

// asked before the XR window exists: applied when it is made
thread_local! { static SEE_THROUGH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

fn pose(p: &XrPose) -> Pose {
    let [x, y, z, w] = p.orientation;
    Pose { orientation: Quat { x, y, z, w }, position: Vec3f { x: p.position[0], y: p.position[1], z: p.position[2] } }
}

/// The view and projection of eye `i` of `frame`.
pub(crate) fn eye_matrices(frame: &XrFrame, i: usize) -> (Mat4f, Mat4f) {
    let eye = &frame.eyes[i];
    let view = pose(&eye.pose).invert().to_mat4();
    let [angle_left, angle_right, angle_up, angle_down] = eye.fov;
    let projection = Mat4f::from_camera_fov(&CameraFov { angle_left, angle_right, angle_up, angle_down }, EYE_NEAR, EYE_FAR);
    (view, projection)
}

impl Cx {
    /// `xr_start_presenting`: the XR window, once (a backend without XR: nothing to present on).
    pub(crate) fn hal_xr_start(&mut self) {
        if self.hal.xr.is_some() { return; }
        if !self.hal.info.is_some_and(|i| i.flags & makepad_hal::info_flags::XR != 0) {
            self.hal.frames.gap("XR presenting needs a head-mounted display (info_flags::XR)");
            return;
        }
        let window = WindowHandle::new(self).window_id();
        let w = &mut self.windows[window];
        w.is_created = true;
        w.window_geom.xr_is_presenting = true;
        self.hal.windows.insert(window.0, super::HalWindow::default());
        // the HAL sizes it (its first Geom confirms it)
        self.hal.frames.w.window_create(hal_window_id(window), window_kind::XR, Id::NONE, [0.0; 4], false);
        self.hal.xr = Some(HalXr { window, frame: None, display_ns: 0, last: Rc::new(XrState::default()), eye_uniforms: Default::default(), depth_writer: None, env_depth: None });
        if SEE_THROUGH.with(|s| s.get()) { self.hal_xr_see_through(true); }
        // the XR passes paint into it from now on
        self.redraw_all();
    }

    pub(crate) fn hal_xr_see_through(&mut self, on: bool) {
        SEE_THROUGH.with(|s| s.set(on));
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.window_visuals(hal_window_id(xr.window), on as u32, [0.0; 4]);
    }

    /// `Cx::xr_present_eyes`: the eyes of the XR window's next frame.
    pub(crate) fn hal_xr_present_eyes(&mut self, eyes: [makepad_hal::XrEye; 2]) {
        let Some(xr) = self.hal.xr.as_ref() else { return };
        let mut bytes = Vec::with_capacity(2 * <makepad_hal::XrEye as makepad_hal::buffers::Wire>::SIZE);
        for e in &eyes { makepad_hal::buffers::Wire::write(e, &mut bytes); }
        self.hal.frames.w.xr_frame_eyes(hal_window_id(xr.window), &bytes);
    }

    /// `Cx::xr_set_pose_lead`.
    pub(crate) fn hal_xr_pose_lead(&mut self, lead_ns: u64) {
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.xr_pose_lead(hal_window_id(xr.window), lead_ns);
    }

    /// `Cx::xr_set_refresh_rate`.
    pub(crate) fn hal_xr_refresh_rate(&mut self, millihertz: u32) {
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.xr_refresh_rate(hal_window_id(xr.window), millihertz);
    }

    pub(crate) fn hal_xr_stop(&mut self) {
        let Some(xr) = self.hal.xr.take() else { return };
        self.hal.frames.w.window_close(hal_window_id(xr.window));
        self.hal.windows.remove(&xr.window.0);
    }

    /// An `XrFrame`: the eyes of the XR window's next frame, and `Event::XrUpdate` for the app.
    pub(crate) fn hal_xr_frame(&mut self, time: f64, display_ns: u64, bytes: &[u8]) {
        let Some(frame) = read_items::<XrFrame>(bytes, 1).next() else { return };
        let Some(xr) = self.hal.xr.as_mut() else { return };
        xr.frame = Some(frame);
        xr.display_ns = display_ns;
        // the HAL's controllers and hands are the old backend's, field for field
        let controller = |c: &makepad_hal::XrController| XrController { grip_pose: pose(&c.grip_pose), aim_pose: pose(&c.aim_pose), trigger: c.trigger,
            grip: c.grip, buttons: c.buttons as u16, stick: normalize_xr_controller_stick(Vec2f { x: c.stick[0], y: c.stick[1] }) };
        let hand = |h: &makepad_hal::XrHand| {
            let mut out = XrHand { flags: h.flags as u8, tips: h.tips, tips_active: h.tips_active as u8, aim_pose: pose(&h.aim_pose), pinch: h.pinch, ..Default::default() };
            for (o, j) in out.joints.iter_mut().zip(h.joints.iter()) { *o = pose(j); }
            out
        };
        let last = xr.last.clone();
        let state = Rc::new(XrState {
            time,
            head_pose: pose(&frame.head),
            order_counter: last.order_counter.wrapping_add(1),
            left_controller: controller(&frame.controllers[0]),
            right_controller: controller(&frame.controllers[1]),
            left_hand: hand(&frame.hands[0]),
            right_hand: hand(&frame.hands[1]),
            ..Default::default()
        });
        xr.last = state.clone();
        // the XR pass draws with the new eyes
        let passes: Vec<_> = self.passes.id_iter().filter(|p| matches!(self.passes[*p].parent, CxDrawPassParent::Xr)).collect();
        for pass in passes { self.redraw_pass_and_child_passes(pass); }
        self.call_event_handler(&Event::XrUpdate(XrUpdateEvent { state, last }));
    }

    /// An `XrDepth`: the headset's environment depth, handed to the TSDF depth writer (its
    /// own thread integrates it into `cx.xr_tsdf()`; this never waits).
    pub(crate) fn hal_xr_depth(&mut self, bytes: &[u8]) {
        let Some(xr) = self.hal.xr.as_mut() else { return };
        let Some(header) = read_items::<XrDepth>(bytes, 1).next() else { return };
        let texels = (header.width * header.height) as usize;
        let Some(raw) = bytes.get(XrDepth::SIZE..XrDepth::SIZE + texels * 2) else { return };
        let depth: Vec<u16> = raw.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        // the newest image as a texture for the app's own per-pixel tests (occlusion): each
        // D16 texel as two bytes (high in red, low in green) of an 8-bit RGBA texel, read at
        // texel centres (a linear sampler on a 32-bit float texture is not filterable on every
        // GPU: on the Quest it sampled garbage, the occlusion looked tiled)
        let values: Vec<u32> = depth.iter().map(|d| 0xff00_0000 | ((*d as u32 >> 8) << 16) | ((*d as u32 & 0xff) << 8)).collect();
        let (w, h) = (header.width as usize, header.height as usize);
        let reuse = xr.env_depth.as_ref().filter(|(_, old)| old.width == header.width && old.height == header.height).map(|(t, _)| t.clone());
        let texture = match reuse {
            Some(t) => {
                let _ = t.take_vec_u32(self);
                t.put_back_vec_u32(self, values, None);
                t
            }
            None => crate::texture::Texture::new_with_format(self, crate::texture::TextureFormat::VecBGRAu8_32 { width: w, height: h, data: Some(values), updated: crate::texture::TextureUpdated::Full }),
        };
        let Some(xr) = self.hal.xr.as_mut() else { return };
        xr.env_depth = Some((texture, header));
        if xr.depth_writer.is_none() {
            match makepad_tsdf::DepthWriter::new() {
                Ok(writer) => xr.depth_writer = Some(writer),
                Err(e) => {
                    crate::error!("XR depth: {e}");
                    return;
                }
            }
        }
        let view = |e: &makepad_hal::XrEye| makepad_tsdf::DepthFrameView { orientation: e.pose.orientation, position: e.pose.position, fov: e.fov };
        let frame = makepad_tsdf::DepthFrame {
            width: header.width,
            height: header.height,
            eye: header.eye as usize,
            near: header.near,
            far: header.far,
            views: [view(&header.views[0]), view(&header.views[1])],
            depth,
            // the hands stay in the depth image (occlusion) but not in the room field: 5 cm
            // around each tracked joint (the newest hand poses; the image is a few ms older)
            exclude: [&xr.last.left_hand, &xr.last.right_hand]
                .into_iter()
                .filter(|h| h.in_view())
                .flat_map(|h| h.joints.iter().map(|j| [j.position.x, j.position.y, j.position.z, 0.05]))
                .collect(),
        };
        if let Some(writer) = xr.depth_writer.as_mut() {
            let _ = writer.submit(frame);
        }
    }

    /// The uniforms of eye `i` for an XR pass: its own uniforms with that eye's camera (left
    /// and right fields alike: each eye is a pass of its own).
    pub(crate) fn hal_xr_eye_uniforms(&mut self, base: &DrawPassUniforms, i: usize) -> Option<&DrawPassUniforms> {
        let xr = self.hal.xr.as_mut()?;
        let frame = xr.frame?;
        let (view, projection) = eye_matrices(&frame, i);
        let inv = view.invert();
        let u = &mut xr.eye_uniforms[i];
        *u = *base;
        u.camera_view = view; u.camera_view_r = view;
        u.camera_projection = projection; u.camera_projection_r = projection;
        u.depth_view = view; u.depth_view_r = view;
        u.depth_projection = projection; u.depth_projection_r = projection;
        u.camera_inv = inv; u.camera_inv_r = inv;
        Some(&xr.eye_uniforms[i])
    }

    /// The uniforms of a multiview XR pass: the left eye's camera in the plain fields, the
    /// right eye's in the `_r` ones (the multiview stages pick them by view index).
    pub(crate) fn hal_xr_stereo_uniforms(&mut self, base: &DrawPassUniforms) -> Option<&DrawPassUniforms> {
        let xr = self.hal.xr.as_mut()?;
        let frame = xr.frame?;
        let (view, projection) = eye_matrices(&frame, 0);
        let (view_r, projection_r) = eye_matrices(&frame, 1);
        let u = &mut xr.eye_uniforms[0];
        *u = *base;
        u.camera_view = view; u.camera_view_r = view_r;
        u.camera_projection = projection; u.camera_projection_r = projection_r;
        u.depth_view = view; u.depth_view_r = view_r;
        u.depth_projection = projection; u.depth_projection_r = projection_r;
        u.camera_inv = view.invert(); u.camera_inv_r = view_r.invert();
        Some(&xr.eye_uniforms[0])
    }
}
